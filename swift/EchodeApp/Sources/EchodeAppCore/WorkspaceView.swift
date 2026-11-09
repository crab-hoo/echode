import EchodeBridge
import SwiftUI

/// The workspace composes leaf views. Audio polling is observed by transport
/// controls only; canvases receive immutable snapshots and a live atomic feed.
@MainActor
public struct WorkspaceView: View {
    @Bindable private var controller: ProjectController
    @State private var state = WorkspaceState()
    public init(controller: ProjectController) { self.controller = controller }
    /// Probes pass state saved to their own defaults, never the user's.
    init(controller: ProjectController, state: WorkspaceState) { self.controller = controller; _state = State(initialValue: state) }
    private var session: WorkspaceSession { WorkspaceSession(controller: controller, state: state) }

    public var body: some View {
        VStack(spacing: 0) {
            WorkspaceTransportView(controller: controller, followsPlayhead: $state.followsPlayhead)
            Divider()
            if controller.snapshot != nil { panes } else { welcome }
            Divider()
            WorkspaceStatusBar(projectName: controller.projectURL?.lastPathComponent, busy: controller.busy,
                               pendingPlayback: controller.pendingPlayback, hasProject: controller.snapshot != nil)
            ExportStatusView(job: state.exportJob)
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
            case .exportAudio: WorkspaceExportView(controller: controller, job: state.exportJob) { if state.presentation == presentation { state.presentation = nil } }
            }
        }
        .background(WorkspacePluginPanel(controller: controller, document: state.pluginBrowserDocument) { state.pluginBrowserDocument = nil })
        .alert(controller.recoveryPrompt?.title ?? text("Could not complete the operation"), isPresented: Binding(
            get: { controller.recoveryPrompt != nil || controller.errorMessage != nil },
            set: { if !$0, controller.recoveryPrompt == nil { controller.errorMessage = nil } }
        )) {
            if let prompt = controller.recoveryPrompt {
                Button(text("Continue from recovered version"), role: .destructive) { controller.respondToRecovery(prompt, accept: true) }
                Button(text("Cancel"), role: .cancel) { controller.respondToRecovery(prompt, accept: false) }
            } else { Button(text("OK")) { controller.errorMessage = nil } }
        } message: { Text(controller.recoveryPrompt?.message ?? controller.errorMessage ?? "") }
        .frame(minWidth: state.layout.minimumWidth, minHeight: state.layout.minimumHeight)
        .onChange(of: context) { _, _ in state.synchronize(controller) }
        .onChange(of: controller.documentID) { _, _ in session.synchronizeAuditionOutput() }
        .onChange(of: state.layout.browser) { _, _ in session.synchronizeAuditionOutput() }
        .onAppear { [controller, state] in
            state.synchronize(controller)
            session.synchronizeAuditionOutput()
            controller.pluginCommand = { [weak controller, state] command in
                guard let controller else { return }
                WorkspaceSession(controller: controller, state: state).perform(command)
            }
        }
        .onDisappear { controller.respondToRecovery(accept: false); controller.pluginCommand = nil; session.synchronizeAuditionOutput(visible: false) }
        .task { await controller.observeAudio() }
    }

    private var panes: some View {
        GeometryReader { geometry in
            let sizes = state.layout.dimensions(in: geometry.size)
            HStack(spacing: 0) {
                if state.layout.inspector {
                    WorkspaceInspectorView(controller: controller, add: { session.openPlugins(on: $0) }).frame(width: sizes.inspector)
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
                              }, pluginNames: controller.pluginNames, autoMutes: controller.autoMuteReasons, onDropAudio: { session.importDroppedAudio($0) != nil },
                              onMoveClip: { _ = session.moveClip($0) },
                              start: controller.playbackStart, loop: controller.rulerLoop, busy: controller.transportLocked,
                              onSetStart: setStart, onSetLoop: setLoop,
                              followsPlayhead: state.followsPlayhead, onManualScroll: { state.followsPlayhead = false }, onOpenPiano: openPiano)
                    .id(controller.documentID)
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
    private var openPiano: (UUID, Int64) -> Void {
        let document = controller.documentID
        return { track, tick in
            guard let document else { return }
            session.openPiano(PianoTrackTarget(document: document, track: track), tick: tick)
        }
    }
    private var setStart: (Int64) -> Void {
        let document = controller.documentID
        return { tick in Task {
            guard controller.isCurrent(document) else { return }
            await controller.setPlaybackStart(tick: tick)
        } }
    }
    private var setLoop: (LoopRange) -> Void {
        let document = controller.documentID
        return { range in Task {
            guard let document else { return }
            await controller.setLoopRange(range, documentID: document)
        } }
    }
    private var clipEdit: (ClipMixEdit) -> Void {
        let document = controller.documentID
        return { edit in Task {
            guard let document else { return }
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
        controller.arrangeMixActions(document: controller.documentID)
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
        if command.isToggle {
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
        WorkspaceSelectionContext(document: controller.documentID, tracks: controller.tracks.map(\.id), track: controller.selection, pianoTrack: controller.pianoTrack,
            note: controller.noteFocus, geometry: controller.noteFocus.flatMap { controller.snapshot?.pianoNote($0).map(NoteGeometry.init) }, snapshot: controller.snapshot)
    }
}
/// The status row must not change height when preparation starts or ends.
struct WorkspaceStatusBar: View {
    let projectName: String?
    let busy: Bool
    let pendingPlayback: Bool?
    let hasProject: Bool

    private var preparing: Bool { busy || pendingPlayback == true }
    private var message: String {
        if pendingPlayback == true { return text("Preparing Playback") }
        if busy { return text("Applying Changes") }
        return text(hasProject ? "Changes recorded automatically" : "Create or open a project to start")
    }

    var body: some View {
        HStack {
            Text(projectName ?? text("No project")).lineLimit(1)
            Spacer()
            // Reserve the indicator's space so starting preparation never resizes the panes.
            ProgressView().controlSize(.small).opacity(preparing ? 1 : 0).accessibilityHidden(!preparing)
            Text(message).foregroundStyle(.secondary)
        }.font(.caption).padding(8)
    }
}

private struct WorkspaceSelectionContext: Equatable {
    let document: DocumentID?
    let tracks: [UUID]
    let track: UUID?
    let pianoTrack: UUID?
    let note: PianoNote.ID?
    let geometry: NoteGeometry?
    let snapshot: Snapshot?
    static func == (lhs: Self, rhs: Self) -> Bool {
        lhs.document == rhs.document && lhs.tracks == rhs.tracks && lhs.track == rhs.track && lhs.pianoTrack == rhs.pianoTrack && lhs.note == rhs.note && lhs.geometry == rhs.geometry && lhs.snapshot === rhs.snapshot
    }
}
