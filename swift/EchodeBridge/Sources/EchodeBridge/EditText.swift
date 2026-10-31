/// A string field's value, or one committed by an earlier action of the same transaction.
public enum EditText: Encodable, Sendable {
    case value(String)
    /// `action` is the zero-based position in the transaction; `field` names a committed string.
    case result(action: Int, field: String)

    private enum ResultKey: String, CodingKey { case ref, field }

    public func encode(to encoder: any Encoder) throws {
        switch self {
        case let .value(value):
            var container = encoder.singleValueContainer()
            try container.encode(value)
        case let .result(action, field):
            var container = encoder.container(keyedBy: ResultKey.self)
            try container.encode(action, forKey: .ref)
            try container.encode(field, forKey: .field)
        }
    }
}
