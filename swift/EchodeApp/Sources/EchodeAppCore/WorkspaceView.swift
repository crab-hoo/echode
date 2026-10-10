import AppKit
import EchodeBridge
import SwiftUI
import UniformTypeIdentifiers

public func text(_ key: String) -> String {
    String(localized: String.LocalizationValue(key), bundle: .module)
}

@MainActor
public struct WorkspaceView: View {
    @Bindable private var controller: ProjectController
    @State private var showInspector = true
    @State private var showBrowser = true
    @State private var showEditor = true
    @State private var showPalette = false
    @State private var search = ""
    @State private var beat = 0.0
    @State private var length = 1.0
    @State private var pitch = 60
    @State private var displayKey = DisplayKey()
    @State private var files: [URL] = []
    @State private var browserURL: URL?
    @State private var showAudio = false
    @State private var seekBeat = 0.0

    public init(controller: ProjectController) { self.controller = controller }

    public var body: some View {
        VStack(spacing: 0) {
            transport
            Divider()
            HSplitView {
                if showInspector { inspector.frame(minWidth: 170, idealWidth: 210, maxWidth: 320) }
                VSplitView {
                    arrange.frame(minHeight: 220)
                    if showEditor { editor.frame(minHeight: 240, idealHeight: 340) }
                }.frame(minWidth: 420)
                if showBrowser { browser.frame(minWidth: 170, idealWidth: 220, maxWidth: 360) }
            }
            Divider()
            HStack {
                Text(controller.projectURL?.lastPathComponent ?? text("No project"))
                Spacer()
                if controller.busy { ProgressView().controlSize(.small) }
                Text(controller.snapshot == nil ? text("Create or open a project to start") : text("Changes recorded automatically"))
                    .foregroundStyle(.secondary)
            }.font(.caption).padding(8)
        }
        .toolbar {
            ToolbarItemGroup(placement: .navigation) {
                Button(action: createProject) { Label(text("New Project"), systemImage: "doc.badge.plus") }
                Button(action: openProject) { Label(text("Open Project"), systemImage: "folder") }
            }
            ToolbarItemGroup {
                Button { Task { await controller.undo() } } label: { Label(text("Undo"), systemImage: "arrow.uturn.backward") }
                    .disabled(controller.undoCount == 0 || controller.busy)
                Button { Task { await controller.redo() } } label: { Label(text("Redo"), systemImage: "arrow.uturn.forward") }
                    .disabled(controller.redoCount == 0 || controller.busy)
                Button(action: addTrack) { Label(text("Add Track"), systemImage: "plus") }
                    .disabled(controller.snapshot == nil || controller.busy)
            }
            ToolbarItemGroup(placement: .primaryAction) {
                Button { showAudio.toggle() } label: { Label(text("Audio Output"), systemImage: "speaker.wave.2") }
                    .popover(isPresented: $showAudio) { outputSettings }
                Button { showInspector.toggle() } label: { Label(text("Inspector"), systemImage: "sidebar.left") }
                Button { showEditor.toggle() } label: { Label(text("Note Editor"), systemImage: "pianokeys") }
                Button { showBrowser.toggle() } label: { Label(text("Files"), systemImage: "sidebar.right") }
                Button { showPalette = true } label: { Label(text("Command Palette"), systemImage: "command") }
            }
        }
        .focusedSceneValue(\.workspaceActions, WorkspaceActions(
            create: createProject, open: openProject, addTrack: addTrack,
            undo: { Task { await controller.undo() } }, redo: { Task { await controller.redo() } },
            inspector: { showInspector.toggle() }, browser: { showBrowser.toggle() },
            editor: { showEditor.toggle() }, palette: { showPalette = true },
            canEdit: controller.snapshot != nil && !controller.busy,
            canUndo: controller.undoCount > 0 && !controller.busy,
            canRedo: controller.redoCount > 0 && !controller.busy,
            canOpen: !controller.busy,
            playback: { Task { await controller.togglePlayback() } },
            stop: { Task { await controller.stopPlayback() } },
            canPlay: controller.snapshot != nil && !controller.busy && !controller.audioBusy
        ))
        .sheet(isPresented: $showPalette) { palette }
        .alert(text("Could not complete the operation"), isPresented: Binding(
            get: { controller.errorMessage != nil }, set: { if !$0 { controller.errorMessage = nil } }
        )) { Button(text("OK")) { controller.errorMessage = nil } } message: { Text(controller.errorMessage ?? "") }
        .frame(minWidth: 820, minHeight: 500)
        .onChange(of: selectionGeometry) { _, geometry in
            if let geometry { pitch = geometry.pitch; beat = Double(geometry.start) / 705_600; length = Double(geometry.length) / 705_600 }
        }
        .onChange(of: controller.selection) { _, _ in controller.noteFocus = nil }
        .task { await controller.observeAudio() }
    }

    private var transport: some View {
        HStack(spacing: 12) {
            Button { Task { await controller.togglePlayback() } } label: {
                Label(text(controller.audio.playing ? "Pause" : "Play"), systemImage: controller.audio.playing ? "pause.fill" : "play.fill")
            }
            Button { Task { await controller.stopPlayback() } } label: { Label(text("Stop"), systemImage: "stop.fill") }
            LabeledContent(text("Position (beats)"), value: (Double(controller.audio.tick) / 705_600).formatted(.number.precision(.fractionLength(2))))
                .monospacedDigit().accessibilityValue((Double(controller.audio.tick) / 705_600).formatted())
            TextField(text("Start beat"), value: $seekBeat, format: .number).frame(width: 80)
                .accessibilityLabel(text("Playback start beat"))
                .onSubmit { Task { await controller.locate(beat: seekBeat) } }
            Button(text("Go")) { Task { await controller.locate(beat: seekBeat) } }
                .disabled(!seekBeat.isFinite || seekBeat < 0)
            Spacer()
            Text(controller.audio.activeDevice?.name ?? text("System Output")).font(.caption).foregroundStyle(.secondary)
        }
        .disabled(controller.snapshot == nil || controller.busy || controller.audioBusy)
        .padding(.horizontal, 12).padding(.vertical, 8)
    }

    private var outputSettings: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(text("Audio Output")).font(.headline)
            Picker(text("Output Device"), selection: Binding(
                get: { controller.audio.selectedUID ?? "" },
                set: { uid in Task { await controller.selectOutput(uid: uid.isEmpty ? nil : uid) } }
            )) {
                Text(text("Follow System Output")).tag("")
                ForEach(controller.audio.devices) { device in Text(device.name).tag(device.uid) }
                if let uid = controller.audio.selectedUID, !controller.audio.devices.contains(where: { $0.uid == uid }) {
                    Text(text("Selected Device Unavailable")).tag(uid)
                }
            }.disabled(controller.snapshot == nil || controller.busy || controller.audioBusy)
            if let device = controller.audio.activeDevice {
                LabeledContent(text("Current Output"), value: device.name)
                LabeledContent(text("Sample Rate"), value: "\(device.sampleRate.formatted()) Hz")
                LabeledContent(text("Buffer Frames"), value: device.bufferFrames.formatted())
                if let uid = controller.audio.selectedUID, uid != device.uid {
                    Text(text("Output Fallback Help")).font(.caption).foregroundStyle(.secondary)
                }
            }
            Text(text("Audio Playback Help")).font(.caption).foregroundStyle(.secondary)
            Button(text("Refresh Devices")) { Task { await controller.refreshOutputs() } }
                .disabled(controller.snapshot == nil || controller.busy || controller.audioBusy)
        }.padding(20).frame(width: 360)
            .task { await controller.refreshOutputs() }
    }

    private var inspector: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(text("Inspector")).font(.headline)
            if let track = controller.tracks.first(where: { $0.id == controller.selection }) {
                Label(track.name, systemImage: "waveform")
                Text(text("Notes are placed on the selected track")).font(.caption).foregroundStyle(.secondary)
                LabeledContent(text("MIDI pitch"), value: "\(pitch)")
                LabeledContent(text("Start beat"), value: beat.formatted())
                LabeledContent(text("Length in beats"), value: length.formatted())
            } else { Text(text("Select a track")).foregroundStyle(.secondary) }
            Spacer()
        }.padding()
    }

    private var arrange: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(text("Arrange")).font(.headline)
                Spacer()
                Text(text("Scroll to move · Pinch to zoom")).font(.caption).foregroundStyle(.secondary)
            }.padding(12)
            Divider()
            if let snapshot = controller.snapshot {
                HSplitView {
                    List(controller.tracks, selection: $controller.selection) { track in
                        Label(track.name, systemImage: "waveform").tag(track.id)
                    }.frame(minWidth: 120, idealWidth: 160, maxWidth: 240)
                    ArrangeCanvas(snapshot: snapshot, selection: $controller.selection, playback: controller.playback, tick: controller.audio.tick)
                        .frame(minWidth: 250)
                }
            } else {
                VStack(spacing: 16) {
                    Image(systemName: "waveform.path").font(.system(size: 48)).foregroundStyle(.secondary)
                    Text(text("Create or open a project to start"))
                    HStack { Button(text("New Project"), action: createProject); Button(text("Open Project"), action: openProject) }
                }.frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }

    private var editor: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(text("Piano Roll")).font(.headline)
                Spacer()
                Picker(text("Display Key"), selection: $displayKey.tonic) {
                    ForEach(0..<12) { tonic in Text(noteName(tonic + 60).dropLast()).tag(tonic) }
                }.fixedSize().help(text("Display key help"))
                Picker(text("Scale"), selection: $displayKey.minor) {
                    Text(text("Major")).tag(false); Text(text("Minor")).tag(true)
                }.fixedSize()
            }.padding(10)
            if let snapshot = controller.snapshot, let track = controller.selection {
                PianoCanvas(snapshot: snapshot, track: track, focus: $controller.noteFocus, key: displayKey, busy: controller.busy,
                    insert: { pitch, beat, length in Task { await controller.insertNote(pitch: pitch, beat: beat, length: length) } },
                    edit: { id, geometry in Task { await controller.editNote(id, geometry: geometry) } },
                    delete: { id in Task { await controller.deleteNote(id) } }, playback: controller.playback, tick: controller.audio.tick)
                    .frame(minHeight: 120)
            } else { Text(text("Select a track")).frame(maxWidth: .infinity, maxHeight: .infinity) }
            Divider()
            HStack {
                Stepper(value: $pitch, in: 0...127) { Text(noteName(pitch)) }.fixedSize()
                TextField(text("Start beat"), value: $beat, format: .number).frame(width: 90)
                    .accessibilityLabel(text("Start beat"))
                TextField(text("Length in beats"), value: $length, format: .number).frame(width: 90)
                    .accessibilityLabel(text("Length in beats"))
                Button(text("Insert Note")) { Task { await controller.insertNote(pitch: pitch, beat: beat, length: length) } }
                    .disabled(controller.selection == nil || controller.busy || !beat.isFinite || !length.isFinite || beat < 0 || length <= 0)
                if let id = controller.noteFocus {
                    Button(text("Apply Changes")) {
                        if let start = PianoCamera.ticks(beat), let ticks = PianoCamera.ticks(length) {
                            Task { await controller.editNote(id, geometry: NoteGeometry(start: start, length: ticks, pitch: pitch)) }
                        }
                    }.disabled(controller.busy)
                    Button(text("Delete Note")) { Task { await controller.deleteNote(id) } }.disabled(controller.busy)
                }
            }.padding(10)
            Text(text("Piano gesture help")).font(.caption).foregroundStyle(.secondary).padding(.horizontal, 10).padding(.bottom, 8)
        }
    }

    private var selectionGeometry: NoteGeometry? {
        guard let id = controller.noteFocus, id.track == controller.selection, let note = controller.snapshot?.pianoNote(id) else { return nil }
        return NoteGeometry(note)
    }

    private var browser: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack { Text(text("Files")).font(.headline); Spacer(); Button(action: chooseFolder) { Image(systemName: "folder.badge.plus") }.help(text("Choose Folder")) }
            if let browserURL { Text(browserURL.lastPathComponent).font(.caption).foregroundStyle(.secondary) }
            List(files, id: \.self) { url in
                Label(url.lastPathComponent, systemImage: "doc")
                    .help(url.path)
                    .onTapGesture(count: 2) { if url.pathExtension == "echode" { Task { await controller.open(url) } } }
            }
        }.padding(12)
    }

    private var palette: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(text("Command Palette")).font(.headline)
            TextField(text("Search commands"), text: $search)
            ForEach(PaletteCommand.allCases.filter { fuzzyMatch(search, in: text($0.rawValue)) }) { command in
                Button(text(command.rawValue)) {
                    showPalette = false
                    search = ""
                    switch command {
                    case .newProject: createProject()
                    case .openProject: openProject()
                    case .addTrack: addTrack()
                    case .undo: Task { await controller.undo() }
                    case .redo: Task { await controller.redo() }
                    case .playback: Task { await controller.togglePlayback() }
                    case .stop: Task { await controller.stopPlayback() }
                    case .inspector: showInspector.toggle()
                    case .browser: showBrowser.toggle()
                    case .editor: showEditor.toggle()
                    }
                }.disabled(!enabled(command))
            }
            Button(text("Close")) { showPalette = false }.keyboardShortcut(.cancelAction)
        }.padding(24).frame(width: 380)
    }

    private func enabled(_ command: PaletteCommand) -> Bool {
        switch command {
        case .newProject, .openProject: !controller.busy
        case .addTrack: controller.snapshot != nil && !controller.busy
        case .undo: controller.undoCount > 0 && !controller.busy
        case .redo: controller.redoCount > 0 && !controller.busy
        case .playback, .stop: controller.snapshot != nil && !controller.busy && !controller.audioBusy
        default: true
        }
    }

    private func createProject() {
        guard !controller.busy else { return }
        let panel = NSSavePanel()
        panel.title = text("New Project")
        panel.nameFieldStringValue = "Untitled.echode"
        panel.allowedContentTypes = [UTType(exportedAs: "io.echode.project", conformingTo: .package)]
        panel.canCreateDirectories = true
        panel.begin { response in
            guard response == .OK, let url = panel.url else { return }
            Task { await controller.open(url, creating: true) }
        }
    }

    private func openProject() {
        guard !controller.busy else { return }
        let panel = NSOpenPanel()
        panel.title = text("Open Project")
        panel.canChooseDirectories = true
        panel.canChooseFiles = true
        panel.allowsMultipleSelection = false
        panel.begin { response in
            guard response == .OK, let url = panel.url else { return }
            Task { await controller.open(url) }
        }
    }

    private func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.begin { response in
            guard response == .OK, let url = panel.url else { return }
            Task {
                do {
                    let found = try await Task.detached {
                        try FileManager.default.contentsOfDirectory(at: url, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
                            .sorted { $0.lastPathComponent.localizedStandardCompare($1.lastPathComponent) == .orderedAscending }
                    }.value
                    browserURL = url
                    files = found
                } catch { controller.errorMessage = error.localizedDescription }
            }
        }
    }

    private func addTrack() { Task { await controller.addTrack(name: "\(text("Track")) \(controller.tracks.count + 1)") } }
}

func noteName(_ pitch: Int) -> String {
    let names = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"]
    return "\(names[pitch % 12])\(pitch / 12 - 1)"
}

enum PaletteCommand: String, CaseIterable, Identifiable {
    case newProject = "New Project", openProject = "Open Project", addTrack = "Add Track"
    case undo = "Undo", redo = "Redo", inspector = "Toggle Inspector", browser = "Toggle Files", editor = "Toggle Note Editor"
    case playback = "Play / Pause", stop = "Stop"
    var id: Self { self }
}

func fuzzyMatch(_ query: String, in title: String) -> Bool {
    var remaining = Array(query.lowercased().filter { !$0.isWhitespace })[...]
    for letter in title.lowercased() where letter == remaining.first { remaining = remaining.dropFirst() }
    return remaining.isEmpty
}

public struct WorkspaceActions {
    public let create: () -> Void, open: () -> Void, addTrack: () -> Void
    public let undo: () -> Void, redo: () -> Void
    public let inspector: () -> Void, browser: () -> Void, editor: () -> Void, palette: () -> Void
    public let canEdit: Bool, canUndo: Bool, canRedo: Bool, canOpen: Bool
    public let playback: () -> Void, stop: () -> Void
    public let canPlay: Bool
}

private struct WorkspaceActionsKey: FocusedValueKey { typealias Value = WorkspaceActions }
extension FocusedValues {
    public var workspaceActions: WorkspaceActions? {
        get { self[WorkspaceActionsKey.self] }
        set { self[WorkspaceActionsKey.self] = newValue }
    }
}
