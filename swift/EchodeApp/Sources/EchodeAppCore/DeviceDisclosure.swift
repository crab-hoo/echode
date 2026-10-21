import Foundation

extension ProjectController {
    /// Opening and closing a column changes no project data or plugin lifetime.
    /// Keep identities through Undo, but reject callbacks from a closed document.
    @discardableResult
    func setDeviceCollapsed(_ device: UUID, collapsed: Bool, documentID: UUID) -> Bool {
        guard self.documentID == documentID,
              tracks.contains(where: { deviceRows(on: $0.id).contains(where: { $0.id == device }) }) else { return false }
        if collapsed { collapsedDevices.insert(device) } else { collapsedDevices.remove(device) }
        return true
    }
}
