import EchodeBridge
import SwiftUI

/// The only reader of the polled position, so a playback poll re-evaluates
/// this label and nothing else (ADR 0289).
private struct TransportPosition: View {
    let controller: ProjectController
    var body: some View {
        LabeledContent(
            text("Position (beats)"),
            value: (Double(controller.position) / Tick.perBeat).formatted(.number.precision(.fractionLength(2)))
        )
        .monospacedDigit()
    }
}

struct WorkspaceTransportView: View {
    let controller: ProjectController
    @Binding var followsPlayhead: Bool
    @State private var seek = "0"
    @State private var loopEntry = LoopEntry(range: .initial)
    @FocusState private var loopField: LoopField?
    @FocusState private var startField: Bool
    private enum LoopField: Hashable { case start, end }
    private var seekBeat: Double? { NoteDraft.number(seek).flatMap { $0 >= 0 && Tick.ticks($0) != nil ? $0 : nil } }
    private var loopRange: LoopRange? { loopEntry.range }
    var body: some View {
        HStack(spacing: 12) {
            WorkspacePlayButton(paused: controller.playbackRequested) { Task { await controller.togglePause() } }
            Button {
                Task { await controller.stopPlayback() }
            } label: {
                Label(text("Stop"), systemImage: "stop.fill")
            }
            Button {
                Task { await controller.toggleMetronome() }
            } label: {
                Label(text("Metronome"), systemImage: controller.audio.metronome ? "metronome.fill" : "metronome")
            }
            TransportPosition(controller: controller)
            TextField(text("Start beat"), text: $seek).frame(width: 80)
                .focused($startField).focusedValue(\.workspaceTextEntry, true)
                .accessibilityLabel(text("Playback start beat")).onSubmit(setStart)
            Button(text("Go"), action: setStart).disabled(seekBeat == nil)
            TextField(text("Loop start (beats)"), text: $loopEntry.start).frame(width: 64)
                .focused($loopField, equals: .start).focusedValue(\.workspaceTextEntry, true)
                .accessibilityLabel(text("Loop start (beats)")).onSubmit(commitLoop)
            TextField(text("Loop end (beats)"), text: $loopEntry.end).frame(width: 64)
                .focused($loopField, equals: .end).focusedValue(\.workspaceTextEntry, true)
                .accessibilityLabel(text("Loop end (beats)")).onSubmit(commitLoop)
            Button {
                storeLoopDraft()
                Task { await controller.toggleLoop() }
            } label: {
                Label(text("Loop"), systemImage: controller.audio.loopStart == nil ? "repeat" : "repeat.circle.fill")
            }.disabled(controller.audio.loopStart == nil && loopRange == nil)
            // A manual time scroll turns this off; it stays off until turned on here (ADR 0159).
            Toggle(isOn: $followsPlayhead) {
                Label(WorkspaceCommand.followPlayhead.title, systemImage: WorkspaceCommand.followPlayhead.icon)
            }.toggleStyle(.button).accessibilityIdentifier("transport.follow")
            Spacer()
            if let notice = controller.autoMuteNotice {
                Text(notice).font(.caption).foregroundStyle(.secondary).lineLimit(1).help(notice)
            }
            Text(controller.audio.activeDevice?.name ?? text("System Output")).font(.caption).foregroundStyle(
                .secondary
            ).lineLimit(1)
        }.disabled(controller.transportLocked)
            .padding(.horizontal, 12).padding(.vertical, 8)
            .onChange(of: loopEntry) { _, _ in storeLoopDraft() }
            .onChange(of: loopField) { previous, _ in
                if previous != nil {
                    commitLoop()
                    if loopField == nil { showLoop() }
                }
            }
            .onChange(of: controller.documentID) { _, _ in
                showLoop()
                showStart()
            }
            // The rulers change the start too. A draft being typed is kept.
            .onChange(of: controller.playbackStart, initial: true) { _, _ in if !startField { showStart() } }
            .onChange(of: controller.loopRange, initial: true) { _, _ in if loopField == nil { showLoop() } }
            .onChange(of: startField) { _, focused in if !focused { showStart() } }
    }
    private func setStart() { if let beat = seekBeat { Task { await controller.setPlaybackStart(beat: beat) } } }
    private func showStart() { seek = NoteDraft.format(Double(controller.playbackStart) / Tick.perBeat) }
    private func showLoop() { loopEntry = LoopEntry(range: controller.loopRange) }
    private func storeLoopDraft() {
        if let range = loopRange { controller.loopRange = range }
    }
    private func commitLoop() {
        guard let range = loopRange else { return }
        controller.loopRange = range
        guard
            LoopCommit.needsSend(
                soundingStart: controller.audio.loopStart, soundingEnd: controller.audio.loopEnd,
                start: range.start, end: range.end)
        else { return }
        Task { await controller.updateLoopBounds() }
    }
}

/// The play control keeps one width while its label switches to pause.
struct WorkspacePlayButton: View {
    let paused: Bool
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            // Both labels share the slot so the transport never shifts sideways.
            ZStack {
                Label(text("Play"), systemImage: "play.fill").opacity(paused ? 0 : 1)
                Label(text("Pause"), systemImage: "pause.fill").opacity(paused ? 1 : 0)
            }.accessibilityHidden(true)
        }.accessibilityLabel(text(paused ? "Pause" : "Play"))
    }
}

enum LoopCommit {
    /// An enabled loop sends a new range. The same sounding range is left alone.
    static func needsSend(soundingStart: Int64?, soundingEnd: Int64?, start: Int64, end: Int64) -> Bool {
        guard soundingStart != nil else { return false }
        return soundingStart != start || soundingEnd != end
    }
}

struct WorkspaceOutputView: View {
    let controller: ProjectController
    let close: () -> Void
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(text("Audio Output")).font(.headline)
            Picker(
                text("Output Device"),
                selection: Binding(
                    get: { controller.audio.selectedUID ?? "" },
                    set: { uid in Task { await controller.selectOutput(uid: uid.isEmpty ? nil : uid) } }
                )
            ) {
                Text(text("Follow System Output")).tag("")
                ForEach(controller.audio.devices) { device in Text(device.name).tag(device.uid) }
                if let uid = controller.audio.selectedUID, !controller.audio.devices.contains(where: { $0.uid == uid })
                {
                    Text(text("Selected Device Unavailable")).tag(uid)
                }
            }.disabled(controller.transportLocked)
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
                    .disabled(controller.transportLocked)
                Spacer()
                Button(text("Close"), action: close).keyboardShortcut(.cancelAction)
            }
        }.padding(20).frame(width: 400).task { await controller.refreshOutputs() }
    }
}
