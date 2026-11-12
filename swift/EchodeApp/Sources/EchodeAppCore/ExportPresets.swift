import Foundation

/// Named choices are local preferences, not document state or automatic defaults.
struct ExportPresets {
    static let storageKey = "workspace.export.presets.v1"

    enum Failure: Error, Equatable {
        case unreadable, invalidName, invalidSettings, duplicateName, missing

        var message: String {
            switch self {
            case .unreadable: "Export presets cannot be read. Existing presets have not been changed."
            case .invalidName: "Enter a nonempty preset name without control characters."
            case .invalidSettings: "Correct the export settings before saving a preset."
            case .duplicateName:
                "That preset name already exists. Choose another name or delete the existing preset first."
            case .missing: "That export preset no longer exists."
            }
        }
    }

    private struct Stored: Codable {
        var version = 1
        var entries: [String: ExportSettings]
    }

    static func normalizedName(_ name: String) -> String? {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines).precomposedStringWithCanonicalMapping
        guard !trimmed.isEmpty, !trimmed.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
        else { return nil }
        return trimmed
    }

    static func load(defaults: UserDefaults = .standard) throws -> [String: ExportSettings] {
        guard defaults.object(forKey: storageKey) != nil else { return [:] }
        guard let json = defaults.string(forKey: storageKey),
            let stored = try? JSONDecoder().decode(Stored.self, from: Data(json.utf8)), stored.version == 1,
            stored.entries.allSatisfy({ normalizedName($0.key) == $0.key && $0.value.isValid })
        else { throw Failure.unreadable }
        return stored.entries
    }

    @discardableResult
    static func save(name: String, settings: ExportSettings, defaults: UserDefaults = .standard) throws -> String {
        guard let name = normalizedName(name) else { throw Failure.invalidName }
        guard settings.isValid else { throw Failure.invalidSettings }
        var entries = try load(defaults: defaults)
        guard entries[name] == nil else { throw Failure.duplicateName }
        entries[name] = settings
        try write(entries, defaults: defaults)
        return name
    }

    static func delete(name: String, defaults: UserDefaults = .standard) throws {
        var entries = try load(defaults: defaults)
        guard entries.removeValue(forKey: name) != nil else { throw Failure.missing }
        try write(entries, defaults: defaults)
    }

    private static func write(_ entries: [String: ExportSettings], defaults: UserDefaults) throws {
        let encoder = JSONEncoder()
        encoder.outputFormatting = .sortedKeys
        let data = try encoder.encode(Stored(entries: entries))
        defaults.set(String(decoding: data, as: UTF8.self), forKey: storageKey)
    }
}
