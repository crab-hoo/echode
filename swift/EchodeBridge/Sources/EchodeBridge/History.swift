import Foundation

/// One autosave commit. `time` is the commit timestamp in Unix seconds.
public struct SavedRevision: Decodable, Sendable, Equatable, Identifiable {
    public let id: String
    public let time: Int64
}

public struct HistoryReport: Decodable, Sendable, Equatable {
    public let revisions: [SavedRevision]
    public let changed: Bool
}

public enum HistoryCommand: Encodable, Sendable {
    case list
    case restore(revision: String)

    enum CodingKeys: String, CodingKey { case type, revision }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .list:
            try container.encode("history.list", forKey: .type)
        case .restore(let revision):
            try container.encode("history.restore", forKey: .type)
            try container.encode(revision, forKey: .revision)
        }
    }
}
