import EchodeBridge
import SwiftUI

@MainActor
struct TrackMixInspector: View {
    let controller: ProjectController
    let track: UUID
    let mix: TrackMix
    @State private var meterPaused = false
    var body: some View {
        let transport = controller.playbackRequested
        let document = controller.documentID
        VStack(alignment: .leading, spacing: 10) {
            VStack(alignment: .leading, spacing: 10) {
                Text(text("Track Controls")).font(.subheadline)
                ControlNumberField(title: text("Volume"), value: MixScale.decibels(mix.gain.value), unit: "dB", minimum: -120,
                    maximum: 20 * log10(4), silence: true) { value in
                    let gain: Float = value == -.infinity ? 0 : Float(pow(10, value / 20))
                    set(document, .gain, gain)
                }
                ControlNumberField(title: text("Pan"), value: Double(mix.pan.value) * 100, unit: "%", minimum: -100, maximum: 100) { set(document, .pan, Float($0 / 100)) }
                HStack {
                    Toggle(text("Mute"), isOn: Binding(get: { mix.mute.value != 0 }, set: { set(document, .mute, $0 ? 1 : 0) }))
                    Toggle(text("Solo"), isOn: Binding(get: { mix.solo.value != 0 }, set: { set(document, .solo, $0 ? 1 : 0) }))
                }.toggleStyle(.button)
                if controller.selectedTracks.contains(track), controller.selectedTracks.count > 1 {
                    Text(text("Numeric entries and header accessibility steps adjust selected tracks relatively. Drags affect one track."))
                        .font(.caption).foregroundStyle(.secondary)
                }
            }.disabled(controller.busy || controller.faderPreview != nil)
            TimelineView(.animation(minimumInterval: 1.0 / 120.0, paused: meterPaused && !transport)) { timeline in
                let peak = controller.playback?.peak(track: track) ?? 0
                let playing = controller.playback?.position.playing == true
                HStack(spacing: 8) {
                    PeakMeter(peak: peak)
                    Text(MixScale.meterLabel(peak)).font(.caption.monospacedDigit()).frame(width: 72, alignment: .trailing)
                }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(text("Peak Level"))
                .accessibilityValue(MixScale.meterLabel(peak))
                .onChange(of: timeline.date) { _, _ in
                    let quiet = !MeterFollow.running(transportActive: transport, playing: playing, peak: peak)
                    if meterPaused != quiet { meterPaused = quiet }
                }
            }
        }
    }
    private func set(_ document: UUID?, _ field: MixerField, _ value: Float) {
        let actions = controller.arrangeMixActions(document: document)
        if field == .gain || field == .pan { (actions.enter ?? actions.set)(track, field, value) }
        else { actions.set(track, field, value) }
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
            ControlNumberField(title: text("Fade In"), value: Double(clip.fadeIn) / ClipEntry.flicksPerMillisecond, unit: "ms", minimum: 0,
                maximum: ClipEntry.maximumFadeMilliseconds) { setFade($0, entering: true) }
            ControlNumberField(title: text("Fade Out"), value: Double(clip.fadeOut) / ClipEntry.flicksPerMillisecond, unit: "ms", minimum: 0,
                maximum: ClipEntry.maximumFadeMilliseconds) { setFade($0, entering: false) }
            Text(text("Drag the clip corners to change fades. Drag its gain handle vertically."))
                .font(.caption).foregroundStyle(.secondary)
        }.disabled(controller.busy || controller.faderPreview != nil)
    }
    private func setFade(_ milliseconds: Double, entering: Bool) {
        let flicks = Int64((milliseconds * ClipEntry.flicksPerMillisecond).rounded())
        let document = controller.documentID
        Task { await controller.setClipFades(clip.id, fadeIn: entering ? flicks : clip.fadeIn,
            fadeOut: entering ? clip.fadeOut : flicks, documentID: document) }
    }
}

/// Same fraction and colors as the arrange header meter. A system progress
/// view animates between values, so it cannot follow that bar.
private struct PeakMeter: View {
    let peak: Float
    var body: some View {
        Canvas { context, size in
            let bounds = CGRect(origin: .zero, size: size)
            context.fill(Path(bounds), with: .color(Color(red: 0.27, green: 0.29, blue: 0.33)))
            var fill = bounds
            fill.size.width *= MixScale.meter(peak)
            let color = peak >= 1
                ? Color(red: 1, green: 0.25, blue: 0.12)
                : Color(red: 0.28, green: 0.80, blue: 0.49)
            context.fill(Path(fill), with: .color(color))
        }
        .frame(height: 4)
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
        guard let parsed = MixEntry.number(draft, unit: unit, silence: silence),
              silence && parsed == -.infinity || parsed.isFinite && parsed >= minimum && parsed <= maximum else { invalid = true; return }
        invalid = false
        if abs(parsed - value) > 0.005 || parsed.isInfinite != value.isInfinite { commit(parsed) }
    }
}
