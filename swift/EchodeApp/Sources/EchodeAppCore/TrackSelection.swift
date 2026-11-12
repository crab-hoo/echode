import Foundation

public enum TrackSelectionChange {
    case replace, toggle, extend, focus
}

/// Selection is transient UI state. The focus remains a single track for
/// note input and ordinary plugin insertion; batch FX uses the selected IDs.
struct TrackSelection {
    private(set) var ids: Set<UUID> = []
    private(set) var focus: UUID?
    private var anchor: UUID?

    mutating func replace(_ id: UUID?) {
        ids = Set(id.map { [$0] } ?? [])
        focus = id
        anchor = id
    }

    mutating func select(_ id: UUID, change: TrackSelectionChange, order: [UUID]) {
        guard let index = order.firstIndex(of: id) else { return }
        switch change {
        case .replace: replace(id)
        case .focus:
            if ids.contains(id) { focus = id } else { replace(id) }
        case .toggle:
            if ids.remove(id) != nil {
                if focus == id { focus = order.last(where: { ids.contains($0) }) }
            } else {
                ids.insert(id)
                focus = id
            }
            anchor = focus
        case .extend:
            guard let anchor, let start = order.firstIndex(of: anchor) else {
                replace(id)
                return
            }
            ids = Set(order[min(start, index)...max(start, index)])
            focus = id
        }
    }

    mutating func reconcile(order: [UUID]) {
        ids.formIntersection(order)
        if !order.contains(where: { $0 == focus }) { focus = order.first(where: { ids.contains($0) }) ?? order.first }
        if let focus { ids.insert(focus) }
        if !order.contains(where: { $0 == anchor }) { anchor = focus }
    }
}
