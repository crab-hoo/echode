import Foundation

/// Devices are a visibility choice, not a resizable pane.
enum WorkspacePane: String, Sendable { case inspector, browser, editor, devices }

struct WorkspaceLayout: Codable, Equatable {
    static let deviceHeight = 300.0
    static let minimumEditorHeight = 260.0
    static let minimumArrangeHeight = 220.0
    static let divider = 5.0
    // Transport, status and their separators outside the pane area.
    static let windowChrome = 80.0

    var inspector = true
    var browser = true
    var editor = true
    var devices = false
    var editorsMaximized = false
    var inspectorWidth = 210.0
    var browserWidth = 220.0
    var editorHeight = 300.0

    var minimumWidth: Double { max(740, 520 + (inspector ? 185 : 0) + (browser ? 185 : 0)) }
    var minimumHeight: Double {
        let content = (editorsMaximized ? 0 : Self.minimumArrangeHeight)
            + (editor ? Self.minimumEditorHeight : 0) + (devices ? Self.deviceHeight : 0)
            + Double(separatorCount) * Self.divider
        return max(editor && !editorsMaximized ? 580 : 360, content + Self.windowChrome)
    }
    private var separatorCount: Int {
        max(0, (editorsMaximized ? 0 : 1) + (editor ? 1 : 0) + (devices ? 1 : 0) - 1)
    }
    var isValid: Bool { inspectorWidth.isFinite && browserWidth.isFinite && editorHeight.isFinite }

    func dimensions(in size: CGSize) -> WorkspaceDimensions {
        let width = size.width.isFinite ? max(0, size.width) : 0
        let height = size.height.isFinite ? max(0, size.height) : 0
        let sideBudget = max(0, width - 520 - (inspector ? 5 : 0) - (browser ? 5 : 0))
        let left = inspector ? min(320, max(180, inspectorWidth)) : 0
        let right = browser ? min(340, max(180, browserWidth)) : 0
        let count = (inspector ? 1.0 : 0) + (browser ? 1.0 : 0)
        let minimum = count > 0 ? min(180, sideBudget / count) : 0
        let extra = max(0, sideBudget - minimum * count)
        let desiredExtra = max(0, left - minimum) + max(0, right - minimum)
        let ratio = desiredExtra > 0 ? min(1, extra / desiredExtra) : 1
        let fittedLeft = inspector ? minimum + (left - minimum) * ratio : 0
        let fittedRight = browser ? minimum + (right - minimum) * ratio : 0

        // Reserve the device view first. Only a transient, sub-minimum host
        // smaller than the device itself can constrain its fixed height.
        let device = devices ? min(Self.deviceHeight, height) : 0
        let separator = separatorCount > 0 ? min(Self.divider, (height - device) / Double(separatorCount)) : 0
        let available = max(0, height - device - separator * Double(separatorCount))
        let piano = editor ? (editorsMaximized ? available :
            min(max(Self.minimumEditorHeight, editorHeight), max(0, available - Self.minimumArrangeHeight))) : 0
        let arrange = editorsMaximized ? 0 : max(0, available - piano)
        return WorkspaceDimensions(inspector: fittedLeft, browser: fittedRight, editor: piano, devices: device,
                                   arrange: arrange, separator: separator, spacer: max(0, available - piano - arrange))
    }

    mutating func resize(_ pane: WorkspacePane, to value: Double, in size: CGSize) {
        guard value.isFinite, size.width.isFinite, size.height.isFinite else { return }
        let actual = dimensions(in: size)
        switch pane {
        case .inspector: inspectorWidth = min(max(180, value), max(180, min(320, size.width - 530 - actual.browser)))
        case .browser: browserWidth = min(max(180, value), max(180, min(340, size.width - 530 - actual.inspector)))
        case .editor:
            guard !editorsMaximized else { return }
            let limit = size.height - actual.devices - Double(separatorCount) * actual.separator - Self.minimumArrangeHeight
            editorHeight = min(max(Self.minimumEditorHeight, value), max(Self.minimumEditorHeight, limit))
        case .devices: break
        }
    }

    mutating func toggle(_ pane: WorkspacePane) {
        switch pane {
        case .inspector: inspector.toggle()
        case .browser: browser.toggle()
        case .editor: editor.toggle()
        case .devices: devices.toggle()
        }
    }
}

extension WorkspaceLayout {
    private enum CodingKeys: String, CodingKey {
        case inspector, browser, editor, devices, editorsMaximized, inspectorWidth, browserWidth, editorHeight
    }

    /// v1 saved only the piano roll's visibility and the shared bottom height.
    /// Its transient editor/devices/both selection was never persisted.
    init(from decoder: any Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        inspector = try values.decode(Bool.self, forKey: .inspector)
        browser = try values.decode(Bool.self, forKey: .browser)
        editor = try values.decode(Bool.self, forKey: .editor)
        inspectorWidth = try values.decode(Double.self, forKey: .inspectorWidth)
        browserWidth = try values.decode(Double.self, forKey: .browserWidth)
        editorHeight = try values.decode(Double.self, forKey: .editorHeight)
        devices = try values.decodeIfPresent(Bool.self, forKey: .devices) ?? false
        editorsMaximized = try values.decodeIfPresent(Bool.self, forKey: .editorsMaximized) ?? false
    }
}

struct WorkspaceDimensions: Equatable {
    let inspector: Double
    let browser: Double
    let editor: Double
    let devices: Double
    let arrange: Double
    let separator: Double
    let spacer: Double
}
