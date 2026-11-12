import Foundation
import Observation

/// Keep every keystroke until the whole active input is valid. Numeric SwiftUI
/// bindings otherwise retain an older value when the displayed text cannot parse.
@MainActor @Observable
final class ExportSettingsDraft {
    enum InputError: Error, Equatable {
        case sampleRate, tail, peak, dither

        var message: String {
            switch self {
            case .sampleRate: "Enter a sample rate from 1 to 4294967295 Hz (no grouping)."
            case .tail: "Enter a finite, nonnegative tail that fits in 4294967295 frames."
            case .peak: "Enter a finite peak target from -120 to 0 dBFS."
            case .dither: "TPDF dither requires an integer PCM format."
            }
        }
    }

    var options: ExportSettings
    var sampleRate: String
    var tailSeconds: String
    var peakDb: String

    init(settings: ExportSettings, locale: Locale = .current) {
        options = settings
        sampleRate = String(settings.sampleRate)
        let separator = locale.decimalSeparator ?? "."
        tailSeconds = String(settings.tailSeconds).replacingOccurrences(of: ".", with: separator)
        peakDb = String(settings.normalizePeakDb ?? -1).replacingOccurrences(of: ".", with: separator)
    }

    func reset(settings: ExportSettings, locale: Locale = .current) {
        let loaded = ExportSettingsDraft(settings: settings, locale: locale)
        options = loaded.options
        sampleRate = loaded.sampleRate
        tailSeconds = loaded.tailSeconds
        peakDb = loaded.peakDb
    }

    func resolve(locale: Locale = .current) -> Result<ExportSettings, InputError> {
        let rateText = sampleRate.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !rateText.isEmpty, rateText.allSatisfy({ $0.isASCII && $0.isNumber }),
            let rate = UInt32(rateText), rate > 0
        else { return .failure(.sampleRate) }
        var result = options
        result.sampleRate = rate
        if !options.automaticTail {
            guard let tail = Self.number(tailSeconds, locale: locale), tail >= 0,
                tail * Double(rate) < Double(UInt32.max) + 0.5
            else { return .failure(.tail) }
            result.tailSeconds = tail
        }
        if options.normalizePeakDb != nil {
            guard let peak = Self.number(peakDb, locale: locale), (-120...0).contains(peak) else {
                return .failure(.peak)
            }
            result.normalizePeakDb = peak
        }
        guard result.isValid else { return .failure(.dither) }
        return .success(result)
    }

    private static func number(_ text: String, locale: Locale) -> Double? {
        let separator = locale.decimalSeparator ?? "."
        let raw = text.trimmingCharacters(in: .whitespacesAndNewlines)
        // Do not accept a numeric prefix, grouping, non-finite values, or an
        // ambiguous decimal separator from a different locale.
        let decimal = NSRegularExpression.escapedPattern(for: separator)
        let pattern = "^[+-]?([0-9]+(\(decimal)[0-9]*)?|\(decimal)[0-9]+)([eE][+-]?[0-9]+)?$"
        guard raw.range(of: pattern, options: .regularExpression) != nil,
            let value = Double(raw.replacingOccurrences(of: separator, with: ".")), value.isFinite
        else { return nil }
        return value
    }
}
