import Foundation

/// One autosave commit. `time` is the commit timestamp in Unix seconds.
public struct SavedRevision: Decodable, Sendable, Equatable, Identifiable {
    public let id: String
    public let time: Int64
}

/// Opening skipped an unreadable autosave. Consent is scoped to these exact revisions.
public struct RecoveryNotice: Decodable, Sendable, Equatable {
    public let tip: String
    public let base: String
    public let applied_seq: UInt64
}

public struct HistoryReport: Decodable, Sendable, Equatable {
    public let recovery: RecoveryNotice?
    public let revisions: [SavedRevision]
    public let changed: Bool
}

public enum HistoryCommand: Encodable, Sendable {
    case list
    case restore(revision: String)
    case acceptRecovery(RecoveryNotice)

    enum CodingKeys: String, CodingKey { case type, revision, tip, base }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .list:
            try container.encode("history.list", forKey: .type)
        case .acceptRecovery(let notice):
            try container.encode("history.accept_recovery", forKey: .type)
            try container.encode(notice.tip, forKey: .tip)
            try container.encode(notice.base, forKey: .base)
        case .restore(let revision):
            try container.encode("history.restore", forKey: .type)
            try container.encode(revision, forKey: .revision)
        }
    }
}
