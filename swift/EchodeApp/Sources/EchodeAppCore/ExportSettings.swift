import EchodeBridge
import Foundation

struct ExportSettings: Codable, Sendable, Equatable {
    enum Format: String, Codable, CaseIterable {
        case pcm24, pcm16, float32
        var title: String {
            switch self {
            case .pcm24: "WAV · 24-bit PCM"
            case .pcm16: "WAV · 16-bit PCM"
            case .float32: "WAV · 32-bit Float"
            }
        }
    }
    var format: Format = .pcm24
    var sampleRate: UInt32 = 48_000
    var automaticTail = true
    var tailSeconds = 2.0
    var tpdf = false
    var normalizePeakDb: Double? = nil

    var isValid: Bool {
        sampleRate > 0 && tailSeconds.isFinite && tailSeconds >= 0 &&
        tailSeconds * Double(sampleRate) < Double(UInt32.max) + 0.5 &&
        !(format == .float32 && tpdf) &&
        (normalizePeakDb.map { $0.isFinite && (-120...0).contains($0) } ?? true)
    }
    var arguments: [String]? {
        guard isValid else { return nil }
        var result = ["--format", format.rawValue, "--sample-rate", String(sampleRate), "--dither", tpdf ? "tpdf" : "none"]
        if !automaticTail { result += ["--tail-frames", String(UInt32((tailSeconds * Double(sampleRate)).rounded()))] }
        if let target = normalizePeakDb { result += ["--normalize-peak-db", String(target)] }
        return result
    }

    private struct Stored: Codable {
        var version = 1
        let settings: ExportSettings
    }
    var storedJSON: String? {
        let encoder = JSONEncoder(); encoder.outputFormatting = .sortedKeys
        guard isValid, let data = try? encoder.encode(Stored(settings: self)) else { return nil }
        return String(decoding: data, as: UTF8.self)
    }
    static func decodeStored(_ json: String?) -> Self? {
        guard let json, let stored = try? JSONDecoder().decode(Stored.self, from: Data(json.utf8)),
              stored.version == 1, stored.settings.isValid else { return nil }
        return stored.settings
    }

    private static func key(_ project: URL) -> String {
        "workspace.export.v1." + project.resolvingSymlinksInPath().path
    }
    static func load(project: URL, defaults: UserDefaults = .standard, initialRate: UInt32 = 48_000) -> Self {
        if let data = defaults.data(forKey: key(project)),
           let settings = try? JSONDecoder().decode(Self.self, from: data), settings.isValid { return settings }
        var settings = Self()
        settings.sampleRate = initialRate > 0 ? initialRate : 48_000
        return settings
    }
    func save(project: URL, defaults: UserDefaults = .standard) {
        guard isValid, let data = try? JSONEncoder().encode(self) else { return }
        defaults.set(data, forKey: Self.key(project))
    }
}

@MainActor
extension ProjectController {
    /// Read on the owning worker; a late result cannot seed another document's sheet.
    func loadExportSettings(documentID: DocumentID, defaults: UserDefaults = .standard) async -> ExportSettings? {
        guard let captured = inputSession(for: documentID), let document = captured.document, let project = projectURL else { return nil }
        let fallback = ExportSettings.load(project: project, defaults: defaults, initialRate: audio.activeDevice?.sampleRate ?? 48_000)
        let snapshot = try? await document.history(.exportSettings())
        guard inputSession(for: documentID) === captured else { return nil }
        let report = try? snapshot?.historyReport()
        return ExportSettings.decodeStored(report?.export_settings) ?? fallback
    }
}
