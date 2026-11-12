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
    public let appliedSeq: UInt64

    enum CodingKeys: String, CodingKey {
        case tip, base
        case appliedSeq = "applied_seq"
    }
}

public struct HistoryReport: Decodable, Sendable, Equatable {
    public let recovery: RecoveryNotice?
    public let revisions: [SavedRevision]
    public let changed: Bool
    /// Versioned GUI preferences; absent on unrelated history responses.
    public let exportSettings: String?

    enum CodingKeys: String, CodingKey {
        case recovery, revisions, changed
        case exportSettings = "export_settings"
    }
}

public enum HistoryCommand: Encodable, Sendable {
    case list
    case restore(revision: String)
    case acceptRecovery(RecoveryNotice)
    /// Freezes the recorded project and its objects into a new private package.
    case exportSnapshot(path: String)
    /// Reads preferences, or atomically replaces them on the document worker.
    case exportSettings(json: String? = nil)

    enum CodingKeys: String, CodingKey { case type, revision, tip, base, path, settings }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .exportSettings(let json):
            try container.encode("history.export_settings", forKey: .type)
            try container.encodeIfPresent(json, forKey: .settings)
        case .exportSnapshot(let path):
            try container.encode("history.export_snapshot", forKey: .type)
            try container.encode(path, forKey: .path)
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
