import EchodeBridge
import Foundation
import Observation

struct ExportError: LocalizedError, Sendable {
    let message: String
    var errorDescription: String? { message }
}

/// The window owns this job, not the currently open document. A document change
/// cannot retarget its output, settings, snapshot, or completion.
@MainActor @Observable
final class ExportJob {
    private(set) var running = false
    private(set) var message: String?
    private(set) var detail = ""
    @ObservationIgnored var executable: URL? = Bundle.main.executableURL?.deletingLastPathComponent().appendingPathComponent("echode-cli")

    @discardableResult
    func start(controller: ProjectController, document: DocumentID, project: URL,
               output: URL, settings: ExportSettings, defaults: UserDefaults = .standard) -> Task<Void, Never>? {
        guard !running, controller.acceptsInput(document),
              controller.projectURL?.resolvingSymlinksInPath() == project.resolvingSymlinksInPath(), let arguments = settings.arguments else { return nil }
        // Do not let an export overwrite a source package or one of its objects.
        let sourcePath = project.resolvingSymlinksInPath().path
        let outputPath = output.resolvingSymlinksInPath().path
        guard outputPath != sourcePath, !outputPath.hasPrefix(sourcePath + "/") else {
            message = text("Choose an output outside the project package"); return nil
        }
        running = true; message = text("Preparing export"); detail = ""
        let executable = self.executable
        return Task {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-\(UUID())")
            var ownsRoot = false
            do {
                guard let executable, FileManager.default.isExecutableFile(atPath: executable.path) else {
                    throw ExportError(message: "Export CLI is missing from the app bundle")
                }
                try await Task.detached {
                    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
                }.value
                ownsRoot = true
                let frozen = root.appendingPathComponent("Snapshot.echode")
                try await controller.freezeExport(to: frozen, documentID: document)
                settings.save(project: project, defaults: defaults)
                message = text("Exporting audio")
                let result = try await Task.detached {
                    try Self.render(executable: executable, package: frozen, output: output, arguments: arguments, work: root)
                }.value
                detail = "Source: \(project.path)\nOutput: \(output.path)\nOptions: \(arguments.joined(separator: " "))\n" + result.detail
                message = text(result.success ? "Export complete" : "Export failed") + ": " + output.lastPathComponent
            } catch {
                detail = (error as? BridgeError)?.message ?? error.localizedDescription
                message = text("Export failed") + ": " + output.lastPathComponent
            }
            if ownsRoot {
                let cleanup = await Task.detached { Result { try FileManager.default.removeItem(at: root) } }.value
                if case .failure(let error) = cleanup { detail += "\nTemporary snapshot cleanup failed: \(error.localizedDescription)" }
            }
            running = false
        }
    }

    nonisolated static func render(executable: URL, package: URL, output: URL, arguments: [String], work: URL) throws -> (success: Bool, detail: String) {
        let log = work.appendingPathComponent("render.log")
        guard FileManager.default.createFile(atPath: log.path, contents: nil) else {
            throw ExportError(message: "Could not create export log")
        }
        let handle = try FileHandle(forWritingTo: log)
        defer { try? handle.close() }
        let process = Process()
        process.executableURL = executable
        process.arguments = ["render", package.path, output.path] + arguments
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice
        // A file rather than a pipe: even a verbose failing helper cannot fill
        // a pipe while the parent waits for termination off the main actor.
        process.standardError = handle
        try process.run()
        process.waitUntilExit()
        let reader = try FileHandle(forReadingFrom: log)
        defer { try? reader.close() }
        let data = try reader.read(upToCount: 65_536) ?? Data()
        return (process.terminationReason == .exit && process.terminationStatus == 0,
                "Exit: \(process.terminationStatus) (\(process.terminationReason))\n" + String(decoding: data, as: UTF8.self))
    }
}

extension ProjectController {
    func freezeExport(to destination: URL, documentID: DocumentID) async throws {
        guard let captured = inputSession(for: documentID), captured.faderPreview == nil,
              captured.parameterGesture == nil, captured.pluginGestures.isEmpty else {
            throw ExportError(message: "Finish the current gesture before exporting")
        }
        await recordWaitingPluginEdits(in: captured)
        let result: Result<Void, any Error> = await edits.run {
            guard self.session === captured, captured.faderPreview == nil, captured.parameterGesture == nil, captured.pluginGestures.isEmpty,
                  let document = captured.document else {
                return .failure(ExportError(message: "The export document is no longer open"))
            }
            do {
                _ = try await document.history(.exportSnapshot(path: destination.path))
                return .success(())
            } catch { return .failure(error) }
        }
        try result.get()
    }
}
