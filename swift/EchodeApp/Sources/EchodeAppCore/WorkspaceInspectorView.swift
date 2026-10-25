import SwiftUI

struct WorkspaceInspectorView: View {
    let controller: ProjectController
    let add: (UUID) -> Void
    var body: some View {
        let document = controller.documentID
        return ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                Text(text("Inspector")).font(.headline)
                if let track = controller.tracks.first(where: { $0.id == controller.selection }),
                   let index = controller.tracks.firstIndex(where: { $0.id == track.id }), let snapshot = controller.snapshot {
                    let audioClip = controller.clipFocus.flatMap { snapshot.clipMix($0)?.isAudio } == true
                    Label(track.name, systemImage: "waveform").lineLimit(2)
                    if let mix = snapshot.mixer(onTrack: index) {
                        Divider()
                        TrackMixInspector(controller: controller, track: track.id, mix: mix)
                    }
                    if let id = controller.clipFocus, let clip = snapshot.clipMix(id), clip.isAudio {
                        Divider()
                        ClipMixInspector(controller: controller, clip: clip).id(clip.id)
                    }
                    if let id = controller.noteFocus, let note = snapshot.pianoNote(id) {
                        Divider()
                        Text(text("Selected Note")).font(.subheadline)
                        LabeledContent(text("MIDI pitch"), value: noteName(note.pitch))
                        LabeledContent(text("Start beat"), value: (Double(note.start) / 705_600).formatted())
                        LabeledContent(text("Length in beats"), value: (Double(note.length) / 705_600).formatted())
                    } else if InspectorNotes.showsPrompt(noteFocused: false, audioClipFocused: audioClip) {
                        Text(text("Select a note to inspect it")).font(.caption).foregroundStyle(.secondary)
                    }
                    Divider()
                    Text(text("Plugins")).font(.subheadline)
                    ForEach(controller.deviceRows(on: track.id)) { device in
                        HStack {
                            Button { if let document { Task { await controller.openPluginWindow(device.id, documentID: document) } } } label: {
                                HStack {
                                    Text(device.title).font(.caption).lineLimit(2)
                                    if device.windowOpen {
                                        Image(systemName: "macwindow")
                                            .foregroundStyle(Color.accentColor)
                                            .accessibilityLabel(text("Plugin window open") + " " + device.title)
                                    }
                                }
                            }
                            .buttonStyle(.plain).help(device.plugin + "\n" + text("Open plugin window"))
                            .accessibilityHint(text("Open plugin window"))
                            .disabled(controller.busy || device.offline)
                            Spacer()
                            Button { Task { await controller.removeDevice(device.id, documentID: document) } } label: { Image(systemName: "minus.circle") }
                                .help(text("Remove")).accessibilityLabel(text("Remove") + " " + device.title).disabled(controller.busy)
                        }
                    }
                    InspectorAddFX(enabled: !controller.busy) {
                        guard controller.documentID == document, !controller.busy else { return }
                        add(track.id)
                    }
                } else { Text(text("Select a track")).foregroundStyle(.secondary) }
            }.frame(maxWidth: .infinity, alignment: .leading).padding(12)
        }
    }
}

enum InspectorNotes {
    /// The empty-note caption stays out of the way while an audio clip is open.
    static func showsPrompt(noteFocused: Bool, audioClipFocused: Bool) -> Bool {
        !noteFocused && !audioClipFocused
    }
}
