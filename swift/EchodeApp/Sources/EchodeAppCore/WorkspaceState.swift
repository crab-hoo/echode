import EchodeBridge
import Foundation
import Observation

enum WorkspacePane: String, Sendable { case inspector, browser, editor }

struct WorkspaceLayout: Codable, Equatable {
    var inspector = true
    var browser = true
    var editor = true
    var inspectorWidth = 210.0
    var browserWidth = 220.0
    var editorHeight = 300.0

    var minimumWidth: Double { max(740, 520 + (inspector ? 185 : 0) + (browser ? 185 : 0)) }
    var minimumHeight: Double { editor ? 580 : 360 }

    func dimensions(in size: CGSize) -> WorkspaceDimensions {
        let sideBudget = max(0, size.width - 520 - (inspector ? 5 : 0) - (browser ? 5 : 0))
        let left = inspector ? min(320, max(180, inspectorWidth)) : 0
        let right = browser ? min(340, max(180, browserWidth)) : 0
        let count = (inspector ? 1.0 : 0) + (browser ? 1.0 : 0)
        let minimum = count > 0 ? min(180, sideBudget / count) : 0
        let extra = max(0, sideBudget - minimum * count)
        let desiredExtra = max(0, left - minimum) + max(0, right - minimum)
        let ratio = desiredExtra > 0 ? min(1, extra / desiredExtra) : 1
        let fittedLeft = inspector ? minimum + (left - minimum) * ratio : 0
        let fittedRight = browser ? minimum + (right - minimum) * ratio : 0
        let bottom = editor ? min(max(260, editorHeight), max(0, size.height - 225)) : 0
        return WorkspaceDimensions(inspector: fittedLeft, browser: fittedRight,
                                   editor: bottom, arrange: max(0, size.height - bottom - (editor ? 5 : 0)))
    }

    mutating func resize(_ pane: WorkspacePane, to value: Double, in size: CGSize) {
        guard value.isFinite else { return }
        let actual = dimensions(in: size)
        switch pane {
        case .inspector: inspectorWidth = min(max(180, value), max(180, min(320, size.width - 530 - actual.browser)))
        case .browser: browserWidth = min(max(180, value), max(180, min(340, size.width - 530 - actual.inspector)))
        case .editor: editorHeight = min(max(260, value), max(260, size.height - 225))
        }
    }

    mutating func toggle(_ pane: WorkspacePane) {
        switch pane { case .inspector: inspector.toggle(); case .browser: browser.toggle(); case .editor: editor.toggle() }
    }
}

struct WorkspaceDimensions: Equatable { let inspector: Double; let browser: Double; let editor: Double; let arrange: Double }

enum WorkspacePresentation: Identifiable, Equatable {
    case palette, plugins(track: UUID, document: UUID), output
    var id: String {
        switch self { case .palette: "palette"; case .plugins(let track, let document): "plugins/\(document)/\(track)"; case .output: "output" }
    }
}

/// Window preferences and transient UI state never enter the project journal.
@MainActor @Observable
public final class WorkspaceState {
    var layout: WorkspaceLayout
    var displayKey = DisplayKey()
    var presentation: WorkspacePresentation?
    var choosingFile = false
    var draft = NoteDraft()
    let browser = FileBrowserState()
    @ObservationIgnored let navigation = CanvasNavigationState()
    @ObservationIgnored private let defaults: UserDefaults
    private static let layoutKey = "workspace.layout.v1"

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        layout = defaults.data(forKey: Self.layoutKey).flatMap { try? JSONDecoder().decode(WorkspaceLayout.self, from: $0) } ?? WorkspaceLayout()
        // Invalid saved values cannot affect frame sizes or native controls.
        if !layout.inspectorWidth.isFinite || !layout.browserWidth.isFinite || !layout.editorHeight.isFinite { layout = WorkspaceLayout() }
    }

    func toggle(_ pane: WorkspacePane) { layout.toggle(pane); saveLayout() }
    func saveLayout() { if let data = try? JSONEncoder().encode(layout) { defaults.set(data, forKey: Self.layoutKey) } }

    func synchronize(_ controller: ProjectController) {
        if case .plugins(_, let document) = presentation, document != controller.documentID { presentation = nil }
        navigation.prepare(document: controller.documentID, tracks: Set(controller.tracks.map(\.id)))
        draft.synchronize(document: controller.documentID, track: controller.selection,
                          note: controller.noteFocus.flatMap { controller.snapshot?.pianoNote($0) })
    }
}

/// Viewport memory is not observed by SwiftUI on every scroll event. Native
/// views own live gestures; this store survives hiding an editor or sidebar.
@MainActor
public final class CanvasNavigationState {
    var arrange: ArrangeCamera?
    var headerWidth = 160.0
    var pianos: [UUID: PianoViewportMemory] = [:]
    private(set) var document: UUID?
    private var tracks: Set<UUID> = []
    public init() {}

    func permits(document: UUID?, track: UUID? = nil) -> Bool {
        self.document == document && (track.map { tracks.contains($0) } ?? true)
    }

    func prepare(document: UUID?, tracks: Set<UUID>) {
        if self.document != document { self.document = document; arrange = nil; headerWidth = 160; pianos = [:] }
        self.tracks = tracks
        pianos = pianos.filter { tracks.contains($0.key) }
    }
}

struct PianoViewportMemory {
    let camera: PianoCamera
    let focus: PianoNote.ID?
    let geometry: NoteGeometry?
}
