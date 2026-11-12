import Foundation

struct ExportProgress: Decodable, Sendable, Equatable {
    enum Phase: String, Decodable, Sendable {
        case rendering, finalizing, complete
        var rank: Int {
            switch self {
            case .rendering: 0
            case .finalizing: 1
            case .complete: 2
            }
        }
    }
    let version: UInt32
    let phase: Phase
    let renderedFrames: UInt32
    let totalFrames: UInt32
    enum CodingKeys: String, CodingKey {
        case version, phase
        case renderedFrames = "rendered_frames"
        case totalFrames = "total_frames"
    }

    // Only the process's successful exit may advance the UI to 100%.
    var fraction: Double {
        phase == .rendering ? min(0.99, Double(renderedFrames) / Double(max(1, totalFrames))) : 0.99
    }
}

/// Bounded framing for an advisory stream, independent of stderr diagnostics.
/// A malformed/unknown/partial record must never manufacture success or grow memory.
struct ExportProgressDecoder: Sendable {
    private var line = Data()
    private var dropping = false
    private(set) var latest: ExportProgress?

    mutating func append(_ data: Data) -> [ExportProgress] {
        var updates: [ExportProgress] = []
        for byte in data {
            if byte == 10 {
                if !dropping, let value = try? JSONDecoder().decode(ExportProgress.self, from: line),
                    accepts(value)
                {
                    latest = value
                    updates.append(value)
                }
                line.removeAll(keepingCapacity: true)
                dropping = false
            } else if !dropping {
                // 4 KiB bounds one protocol record, not the export or its diagnostics.
                if line.count < 4096 {
                    line.append(byte)
                } else {
                    line.removeAll(keepingCapacity: true)
                    dropping = true
                }
            }
        }
        return updates
    }

    private func accepts(_ value: ExportProgress) -> Bool {
        guard value.version == 1, value.renderedFrames <= value.totalFrames,
            value.phase == .rendering || value.renderedFrames == value.totalFrames
        else { return false }
        guard let latest else { return true }
        return value.totalFrames == latest.totalFrames && value.renderedFrames >= latest.renderedFrames
            && value.phase.rank >= latest.phase.rank && value != latest
    }
}
