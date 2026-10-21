import AppKit
import EchodeBridge

struct TrackDevice: Identifiable, Equatable, Sendable {
    let id: UUID
    let plugin: String
    var name: String? = nil
    var bypassed = false
    var offline = false
    /// CLAP parameter ids shown first in this device's list.
    var pinned: [UInt32] = []
    /// Presentation only: a confirmed window lifetime, even while hidden.
    var windowOpen = false
    var title: String { name ?? plugin }
}

extension Snapshot {
    func devices(on track: UUID) -> [TrackDevice] {
        guard let index = (0..<trackCount).first(where: { trackID(at: $0) == track }) else { return [] }
        return devices(onTrack: index)
    }

    func devices(onTrack index: Int) -> [TrackDevice] {
        return (0..<deviceCount(onTrack: index)).compactMap {
            guard let id = deviceID(onTrack: index, at: $0), let plugin = devicePlugin(onTrack: index, at: $0) else { return nil }
            return TrackDevice(id: id, plugin: plugin, bypassed: deviceBypassed(onTrack: index, at: $0) ?? false,
                               offline: deviceOffline(onTrack: index, at: $0) ?? false,
                               pinned: devicePins(onTrack: index, at: $0) ?? [])
        }
    }
}

enum DeviceChainEdit: Equatable {
    case reorder(device: UUID, before: UUID?)
    case copy(source: UUID, track: UUID, before: UUID?)
    case remove(UUID)
    case bypass(device: UUID, bypassed: Bool)
    case offline(device: UUID, offline: Bool)
}

public enum TrackFXDrop {
    case plugin(String)
    case copyDevice(UUID)
    case moveToEnd(UUID)
}

/// Payloads carry the document session as well as object identities. A stale
/// drag cannot address a newly opened copy of the same project.
struct FXDrag: Codable {
    static let deviceType = NSPasteboard.PasteboardType("io.echode.device")
    static let pluginType = NSPasteboard.PasteboardType("io.echode.plugin")
    let document: UUID
    let track: UUID?
    let device: UUID?
    let plugin: String?

    var type: NSPasteboard.PasteboardType { device == nil ? Self.pluginType : Self.deviceType }
    var data: Data? { try? JSONEncoder().encode(self) }

    static func read(_ pasteboard: NSPasteboard) -> Self? {
        for type in [deviceType, pluginType] {
            guard let data = pasteboard.data(forType: type), let payload = try? JSONDecoder().decode(Self.self, from: data) else { continue }
            if type == deviceType, payload.device != nil, payload.track != nil, payload.plugin == nil { return payload }
            if type == pluginType, payload.device == nil, payload.track == nil, payload.plugin?.isEmpty == false { return payload }
        }
        return nil
    }

    func operation(on target: UUID, copying: Bool, before: UUID?) -> DeviceChainEdit? {
        guard let device, let track else { return nil }
        return copying || track != target ? .copy(source: device, track: target, before: before) : .reorder(device: device, before: before)
    }

    func provider() -> NSItemProvider {
        let provider = NSItemProvider()
        if let data {
            provider.registerDataRepresentation(forTypeIdentifier: type.rawValue, visibility: .ownProcess) { completion in
                completion(data, nil); return nil
            }
        }
        return provider
    }
}

struct TrackFXLayout {
    let row: CGRect
    var visible: Bool { row.width >= 100 && row.height >= 24 }
    var rect: CGRect { CGRect(x: row.maxX - 31, y: row.minY + 4, width: 23, height: 18) }
}
