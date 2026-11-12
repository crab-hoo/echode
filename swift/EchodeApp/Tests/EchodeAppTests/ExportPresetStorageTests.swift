import Foundation
import Testing

@testable import EchodeAppCore

struct ExportPresetStorageTests {
    @Test func namesAndValidatedSettingsRoundTripWithoutImplicitReplacement() throws {
        let suite = "export-preset-storage-\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        #expect(try ExportPresets.load(defaults: defaults).isEmpty)
        var settings = ExportSettings()
        settings.format = .pcm16
        settings.sampleRate = 44_100
        settings.tpdf = true
        settings.automaticTail = false
        settings.tailSeconds = 0.125
        settings.normalizePeakDb = -3.5
        #expect(try ExportPresets.save(name: "  Cafe\u{301}  ", settings: settings, defaults: defaults) == "Café")
        #expect(try ExportPresets.load(defaults: defaults) == ["Café": settings])
        let saved = defaults.string(forKey: ExportPresets.storageKey)
        #expect(throws: ExportPresets.Failure.duplicateName) {
            try ExportPresets.save(name: "Café", settings: ExportSettings(), defaults: defaults)
        }
        for name in ["", " \n\t ", "A\nB", "A\u{0}B"] {
            #expect(throws: ExportPresets.Failure.invalidName) {
                try ExportPresets.save(name: name, settings: settings, defaults: defaults)
            }
        }
        for value in [Double.nan, .infinity, -1] {
            var invalid = settings
            invalid.tailSeconds = value
            #expect(throws: ExportPresets.Failure.invalidSettings) {
                try ExportPresets.save(name: "Invalid", settings: invalid, defaults: defaults)
            }
        }
        #expect(throws: ExportPresets.Failure.missing) { try ExportPresets.delete(name: "Missing", defaults: defaults) }
        #expect(defaults.string(forKey: ExportPresets.storageKey) == saved)
        // A later command reloads the store instead of overwriting another sheet's additions.
        try ExportPresets.save(name: "Archive", settings: ExportSettings(), defaults: defaults)
        #expect(try ExportPresets.load(defaults: defaults).count == 2)
        try ExportPresets.delete(name: "Café", defaults: defaults)
        #expect(try ExportPresets.load(defaults: defaults) == ["Archive": ExportSettings()])
        try ExportPresets.delete(name: "Archive", defaults: defaults)
        #expect(try ExportPresets.load(defaults: defaults).isEmpty)
    }

    @Test func malformedOrFutureStoresAreNotSilentlyOverwritten() throws {
        let suite = "export-preset-invalid-\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        try ExportPresets.save(name: "Mix", settings: ExportSettings(), defaults: defaults)
        let valid = try #require(defaults.string(forKey: ExportPresets.storageKey))
        let invalidStores = [
            "not JSON", "[]", valid.replacingOccurrences(of: "\"version\":1", with: "\"version\":2"),
            valid.replacingOccurrences(of: "48000", with: "0"),
            valid.replacingOccurrences(of: "\"Mix\"", with: "\" \""),
        ]
        for raw in invalidStores {
            defaults.set(raw, forKey: ExportPresets.storageKey)
            #expect(throws: ExportPresets.Failure.unreadable) { try ExportPresets.load(defaults: defaults) }
            #expect(throws: ExportPresets.Failure.unreadable) {
                try ExportPresets.save(name: "New", settings: ExportSettings(), defaults: defaults)
            }
            #expect(throws: ExportPresets.Failure.unreadable) {
                try ExportPresets.delete(name: "Mix", defaults: defaults)
            }
            #expect(defaults.string(forKey: ExportPresets.storageKey) == raw)
        }
        defaults.set(Data([0]), forKey: ExportPresets.storageKey)
        #expect(throws: ExportPresets.Failure.unreadable) { try ExportPresets.load(defaults: defaults) }
        #expect(defaults.data(forKey: ExportPresets.storageKey) == Data([0]))
    }
}
