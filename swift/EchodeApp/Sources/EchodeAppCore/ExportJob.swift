import Darwin
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
    private(set) var cancelRequested = false
    private(set) var fractionCompleted: Double?
    private(set) var message: String?
    private(set) var detail = ""
    @ObservationIgnored var executable: URL? = Bundle.main.executableURL?.deletingLastPathComponent().appendingPathComponent("echode-cli")

    func cancel() {
        guard running, !cancelRequested else { return }
        cancelRequested = true
        message = text("Cancelling export")
    }

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
        fractionCompleted = nil
        cancelRequested = false
        let executable = self.executable
        return Task {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-\(UUID())")
            var ownsRoot = false
            do {
                if cancelRequested { throw CancellationError() }
                guard let executable, FileManager.default.isExecutableFile(atPath: executable.path) else {
                    throw ExportError(message: "Export CLI is missing from the app bundle")
                }
                try await Task.detached {
                    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
                }.value
                ownsRoot = true
                if cancelRequested { throw CancellationError() }
                let frozen = root.appendingPathComponent("Snapshot.echode")
                let settingsWarning = try await controller.freezeExport(to: frozen, documentID: document, settings: settings)
                if cancelRequested { throw CancellationError() }
                settings.save(project: project, defaults: defaults)
                message = text("Exporting audio")
                let result = try await Task.detached {
                    try await Self.render(executable: executable, package: frozen, output: output, arguments: arguments, work: root,
                                          isCancelled: { self.cancelRequested }) { progress in
                        guard !self.cancelRequested else { return }
                        self.fractionCompleted = progress.fraction
                        self.message = text(progress.phase == .rendering ? "Exporting audio" : "Finalizing export")
                    }
                }.value
                if result.success { fractionCompleted = 1 }
                detail = "Source: \(project.path)\nOutput: \(output.path)\nOptions: \(arguments.joined(separator: " "))\n" + result.detail + (settingsWarning.map { "\n" + $0 } ?? "")
                let status = result.success ? "Export complete" : (result.cancelled ? "Export cancelled" : "Export failed")
                message = text(status) + ": " + output.lastPathComponent
            } catch {
                detail = (error as? BridgeError)?.message ?? error.localizedDescription
                message = text(error is CancellationError ? "Export cancelled" : "Export failed") + ": " + output.lastPathComponent
            }
            if ownsRoot {
                let cleanup = await Task.detached { Result { try FileManager.default.removeItem(at: root) } }.value
                if case .failure(let error) = cleanup { detail += "\nTemporary snapshot cleanup failed: \(error.localizedDescription)" }
            }
            running = false
        }
    }

    nonisolated static func render(executable: URL, package: URL, output: URL, arguments: [String], work: URL,
                                   isCancelled: @MainActor @Sendable () -> Bool = { false },
                                   onProgress: @MainActor @Sendable (ExportProgress) async -> Void = { _ in }) async throws -> (success: Bool, cancelled: Bool, detail: String) {
        let log = work.appendingPathComponent("render.log")
        guard FileManager.default.createFile(atPath: log.path, contents: nil) else {
            throw ExportError(message: "Could not create export log")
        }
        let handle = try FileHandle(forWritingTo: log)
        defer { try? handle.close() }
        let process = Process()
        process.executableURL = executable
        process.arguments = ["render", package.path, output.path] + arguments + ["--progress-json", "--cancel-stdin"]
        let controlPipe = Pipe()
        process.standardInput = controlPipe
        defer {
            try? controlPipe.fileHandleForReading.close()
            try? controlPipe.fileHandleForWriting.close()
        }
        let progressPipe = Pipe()
        process.standardOutput = progressPipe
        // A file rather than a pipe: even a verbose failing helper cannot fill
        // a pipe while the parent waits for termination off the main actor.
        process.standardError = handle
        try process.run()
        try? controlPipe.fileHandleForReading.close()
        // Drain progress while the child runs, before waiting for exit. POSIX read
        // returns available bytes without waiting to fill a whole 4 KiB record.
        try? progressPipe.fileHandleForWriting.close()
        let readerHandle = progressPipe.fileHandleForReading
        defer { try? readerHandle.close() }
        var decoder = ExportProgressDecoder()
        var bytes = [UInt8](repeating: 0, count: 4096)
        var progressError = ""
        var descriptor = pollfd(fd: readerHandle.fileDescriptor, events: Int16(POLLIN), revents: 0)
        var cancellationSent = false
        // Bound only post-exit observer data, never audio or project size. A
        // descendant that keeps writing stdout cannot hold an already-finished job.
        var exitDrainRemaining = 65_536
        while true {
            if !cancellationSent, await isCancelled() {
                // EOF is a cooperative cancellation request. The CLI still owns
                // cleanup, helper shutdown, and the atomic publication boundary.
                try? controlPipe.fileHandleForWriting.close()
                cancellationSent = true
            }
            // A plugin descendant may inherit stdout. Its open descriptor must
            // not hold the GUI job after the CLI itself has already exited.
            let processRunning = process.isRunning
            if !processRunning && exitDrainRemaining == 0 { break }
            let ready = Darwin.poll(&descriptor, 1, processRunning ? 100 : 0)
            if ready < 0 && errno == EINTR { continue }
            if ready < 0 {
                progressError = "\nProgress stream poll failed: errno \(errno)"
                descriptor.fd = -1
                continue
            }
            if ready == 0 {
                if !processRunning { break }
                continue
            }
            let count = Darwin.read(readerHandle.fileDescriptor, &bytes, bytes.count)
            if count < 0 && errno == EINTR { continue }
            if count <= 0 {
                if !process.isRunning { break }
                if count < 0 { progressError = "\nProgress stream read failed: errno \(errno)" }
                // Continue polling cancellation even if progress closes early.
                descriptor.fd = -1
                continue
            }
            if !processRunning { exitDrainRemaining = max(0, exitDrainRemaining - count) }
            for update in decoder.append(Data(bytes.prefix(count))) { await onProgress(update) }
            // Drain buffered records even if the CLI exited during the UI update.
            // Once exited, a zero-time poll avoids waiting on inherited stdout.
        }
        try? readerHandle.close()
        // Every exit above has observed termination. Do not re-enter Foundation's
        // run-loop wait after this async task may have resumed on another thread.
        let reader = try FileHandle(forReadingFrom: log)
        defer { try? reader.close() }
        let data = try reader.read(upToCount: 65_536) ?? Data()
        return (process.terminationReason == .exit && process.terminationStatus == 0,
                process.terminationReason == .exit && process.terminationStatus == 130,
                "Exit: \(process.terminationStatus) (\(process.terminationReason))\n" + String(decoding: data, as: UTF8.self) + progressError)
    }
}

extension ProjectController {
    @discardableResult
    func freezeExport(to destination: URL, documentID: DocumentID, settings: ExportSettings? = nil) async throws -> String? {
        guard let captured = inputSession(for: documentID), captured.faderPreview == nil,
              captured.parameterGesture == nil, captured.pluginGestures.isEmpty else {
            throw ExportError(message: "Finish the current gesture before exporting")
        }
        guard settings == nil || settings?.storedJSON != nil else { throw ExportError(message: "Invalid export settings") }
        await recordWaitingPluginEdits(in: captured)
        let result: Result<String?, any Error> = await edits.run {
            guard self.session === captured, captured.faderPreview == nil, captured.parameterGesture == nil, captured.pluginGestures.isEmpty,
                  let document = captured.document else {
                return .failure(ExportError(message: "The export document is no longer open"))
            }
            do {
                _ = try await document.history(.exportSnapshot(path: destination.path))
                if let settings {
                    guard let json = settings.storedJSON else { throw ExportError(message: "Invalid export settings") }
                    do { _ = try await document.history(.exportSettings(json: json)) }
                    catch { return .success("Export settings were not saved in the package: \(error.localizedDescription)") }
                }
                return .success(nil)
            } catch { return .failure(error) }
        }
        return try result.get()
    }
}
