import EchodeBridge
import SwiftUI

struct WorkspaceNoteEditorView: View {
    @Bindable var controller: ProjectController
    @Bindable var state: WorkspaceState
    var body: some View {
        VStack(spacing: 0) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(text("Piano Roll")).font(.headline)
                    Text(controller.tracks.first { $0.id == controller.pianoTrack }?.name ?? text("Select a track"))
                        .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer()
                Picker(text("Display Key"), selection: $state.displayKey.tonic) {
                    ForEach(0..<12) { tonic in Text(noteName(tonic + 60).dropLast()).tag(tonic) }
                }.fixedSize().help(text("Display key help"))
                Picker(text("Scale"), selection: $state.displayKey.minor) {
                    Text(text("Major")).tag(false); Text(text("Minor")).tag(true)
                }.fixedSize()
            }.padding(10)
            Divider()
            if let snapshot = controller.snapshot, let track = controller.pianoTrack {
                let document = controller.documentID
                HStack(spacing: 0) {
                PianoCanvas(snapshot: snapshot, track: track, focus: $controller.noteFocus, key: state.displayKey, busy: controller.busy,
                    insert: { pitch, beat, length in
                        Task { await controller.insertNote(pitch: pitch, beat: beat, length: length, on: track, documentID: document) }
                    },
                    edit: { id, geometry in Task { await controller.editNote(id, geometry: geometry, documentID: document) } },
                    delete: { id in Task { await controller.deleteNote(id, documentID: document) } },
                    start: controller.playbackStart,
                    setStart: { tick in Task { await controller.setPlaybackStart(tick: tick) } },
                    loop: controller.rulerLoop,
                    setLoop: { range in if let document { Task { await controller.setLoopRange(range, documentID: document) } } },
                    playback: controller.playback, transportGeneration: controller.transportGeneration,
                    navigation: state.navigation, documentID: document,
                    followsPlayhead: state.followsPlayhead, onManualScroll: { state.followsPlayhead = false },
                    inputLength: { controller.noteInput.length },
                    velocity: { id, value in Task { await controller.setNoteVelocity(id, velocity: value, documentID: document) } },
                    velocities: { change in Task { await controller.setNoteVelocities(change, documentID: document) } },
                    moves: { change in Task { await controller.moveNotes(change, documentID: document) } },
                    resizes: { change in Task { await controller.resizeNotes(change, documentID: document) } },
                    steps: { step in if let document { _ = controller.enqueuePianoStep(step, documentID: document) } },
                    references: controller.pianoReferences, location: controller.pianoLocation,
                    selection: controller.noteSelection, selectNotes: { if controller.documentID == document { controller.selectNotes($0) } },
                    deleteNotes: { ids in Task { await controller.deleteNotes(ids, documentID: document) } })
                    .id(controller.documentID)
                    Divider()
                    PianoTrackSelector(controller: controller, rows: controller.pianoTrackRows).frame(width: 210)
                }
                Divider()
                fields
            } else { Text(text("Select a track")).foregroundStyle(.secondary).frame(maxWidth: .infinity, maxHeight: .infinity) }
        }
    }
    private var fields: some View {
        let document = state.draft.document
        return VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 12) {
                Stepper(value: $state.draft.pitch, in: 0...127) { Text(noteName(state.draft.pitch)) }.fixedSize().accessibilityLabel(text("MIDI pitch"))
                LabeledContent(text("Start beat")) {
                    TextField(text("Start beat"), text: $state.draft.beat).frame(width: 70).focusedValue(\.workspaceTextEntry, true)
                        .accessibilityLabel(text("Start beat"))
                }
                LabeledContent(text("Length in beats")) {
                    TextField(text("Length in beats"), text: $state.draft.length).frame(width: 70).focusedValue(\.workspaceTextEntry, true)
                        .accessibilityLabel(text("Length in beats"))
                }
            }.disabled(controller.busy)
            HStack {
                if let id = state.draft.note {
                    Button(text("Apply Changes")) {
                        guard let geometry = state.draft.geometry else { return }
                        Task { await controller.editNote(id, geometry: geometry, documentID: document) }
                    }.disabled(!state.draft.canApply)
                    Button(text("Revert")) { state.draft.reset() }.disabled(!state.draft.dirty)
                    Button(text("Delete Note")) { Task { await controller.deleteNote(id, documentID: document) } }
                    Spacer()
                    Button(text("New Note")) { if controller.documentID == document { controller.noteFocus = nil } }
                } else {
                    Button(text("Insert Note")) {
                        guard let geometry = state.draft.geometry, let track = state.draft.track else { return }
                        Task {
                            if controller.documentID == document {
                                await controller.insertNote(pitch: geometry.pitch, beat: Double(geometry.start) / 705_600,
                                                            length: Double(geometry.length) / 705_600, on: track, documentID: document)
                            }
                        }
                    }.disabled(state.draft.geometry == nil)
                    Spacer()
                }
            }.disabled(controller.busy)
            Text(state.draft.validation ?? text("Piano gesture help"))
                .font(.caption).foregroundStyle(state.draft.validation == nil ? .secondary : .primary).lineLimit(2)
        }.padding(10)
    }
}
