import AppKit
import EchodeBridge

/// Compact headers show the first device directly and keep every remaining
/// device reachable in a native menu. This is a viewport limit, not a chain limit.
struct TrackFXNamesLayout {
    let name: CGRect
    let overflow: CGRect
    init(row: CGRect, count: Int, depth: Int = 0) {
        let fx = TrackFXLayout(row: row)
        let indent = min(Double(depth) * 12, max(0, row.width - 76))
        let start = max(40, row.width * 0.45, indent + 32)
        let width = max(0, fx.rect.minX - start - 4)
        let shown = fx.visible && row.height >= 52 && count > 0 && width >= 36
        overflow = shown && count > 1 ? CGRect(x: fx.rect.minX - 22, y: fx.rect.minY, width: 18, height: 18) : .zero
        name = shown ? CGRect(x: start, y: fx.rect.minY, width: max(0, width - (count > 1 ? 22 : 0)), height: 18) : .zero
    }
    var titleEnd: Double? { name.isEmpty ? nil : name.minX - 4 }
}

extension ArrangeHeaderView {
    func openHeaderDevice(_ device: TrackDevice, track: UUID) {
        guard !device.offline else { return }
        onOpenPlugin?(track, device.id)
    }

    func deviceMenu(_ devices: [TrackDevice], track: UUID) -> NSMenu {
        let menu = NSMenu()
        for device in devices {
            let item = NSMenuItem(title: pluginNames[device.plugin] ?? device.plugin,
                                  action: #selector(openHeaderMenuDevice(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = HeaderDeviceTarget(document: documentID, track: track, device: device.id)
            item.isEnabled = !device.offline
            menu.addItem(item)
        }
        menu.autoenablesItems = false
        return menu
    }

    @objc private func openHeaderMenuDevice(_ sender: NSMenuItem) {
        guard let target = sender.representedObject as? HeaderDeviceTarget, target.document == documentID,
              let device = headerDevices(on: target.track).first(where: { $0.id == target.device }) else { return }
        openHeaderDevice(device, track: target.track)
    }
}

private struct HeaderDeviceTarget {
    let document: UUID?
    let track: UUID
    let device: UUID
}
