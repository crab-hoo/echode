import CEchodeFfi
import Foundation

public struct TrackHierarchy: Sendable {
    public let parent: UUID?
    public let hasChildren: Bool
    public let depth: Int
}

extension Snapshot {
    public func trackHierarchy(at row: Int) -> TrackHierarchy? {
        guard row >= 0 else { return nil }
        var info = TrackTreeInfo()
        guard echode_snapshot_track_tree(handle, row, &info) == 0 else { return nil }
        return TrackHierarchy(
            parent: info.has_parent == 1 ? UUID(uuid: info.parent) : nil,
            hasChildren: info.has_children == 1, depth: info.depth)
    }

    /// A retained display view. Audio, the project and undo history are shared
    /// unchanged; track row access and arrange rectangles use the same mapping.
    public func arrangeProjection(collapsedTracks: Set<UUID>) -> Snapshot? {
        let bytes = collapsedTracks.flatMap { id in
            var uuid = id.uuid
            return withUnsafeBytes(of: &uuid) { Array($0) }
        }
        return bytes.withUnsafeBufferPointer { bytes in
            guard let projected = echode_snapshot_project_arrange(handle, bytes.baseAddress, collapsedTracks.count)
            else { return nil }
            return Snapshot(taking: projected)
        }
    }
}
