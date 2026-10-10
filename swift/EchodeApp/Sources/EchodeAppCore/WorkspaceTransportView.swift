import SwiftUI

struct WorkspaceTransportView: View {
    let controller: ProjectController
    @State private var seek = "0"
    private var seekBeat: Double? { NoteDraft.number(seek).flatMap { $0 >= 0 && PianoCamera.ticks($0) != nil ? $0 : nil } }
    var body: some View {
        HStack(spacing: 12) {
            Button { Task { await controller.togglePlayback() } } label: {
                Label(text(controller.audio.playing ? "Pause" : "Play"), systemImage: controller.audio.playing ? "pause.fill" : "play.fill")
            }
            Button { Task { await controller.stopPlayback() } } label: { Label(text("Stop"), systemImage: "stop.fill") }
            LabeledContent(text("Position (beats)"), value: (Double(controller.audio.tick) / 705_600).formatted(.number.precision(.fractionLength(2))))
                .monospacedDigit()
            TextField(text("Start beat"), text: $seek).frame(width: 80)
                .focusedValue(\.workspaceTextEntry, true)
                .accessibilityLabel(text("Playback start beat")).onSubmit(locate)
            Button(text("Go"), action: locate).disabled(seekBeat == nil)
            Spacer()
            Text(controller.audio.activeDevice?.name ?? text("System Output")).font(.caption).foregroundStyle(.secondary).lineLimit(1)
        }.disabled(controller.snapshot == nil || controller.busy || controller.audioBusy)
            .padding(.horizontal, 12).padding(.vertical, 8)
    }
    private func locate() { if let beat = seekBeat { Task { await controller.locate(beat: beat) } } }
}

struct WorkspaceOutputView: View {
    let controller: ProjectController
    let close: () -> Void
    var body: some View {
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
            HStack {
                Button(text("Refresh Devices")) { Task { await controller.refreshOutputs() } }
                    .disabled(controller.snapshot == nil || controller.busy || controller.audioBusy)
                Spacer()
                Button(text("Close"), action: close).keyboardShortcut(.cancelAction)
            }
        }.padding(20).frame(width: 400).task { await controller.refreshOutputs() }
    }
}
