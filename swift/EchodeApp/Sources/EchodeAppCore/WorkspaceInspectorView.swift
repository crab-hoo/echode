import SwiftUI

struct WorkspaceInspectorView: View {
    let controller: ProjectController
    var body: some View {
        let document = controller.documentID
        return ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                Text(text("Inspector")).font(.headline)
                if let track = controller.tracks.first(where: { $0.id == controller.selection }),
                   let index = controller.tracks.firstIndex(where: { $0.id == track.id }), let snapshot = controller.snapshot {
                    Label(track.name, systemImage: "waveform").lineLimit(2)
                    if let id = controller.noteFocus, let note = snapshot.pianoNote(id) {
                        Divider()
                        Text(text("Selected Note")).font(.subheadline)
                        LabeledContent(text("MIDI pitch"), value: noteName(note.pitch))
                        LabeledContent(text("Start beat"), value: (Double(note.start) / 705_600).formatted())
                        LabeledContent(text("Length in beats"), value: (Double(note.length) / 705_600).formatted())
                    } else { Text(text("Select a note to inspect it")).font(.caption).foregroundStyle(.secondary) }
                    Divider()
                    Text(text("Plugins")).font(.subheadline)
                    ForEach(0..<snapshot.deviceCount(onTrack: index), id: \.self) { device in
                        if let plugin = snapshot.devicePlugin(onTrack: index, at: device), let id = snapshot.deviceID(onTrack: index, at: device) {
                            HStack {
                                Text(plugin).font(.caption).lineLimit(2).help(plugin)
                                Spacer()
                                Button { Task { await controller.removeDevice(id, documentID: document) } } label: { Image(systemName: "minus.circle") }
                                    .help(text("Remove")).accessibilityLabel(text("Remove") + " " + plugin).disabled(controller.busy)
                            }
                        }
                    }
                } else { Text(text("Select a track")).foregroundStyle(.secondary) }
            }.frame(maxWidth: .infinity, alignment: .leading).padding(12)
        }
    }
}
