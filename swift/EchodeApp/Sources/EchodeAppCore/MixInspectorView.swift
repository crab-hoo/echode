import EchodeBridge
import SwiftUI

@MainActor
struct TrackMixInspector: View {
    let controller: ProjectController
    let track: UUID
    let mix: TrackMix
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(text("Track Controls")).font(.subheadline)
            ControlNumberField(title: text("Volume"), value: MixScale.decibels(mix.gain.value), unit: "dB", minimum: -120,
                maximum: 20 * log10(4), silence: true) { value in
                let gain: Float = value == -.infinity ? 0 : Float(pow(10, value / 20))
                set(.gain, gain)
            }
            ControlNumberField(title: text("Pan"), value: Double(mix.pan.value) * 100, unit: "%", minimum: -100, maximum: 100) { set(.pan, Float($0 / 100)) }
            HStack {
                Toggle(text("Mute"), isOn: Binding(get: { mix.mute.value != 0 }, set: { set(.mute, $0 ? 1 : 0) }))
                Toggle(text("Solo"), isOn: Binding(get: { mix.solo.value != 0 }, set: { set(.solo, $0 ? 1 : 0) }))
            }.toggleStyle(.button)
            TimelineView(.periodic(from: .now, by: 0.05)) { _ in
                let peak = controller.playback?.peak(track: track) ?? 0
                HStack {
                    ProgressView(value: MixScale.meter(peak)).tint(peak >= 1 ? .red : .green)
                        .accessibilityLabel(text("Peak Level"))
                    Text(MixScale.label(peak)).font(.caption.monospacedDigit()).frame(width: 65, alignment: .trailing)
                }
            }
        }.disabled(controller.busy || controller.faderPreview != nil)
    }
    private func set(_ field: MixerField, _ value: Float) {
        let document = controller.documentID
        Task { await controller.setMixer(track, field: field, value: value, documentID: document) }
    }
}

@MainActor
struct ClipMixInspector: View {
    let controller: ProjectController
    let clip: ClipMix
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(text("Audio Clip")).font(.subheadline)
            ControlNumberField(title: text("Clip Gain"), value: MixScale.decibels(clip.gain), unit: "dB", minimum: -120,
                maximum: 20 * log10(Double(MixScale.clipGainMax)), silence: true) { db in
                let gain: Float = db == -.infinity ? 0 : min(MixScale.clipGainMax, Float(pow(10, db / 20)))
                let document = controller.documentID
                Task { await controller.setClipGain(clip.id, gain: gain, documentID: document) }
            }
            ControlNumberField(title: text("Fade In"), value: Double(clip.fadeIn) / 705_600, unit: "ms", minimum: 0,
                maximum: Double(UInt32.max) / 768_000 * 1000) { setFade($0, entering: true) }
            ControlNumberField(title: text("Fade Out"), value: Double(clip.fadeOut) / 705_600, unit: "ms", minimum: 0,
                maximum: Double(UInt32.max) / 768_000 * 1000) { setFade($0, entering: false) }
            Text(text("Drag the clip corners to change fades. Drag its gain handle vertically."))
                .font(.caption).foregroundStyle(.secondary)
        }.disabled(controller.busy || controller.faderPreview != nil)
    }
    private func setFade(_ milliseconds: Double, entering: Bool) {
        let flicks = Int64((milliseconds * 705_600).rounded())
        let document = controller.documentID
        Task { await controller.setClipFades(clip.id, fadeIn: entering ? flicks : clip.fadeIn,
            fadeOut: entering ? clip.fadeOut : flicks, documentID: document) }
    }
}

/// A durable edit is submitted once on Return or focus loss, never per digit.
struct ControlNumberField: View {
    let title: String
    let value: Double
    let unit: String
    var minimum: Double
    var maximum: Double = Double.greatestFiniteMagnitude
    var silence = false
    let commit: (Double) -> Void
    @State private var draft = ""
    @State private var invalid = false
    @FocusState private var focused: Bool
    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text(title).font(.caption)
                Spacer()
                TextField(title, text: $draft).multilineTextAlignment(.trailing).frame(width: 70)
                    .textFieldStyle(.roundedBorder).focused($focused).accessibilityLabel(title)
                    .onSubmit(submit).onExitCommand { reset(); focused = false }
                Text(unit).font(.caption).foregroundStyle(.secondary).frame(width: 22, alignment: .leading)
            }
            if invalid { Text(text("Enter a valid value in range")).font(.caption).foregroundStyle(.red) }
        }
        .onAppear(perform: reset)
        .onChange(of: value) { _, _ in if !focused { reset() } }
        .onChange(of: focused) { _, active in if !active { submit() } }
    }
    private func reset() { draft = value.isFinite ? String(format: "%.2f", value) : "−∞"; invalid = false }
    private func submit() {
        let normalized = draft.trimmingCharacters(in: .whitespacesAndNewlines).replacingOccurrences(of: ",", with: ".")
        let parsed = silence && ["−∞", "-∞", "-inf"].contains(normalized.lowercased()) ? -Double.infinity : Double(normalized)
        guard let parsed, silence && parsed == -.infinity || parsed.isFinite && parsed >= minimum && parsed <= maximum else { invalid = true; return }
        invalid = false
        if abs(parsed - value) > 0.005 || parsed.isInfinite != value.isInfinite { commit(parsed) }
    }
}
