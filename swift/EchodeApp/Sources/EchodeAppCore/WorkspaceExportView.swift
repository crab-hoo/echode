import AppKit
import SwiftUI
import UniformTypeIdentifiers

@MainActor
struct WorkspaceExportView: View {
    let controller: ProjectController
    let job: ExportJob
    let close: () -> Void
    private let document: DocumentID?
    private let project: URL?
    @Environment(\.locale) private var locale
    @State private var draft: ExportSettingsDraft
    @State private var choosingOutput = false
    @State private var loadingSettings = true

    init(controller: ProjectController, job: ExportJob, close: @escaping () -> Void) {
        self.controller = controller; self.job = job; self.close = close
        document = controller.documentID; project = controller.projectURL
        let settings = controller.projectURL.map {
            ExportSettings.load(project: $0, initialRate: controller.audio.activeDevice?.sampleRate ?? 48_000)
        } ?? ExportSettings()
        _draft = State(initialValue: ExportSettingsDraft(settings: settings))
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
                if loadingSettings { ProgressView().controlSize(.small).accessibilityLabel(text("Loading export settings")) }
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
            }.disabled(loadingSettings)
            if !loadingSettings, let validationMessage {
                Text(validationMessage).font(.caption).foregroundStyle(.red)
                    .accessibilityIdentifier("export.validation")
            }
            Text(text("Export uses a private snapshot. Editing and playback can continue after preparation."))
                .font(.caption).foregroundStyle(.secondary)
            HStack {
                Button(text("Cancel"), action: close).keyboardShortcut(.cancelAction)
                Spacer()
                Button(text("Export Audio"), action: chooseOutput).keyboardShortcut(.defaultAction)
                    .disabled(loadingSettings || validatedSettings == nil || choosingOutput || job.running || !controller.acceptsInput(document))
                    .accessibilityIdentifier("export.start")
            }
        }.padding(24).frame(width: 460)
            .onChange(of: draft.options.format) { _, format in if format == .float32 { draft.options.tpdf = false } }
            .interactiveDismissDisabled(choosingOutput)
            .task {
                guard let document, let loaded = await controller.loadExportSettings(documentID: document) else { return }
                draft.reset(settings: loaded, locale: locale)
                loadingSettings = false
            }
    }

    private func chooseOutput() {
        guard let document, let project, let captured = validatedSettings else { return }
        let panel = NSSavePanel()
        panel.title = text("Export Audio"); panel.allowedContentTypes = [.wav]
        panel.nameFieldStringValue = project.deletingPathExtension().lastPathComponent + ".wav"
        panel.canCreateDirectories = true
        choosingOutput = true
        panel.begin { response in
            choosingOutput = false
            guard response == .OK, let output = panel.url else { return }
            close()
            job.start(controller: controller, document: document, project: project, output: output, settings: captured)
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
