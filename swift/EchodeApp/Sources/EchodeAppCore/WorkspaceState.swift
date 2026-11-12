import EchodeBridge
import Foundation
import Observation

enum WorkspacePresentation: Identifiable, Equatable {
    case palette, output, history, exportAudio
    var id: String {
        switch self {
        case .palette: "palette"
        case .output: "output"
        case .history: "history"
        case .exportAudio: "exportAudio"
        }
    }
}

/// Window preferences and transient UI state never enter the project journal.
@MainActor @Observable
public final class WorkspaceState {
    var layout: WorkspaceLayout
    var displayKey = DisplayKey()
    var presentation: WorkspacePresentation?
    let exportJob = ExportJob()
    var pluginBrowserDocument: DocumentID?
    var showsDevices: Bool { layout.devices }
    var showsEditor: Bool { layout.editor }

    /// Candidate entry points reveal devices, without closing the piano roll.
    func showDevices() {
        layout.devices = true
        saveLayout()
    }
    func toggleEditorsMaximized() {
        layout.editorsMaximized.toggle()
        saveLayout()
    }
    var collapsedTracks: Set<UUID> = []
    /// Editors page to the playhead until a manual time scroll. Not saved;
    /// each opened document starts following (ADR 0159).
    var followsPlayhead = true
    @ObservationIgnored private var folderDocument: DocumentID?
    @ObservationIgnored private var projectionSource: Snapshot?
    @ObservationIgnored private var projectionCollapsed: Set<UUID> = []
    @ObservationIgnored private var projectedOrder: [UUID] = []
    var choosingFile = false
    var auditionPreferences: SampleAuditionPreferences
    var draft = NoteDraft()
    let browser: FileBrowserState
    @ObservationIgnored let navigation = CanvasNavigationState()
    @ObservationIgnored private let defaults: UserDefaults
    private static let layoutKey = "workspace.layout.v2"

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        browser = FileBrowserState(defaults: defaults)
        let savedAudition =
            defaults.data(forKey: "workspace.audition.v1")
            .flatMap { try? JSONDecoder().decode(SampleAuditionPreferences.self, from: $0) }
            ?? SampleAuditionPreferences()
        auditionPreferences = savedAudition.isValid ? savedAudition : SampleAuditionPreferences()
        let saved = defaults.data(forKey: Self.layoutKey) ?? defaults.data(forKey: "workspace.layout.v1")
        layout = saved.flatMap { try? JSONDecoder().decode(WorkspaceLayout.self, from: $0) } ?? WorkspaceLayout()
        // Invalid saved values cannot affect frame sizes or native controls.
        if !layout.isValid { layout = WorkspaceLayout() }
        saveLayout()
    }

    func toggle(_ pane: WorkspacePane) {
        layout.toggle(pane)
        saveLayout()
    }
    func saveLayout() { if let data = try? JSONEncoder().encode(layout) { defaults.set(data, forKey: Self.layoutKey) } }
    func saveAuditionPreferences() {
        if let data = try? JSONEncoder().encode(auditionPreferences) {
            defaults.set(data, forKey: "workspace.audition.v1")
        }
    }

    func synchronize(_ controller: ProjectController) {
        if folderDocument != controller.documentID {
            collapsedTracks = []
            followsPlayhead = true
            folderDocument = controller.documentID
        }
        if let snapshot = controller.snapshot {
            let folders = Set(
                (0..<snapshot.trackCount).compactMap { index in
                    snapshot.trackHierarchy(at: index)?.hasChildren == true ? snapshot.trackID(at: index) : nil
                })
            collapsedTracks.formIntersection(folders)
            let visible = Set(visibleTrackOrder(controller))
            if let hidden = controller.tracks.first(where: {
                controller.selectedTracks.contains($0.id) && !visible.contains($0.id)
            })?.id {
                let parents = Dictionary(
                    uniqueKeysWithValues: (0..<snapshot.trackCount).compactMap { index -> (UUID, UUID)? in
                        guard let id = snapshot.trackID(at: index),
                            let parent = snapshot.trackHierarchy(at: index)?.parent
                        else { return nil }
                        return (id, parent)
                    })
                var cursor = hidden
                while let parent = parents[cursor] {
                    cursor = parent
                    if visible.contains(parent) { break }
                }
                controller.selection = cursor
            }
        } else {
            collapsedTracks = []
            projectionSource = nil
            projectedOrder = []
            projectionCollapsed = []
        }
        if pluginBrowserDocument != controller.documentID { pluginBrowserDocument = nil }
        if case .history = presentation, controller.snapshot == nil { presentation = nil }
        navigation.prepare(document: controller.documentID, tracks: Set(controller.tracks.map(\.id)))
        draft.synchronize(
            document: controller.documentID, track: controller.pianoTrack,
            note: controller.noteFocus.flatMap { controller.snapshot?.pianoNote($0) })
    }

    func visibleTrackOrder(_ controller: ProjectController) -> [UUID] {
        guard let snapshot = controller.snapshot else {
            projectionSource = nil
            projectedOrder = []
            projectionCollapsed = []
            return []
        }
        if projectionSource !== snapshot || projectionCollapsed != collapsedTracks {
            guard let projected = snapshot.arrangeProjection(collapsedTracks: collapsedTracks) else { return [] }
            projectedOrder = (0..<projected.trackCount).compactMap { projected.trackID(at: $0) }
            projectionSource = snapshot
            projectionCollapsed = collapsedTracks
        }
        return projectedOrder
    }
}

/// Viewport memory is not observed by SwiftUI on every scroll event. Native
/// views own live gestures; this store survives hiding an editor or sidebar.
@MainActor
public final class CanvasNavigationState {
    var arrange: ArrangeCamera?
    var headerWidth = 160.0
    var pianoCamera: PianoCamera?
    var pianoLocationID: UUID?
    private(set) var document: DocumentID?
    private var tracks: Set<UUID> = []
    public init() {}

    func permits(document: DocumentID?, track: UUID? = nil) -> Bool {
        self.document == document && (track.map { tracks.contains($0) } ?? true)
    }

    func prepare(document: DocumentID?, tracks: Set<UUID>) {
        if self.document != document {
            self.document = document
            arrange = nil
            headerWidth = 160
            pianoCamera = nil
            pianoLocationID = nil
        }
        self.tracks = tracks
    }
}
