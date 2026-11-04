import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct ExportSettingsDraftTests {
    private let locale = Locale(identifier: "en_US_POSIX")

    @Test(arguments: ["", " ", "0", "-1", "1.5", "48,000", "48000 Hz", "4294967296", "1e3", "NaN", "+48000"])
    func ratesMustBeWholePositiveUInt32Values(input: String) {
        let draft = ExportSettingsDraft(settings: ExportSettings(), locale: locale)
        draft.sampleRate = input
        #expect(throws: ExportSettingsDraft.InputError.sampleRate) { try draft.resolve(locale: locale).get() }
    }

    @Test(arguments: ["", "seconds", "NaN", "inf", "-0.1", "2 sec", "1,000.5", "1e999", "4294967295"])
    func activeTailRejectsInvalidOrOverflowingValues(input: String) {
        var settings = ExportSettings(); settings.automaticTail = false
        let draft = ExportSettingsDraft(settings: settings, locale: locale)
        draft.tailSeconds = input
        #expect(throws: ExportSettingsDraft.InputError.tail) { try draft.resolve(locale: locale).get() }
    }

    @Test(arguments: ["", "NaN", "infinity", "1", "-120.00001", "-3 dB", "-3.5x", "1e309"])
    func activePeakRequiresAFiniteValueInRange(input: String) {
        var settings = ExportSettings(); settings.normalizePeakDb = -1
        let draft = ExportSettingsDraft(settings: settings, locale: locale)
        draft.peakDb = input
        #expect(throws: ExportSettingsDraft.InputError.peak) { try draft.resolve(locale: locale).get() }
    }

    @Test(arguments: ["en_US_POSIX", "de_DE", "fr_FR"])
    func localeDecimalsRoundTripWithoutGroupingOrPrecisionLoss(identifier: String) throws {
        let locale = Locale(identifier: identifier)
        var settings = ExportSettings(); settings.automaticTail = false
        settings.sampleRate = 96_000; settings.tailSeconds = 0.12345678912345678
        settings.normalizePeakDb = -3.987654321987654
        let draft = ExportSettingsDraft(settings: settings, locale: locale)
        #expect(try draft.resolve(locale: locale).get() == settings)
        let decimal = locale.decimalSeparator ?? "."
        draft.tailSeconds = " 1\(decimal)25e-1 "
        draft.peakDb = " -1\(decimal)5 "
        let parsed = try draft.resolve(locale: locale).get()
        #expect(parsed.tailSeconds == 0.125 && parsed.normalizePeakDb == -1.5)
        #expect(parsed.arguments?.contains("12000") == true)
        draft.tailSeconds = decimal == "." ? "0,125" : "0.125"
        #expect(throws: ExportSettingsDraft.InputError.tail) { try draft.resolve(locale: locale).get() }
    }

    @Test func disabledInputsDoNotBlockExportButAreRevalidatedWhenEnabled() throws {
        let draft = ExportSettingsDraft(settings: ExportSettings(), locale: locale)
        draft.tailSeconds = "unfinished tail"; draft.peakDb = "unfinished peak"
        #expect(try draft.resolve(locale: locale).get() == ExportSettings())
        draft.options.automaticTail = false
        #expect(throws: ExportSettingsDraft.InputError.tail) { try draft.resolve(locale: locale).get() }
        draft.tailSeconds = "0"
        draft.options.normalizePeakDb = -1
        #expect(throws: ExportSettingsDraft.InputError.peak) { try draft.resolve(locale: locale).get() }
        draft.peakDb = "-120"
        #expect(try draft.resolve(locale: locale).get().normalizePeakDb == -120)
        draft.options.format = .float32; draft.options.tpdf = true
        #expect(throws: ExportSettingsDraft.InputError.dither) { try draft.resolve(locale: locale).get() }
    }

    @Test func frameRoundingAndRateBoundariesRemainSafe() throws {
        var settings = ExportSettings(); settings.automaticTail = false
        let draft = ExportSettingsDraft(settings: settings, locale: locale)
        draft.sampleRate = "1"; draft.tailSeconds = "4294967295.49"
        #expect(try draft.resolve(locale: locale).get().arguments?.contains("4294967295") == true)
        draft.tailSeconds = "4294967295.5"
        #expect(throws: ExportSettingsDraft.InputError.tail) { try draft.resolve(locale: locale).get() }
        draft.tailSeconds = "0"; draft.sampleRate = " 4294967295 "
        #expect(try draft.resolve(locale: locale).get().sampleRate == UInt32.max)
        draft.options.automaticTail = true; draft.options.tailSeconds = 2
        #expect(try draft.resolve(locale: locale).get().arguments?.contains("--tail-frames") == false)
        draft.options.automaticTail = false; draft.tailSeconds = "2"
        #expect(throws: ExportSettingsDraft.InputError.tail) { try draft.resolve(locale: locale).get() }
        draft.peakDb = "obsolete draft"
        draft.reset(settings: settings, locale: locale)
        #expect(try draft.resolve(locale: locale).get() == settings)
        #expect(draft.peakDb == "-1.0")
    }
}
