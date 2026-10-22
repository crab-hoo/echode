import EchodeBridge
import SwiftUI

/// The workspace composes leaf views. Audio polling is observed by transport
/// controls only; canvases receive immutable snapshots and a live atomic feed.
@MainActor
public struct WorkspaceView: View {
    @Bindable private var controller: ProjectController
    @State private var state = WorkspaceState()
    public init(controller: ProjectController) { self.controller = controller }
    private var session: WorkspaceSession { WorkspaceSession(controller: controller, state: state) }

    public var body: some View {
        VStack(spacing: 0) {
            WorkspaceTransportView(controller: controller)
            Divider()
            if controller.snapshot != nil { panes } else { welcome }
            Divider()
            HStack {
                Text(controller.projectURL?.lastPathComponent ?? text("No project")).lineLimit(1)
                Spacer()
                if controller.busy || controller.pendingPlayback == true { ProgressView().controlSize(.small) }
                Text(text(controller.pendingPlayback == true ? "Preparing Playback" : controller.busy ? "Applying Changes" : controller.snapshot == nil ? "Create or open a project to start" : "Changes recorded automatically"))
                    .foregroundStyle(.secondary)
            }.font(.caption).padding(8)
        }
        .toolbar {
            ToolbarItemGroup(placement: .navigation) { command(.newProject); command(.openProject) }
            ToolbarItemGroup { command(.undo); command(.redo); command(.history); command(.addTrack) }
            ToolbarItemGroup(placement: .primaryAction) {
                command(.output); command(.plugins); command(.devices); command(.inspector); command(.editor); command(.maximizeEditors); command(.browser); command(.palette)
            }
        }
        .focusedSceneValue(\.workspaceActions, WorkspaceActions(perform: session.perform,
            enabled: Set(WorkspaceCommand.allCases.filter { session.enabled($0) }),
            selected: Set(WorkspaceCommand.allCases.filter { session.isSelected($0) })))
        .sheet(item: $state.presentation) { presentation in
            switch presentation {
            case .palette: WorkspacePalette(session: session)
            case .output: WorkspaceOutputView(controller: controller) { if state.presentation == presentation { state.presentation = nil } }
            case .history: WorkspaceHistoryView(controller: controller) { if state.presentation == presentation { state.presentation = nil } }
            }
        }
        .background(WorkspacePluginPanel(controller: controller, document: state.pluginBrowserDocument) { state.pluginBrowserDocument = nil })
        .alert(text("Could not complete the operation"), isPresented: Binding(
            get: { controller.errorMessage != nil }, set: { if !$0 { controller.errorMessage = nil } }
        )) { Button(text("OK")) { controller.errorMessage = nil } } message: { Text(controller.errorMessage ?? "") }
        .frame(minWidth: state.layout.minimumWidth, minHeight: state.layout.minimumHeight)
        .onChange(of: context) { _, _ in state.synchronize(controller) }
        .onAppear { [controller, state] in
            state.synchronize(controller)
            controller.pluginCommand = { [weak controller, state] command in
                guard let controller else { return }
                WorkspaceSession(controller: controller, state: state).perform(command)
            }
        }
        .onDisappear { controller.pluginCommand = nil }
        .task { await controller.observeAudio() }
    }

    private var panes: some View {
        GeometryReader { geometry in
            let sizes = state.layout.dimensions(in: geometry.size)
            HStack(spacing: 0) {
                if state.layout.inspector {
                    WorkspaceInspectorView(controller: controller).frame(width: sizes.inspector)
                    splitter(.inspector, value: sizes.inspector, size: geometry.size)
                }
                WorkspaceCenterStack(layout: state.layout, sizes: sizes,
                    resize: { state.layout.resize(.editor, to: $0, in: geometry.size) }, finish: state.saveLayout,
                    arrange: { arrange }, editor: { WorkspaceNoteEditorView(controller: controller, state: state) },
                    devices: { WorkspaceDeviceView(controller: controller, add: { session.openPlugins() }) })
                if state.layout.browser {
                    splitter(.browser, value: sizes.browser, size: geometry.size)
                    WorkspaceFileBrowser(session: session).frame(width: sizes.browser)
                }
            }
        }
    }

    private var arrange: some View {
        VStack(spacing: 0) {
            HStack {
                Text(text("Arrange")).font(.headline)
                Spacer()
                Text(text("Scroll to move · Pinch to zoom")).font(.caption).foregroundStyle(.secondary)
            }.padding(10)
            Divider()
            if let snapshot = controller.snapshot {
                ArrangeCanvas(snapshot: snapshot, selection: $controller.selection,
                              playback: controller.playback, transportGeneration: controller.transportGeneration,
                              navigation: state.navigation, documentID: controller.documentID,
                              selectedClip: controller.clipFocus, faderPreview: controller.faderPreview,
                              mixActions: mixActions, onSelectClip: { controller.clipFocus = $0 }, onClipEdit: clipEdit,
                              onOpenFX: { session.openPlugins(on: $0) }, onDropFX: fxDrop,
                              selectedTracks: controller.selectedTracks, onSelectTrack: { controller.selectTrack($0, change: $1, visibleOrder: state.visibleTrackOrder(controller)) },
                              collapsedTracks: state.collapsedTracks, onFolderDisclosure: { id, collapsed in
                                  if collapsed { state.collapsedTracks.insert(id) } else { state.collapsedTracks.remove(id) }
                              }, onDuplicateTracks: { _ = session.duplicateTracks() }, onOpenPlugin: { _, device in
                                  guard let document = controller.documentID else { return }
                                  Task { _ = await controller.openPluginWindow(device, documentID: document) }
                              }, pluginNames: controller.pluginNames, onDropAudio: { session.importDroppedAudio($0) != nil }).id(controller.documentID)
                Divider()
                HStack {
                    Button { session.addTrack(atEnd: true) } label: {
                        Label(text("Add Track at End"), systemImage: "plus")
                    }.disabled(!session.enabled(.addTrack)).accessibilityIdentifier("track.append")
                    Spacer()
                }.padding(8)
            } else {
                VStack(spacing: 12) {
                    Text(text("Add a track to start"))
                    Button(WorkspaceCommand.addTrack.title) { session.perform(.addTrack) }.disabled(!session.enabled(.addTrack))
                }.frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }
    private var clipEdit: (ClipMixEdit) -> Void {
        let document = controller.documentID
        return { edit in Task {
            switch edit {
            case .gain(let id, let gain): await controller.setClipGain(id, gain: gain, documentID: document)
            case .fades(let id, let entering, let leaving): await controller.setClipFades(id, fadeIn: entering, fadeOut: leaving, documentID: document)
            }
        } }
    }
    private var fxDrop: (UUID, TrackFXDrop) -> Void {
        let document = controller.documentID
        return { track, drop in Task {
            guard let document else { return }
            await controller.dropFX(drop, on: track, documentID: document)
        } }
    }
    private var mixActions: MixActions {
        let document = controller.documentID
        return MixActions(begin: { controller.beginFader($0, field: $1) }, change: { controller.changeFader($0) },
            finish: { commit in Task { await controller.finishFader(commit: commit) } },
            set: { track, field, value in Task {
                if controller.faderPreview != nil { await controller.finishFader(commit: true) }
                await controller.setMixer(track, field: field, value: value, documentID: document)
            } })
    }
    private var welcome: some View {
        VStack(spacing: 16) {
            Image(systemName: "waveform.path").font(.system(size: 48)).foregroundStyle(.secondary)
            Text(text("Create or open a project to start"))
            HStack { Button(WorkspaceCommand.newProject.title) { session.perform(.newProject) }; Button(WorkspaceCommand.openProject.title) { session.perform(.openProject) } }
                .disabled(controller.busy || state.choosingFile)
        }.frame(maxWidth: .infinity, maxHeight: .infinity)
    }
    @ViewBuilder private func command(_ command: WorkspaceCommand) -> some View {
        if command.isLayoutToggle {
            Toggle(isOn: Binding(get: { session.isSelected(command) }, set: { _ in session.perform(command) })) {
                Label(command.title, systemImage: command.icon)
            }.toggleStyle(.button).labelStyle(.iconOnly).help(command.title)
                .disabled(!session.enabled(command)).accessibilityIdentifier("workspace.\(command.rawValue)")
        } else {
            Button { session.perform(command) } label: { Label(command.title, systemImage: command.icon) }
                .labelStyle(.iconOnly).help(command.title).disabled(!session.enabled(command))
        }
    }
    private func splitter(_ pane: WorkspacePane, value: Double, size: CGSize) -> some View {
        WorkspaceDivider(pane: pane, value: value, resize: { state.layout.resize(pane, to: $0, in: size) }, finish: state.saveLayout)
            .frame(width: pane == .editor ? nil : 5, height: pane == .editor ? 5 : nil)
    }
    private var context: WorkspaceSelectionContext {
        WorkspaceSelectionContext(document: controller.documentID, tracks: controller.tracks.map(\.id), track: controller.selection,
            note: controller.noteFocus, geometry: controller.noteFocus.flatMap { controller.snapshot?.pianoNote($0).map(NoteGeometry.init) }, snapshot: controller.snapshot)
    }
}
private struct WorkspaceSelectionContext: Equatable {
    let document: UUID?
    let tracks: [UUID]
    let track: UUID?
    let note: PianoNote.ID?
    let geometry: NoteGeometry?
    let snapshot: Snapshot?
    static func == (lhs: Self, rhs: Self) -> Bool {
        lhs.document == rhs.document && lhs.tracks == rhs.tracks && lhs.track == rhs.track && lhs.note == rhs.note && lhs.geometry == rhs.geometry && lhs.snapshot === rhs.snapshot
    }
}
