import AppKit
import SwiftUI
import Observation
import UniformTypeIdentifiers

@MainActor
struct WorkspaceExportView: View {
    let controller: ProjectController
    let job: ExportJob
    let close: () -> Void
    private let document: DocumentID?
    private let project: URL?
    private let defaults: UserDefaults
    private let initialRate: UInt32
    @Environment(\.locale) private var locale
    @State private var draft: ExportSettingsDraft
    @MainActor @Observable
    fileprivate final class Presentation {
        var choosingOutput = false
        var loadingSettings = true
        var defaultsMessage: String?
    }
    @State private var presentation: Presentation

    init(controller: ProjectController, job: ExportJob, defaults: UserDefaults = .standard, close: @escaping () -> Void) {
        self.controller = controller; self.job = job; self.defaults = defaults; self.close = close
        document = controller.documentID; project = controller.projectURL
        let rate = controller.audio.activeDevice?.sampleRate ?? 48_000
        initialRate = rate
        let settings = controller.projectURL.map {
            ExportSettings.load(project: $0, defaults: defaults, initialRate: rate)
        } ?? ExportSettings.commonDefault(defaults: defaults, initialRate: rate)
        _draft = State(initialValue: ExportSettingsDraft(settings: settings))
        _presentation = State(initialValue: Presentation())
    }

    var validatedSettings: ExportSettings? { try? draft.resolve(locale: locale).get() }
    private var validationMessage: String? {
        guard case .failure(let error) = draft.resolve(locale: locale) else { return nil }
        return text(error.message)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text(text("Export Audio")).font(.headline)
                Spacer()
                if presentation.loadingSettings { ProgressView().controlSize(.small).accessibilityLabel(text("Loading export settings")) }
            }.frame(height: 20)
            Text(text("Whole song · Stereo WAV · Recorded mix and effects")).foregroundStyle(.secondary)
            Form {
                Picker(text("Format"), selection: $draft.options.format) {
                    ForEach(ExportSettings.Format.allCases, id: \.self) { Text(text($0.title)).tag($0) }
                }.accessibilityIdentifier("export.format")
                TextField(text("Sample rate (Hz)"), text: $draft.sampleRate)
                    .accessibilityIdentifier("export.rate")
                Toggle(text("Use plugin tail length"), isOn: $draft.options.automaticTail)
                TextField(text("Tail (seconds)"), text: $draft.tailSeconds)
                    .disabled(draft.options.automaticTail).accessibilityIdentifier("export.tail")
                Toggle(text("TPDF dither"), isOn: $draft.options.tpdf).disabled(draft.options.format == .float32)
                    .accessibilityIdentifier("export.dither")
                Toggle(text("Normalize sample peak"), isOn: Binding(
                    get: { draft.options.normalizePeakDb != nil },
                    set: { draft.options.normalizePeakDb = $0 ? -1 : nil }
                )).accessibilityIdentifier("export.normalize")
                TextField(text("Peak target (dBFS, −120…0)"), text: $draft.peakDb)
                    .disabled(draft.options.normalizePeakDb == nil).accessibilityIdentifier("export.peak")
            }.disabled(presentation.loadingSettings)
            if !presentation.loadingSettings, let validationMessage {
                Text(validationMessage).font(.caption).foregroundStyle(.red)
                    .accessibilityIdentifier("export.validation")
            }
            HStack {
                Button(text("Use Default"), action: useCommonDefault).accessibilityIdentifier("export.defaults.load")
                Button(text("Save as Default"), action: saveCommonDefault)
                    .disabled(validatedSettings == nil).accessibilityIdentifier("export.defaults.save")
                Button(text("Reset Default"), action: resetCommonDefault).accessibilityIdentifier("export.defaults.reset")
            }.controlSize(.small).disabled(!canChangeDefaults)
            Text(text(presentation.defaultsMessage ?? "Defaults apply on this Mac to projects without previous export settings."))
                .font(.caption).foregroundStyle(.secondary).accessibilityIdentifier("export.defaults.status")
            Text(text("Export uses a private snapshot. Editing and playback can continue after preparation."))
                .font(.caption).foregroundStyle(.secondary)
            HStack {
                Button(text("Cancel"), action: close).keyboardShortcut(.cancelAction)
                Spacer()
                Button(text("Export Audio"), action: chooseOutput).keyboardShortcut(.defaultAction)
                    .disabled(presentation.loadingSettings || validatedSettings == nil || presentation.choosingOutput || job.running || !controller.acceptsInput(document))
                    .accessibilityIdentifier("export.start")
            }
        }.padding(24).frame(width: 460)
            .onChange(of: draft.options.format) { _, format in if format == .float32 { draft.options.tpdf = false } }
            .interactiveDismissDisabled(presentation.choosingOutput)
            .task {
                guard let document, let loaded = await controller.loadExportSettings(documentID: document, defaults: defaults) else { return }
                draft.reset(settings: loaded, locale: locale)
                presentation.loadingSettings = false
            }
    }

    private var canChangeDefaults: Bool {
        !presentation.loadingSettings && !presentation.choosingOutput && controller.acceptsInput(document)
    }

    func useCommonDefault() {
        guard canChangeDefaults else { return }
        draft.reset(settings: .commonDefault(defaults: defaults, initialRate: initialRate), locale: locale)
        presentation.defaultsMessage = "Default export settings loaded."
    }

    func saveCommonDefault() {
        guard canChangeDefaults, let settings = validatedSettings, settings.saveAsDefault(defaults: defaults) else { return }
        presentation.defaultsMessage = "Default export settings saved on this Mac."
    }

    func resetCommonDefault() {
        guard canChangeDefaults else { return }
        ExportSettings.resetCommonDefault(defaults: defaults)
        draft.reset(settings: .factoryDefault(initialRate: initialRate), locale: locale)
        presentation.defaultsMessage = "Factory export defaults restored on this Mac."
    }

    private func chooseOutput() {
        guard let document, let project, let captured = validatedSettings else { return }
        let panel = NSSavePanel()
        panel.title = text("Export Audio"); panel.allowedContentTypes = [.wav]
        panel.nameFieldStringValue = project.deletingPathExtension().lastPathComponent + ".wav"
        panel.canCreateDirectories = true
        presentation.choosingOutput = true
        panel.begin { response in
            presentation.choosingOutput = false
            guard response == .OK, let output = panel.url else { return }
            close()
            job.start(controller: controller, document: document, project: project, output: output, settings: captured, defaults: defaults)
        }
    }
}

@MainActor
struct ExportStatusView: View {
    let job: ExportJob
    @State private var details = false
    var body: some View {
        if let message = job.message {
            HStack {
                if job.running {
                    ProgressView(value: job.fractionCompleted).frame(width: 90).controlSize(.small)
                        .accessibilityLabel(text("Export progress"))
                    if let fraction = job.fractionCompleted {
                        Text(fraction, format: .percent.precision(.fractionLength(0))).font(.caption.monospacedDigit())
                    }
                }
                Text(message).font(.caption)
                Spacer()
                if job.running {
                    Button(text("Cancel export")) { job.cancel() }
                        .disabled(job.cancelRequested).accessibilityIdentifier("export.cancel")
                }
                if !job.running, let output = job.completedOutput {
                    Button(text("Show in Finder")) { job.revealOutput() }
                        .help(output.path).accessibilityIdentifier("export.reveal")
                }
                if !job.detail.isEmpty {
                    Button(text("Details")) { details.toggle() }
                        .popover(isPresented: $details) {
                            ScrollView { Text(job.detail).font(.system(.caption, design: .monospaced)).textSelection(.enabled).padding() }
                                .frame(width: 520, height: 240)
                        }
                }
            }.padding(.horizontal, 10).padding(.vertical, 4)
        }
    }
}
