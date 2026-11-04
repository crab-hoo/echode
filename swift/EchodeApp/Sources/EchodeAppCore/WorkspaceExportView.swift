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
    @State private var settings: ExportSettings
    @State private var choosingOutput = false
    @State private var loadingSettings = true

    init(controller: ProjectController, job: ExportJob, close: @escaping () -> Void) {
        self.controller = controller; self.job = job; self.close = close
        document = controller.documentID; project = controller.projectURL
        _settings = State(initialValue: controller.projectURL.map {
            ExportSettings.load(project: $0, initialRate: controller.audio.activeDevice?.sampleRate ?? 48_000)
        } ?? ExportSettings())
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
                Picker(text("Format"), selection: $settings.format) {
                    ForEach(ExportSettings.Format.allCases, id: \.self) { Text(text($0.title)).tag($0) }
                }.accessibilityIdentifier("export.format")
                TextField(text("Sample rate (Hz)"), value: $settings.sampleRate, format: .number.grouping(.never))
                    .accessibilityIdentifier("export.rate")
                Toggle(text("Use plugin tail length"), isOn: $settings.automaticTail)
                TextField(text("Tail (seconds)"), value: $settings.tailSeconds, format: .number)
                    .disabled(settings.automaticTail).accessibilityIdentifier("export.tail")
                Toggle(text("TPDF dither"), isOn: $settings.tpdf).disabled(settings.format == .float32)
                    .accessibilityIdentifier("export.dither")
                Toggle(text("Normalize sample peak"), isOn: Binding(
                    get: { settings.normalizePeakDb != nil },
                    set: { settings.normalizePeakDb = $0 ? -1 : nil }
                )).accessibilityIdentifier("export.normalize")
                TextField(text("Peak target (dBFS, −120…0)"), value: Binding(
                    get: { settings.normalizePeakDb ?? -1 },
                    set: { settings.normalizePeakDb = $0 }
                ), format: .number)
                    .disabled(settings.normalizePeakDb == nil).accessibilityIdentifier("export.peak")
            }.disabled(loadingSettings)
            Text(text("Export uses a private snapshot. Editing and playback can continue after preparation."))
                .font(.caption).foregroundStyle(.secondary)
            HStack {
                Button(text("Cancel"), action: close).keyboardShortcut(.cancelAction)
                Spacer()
                Button(text("Export Audio"), action: chooseOutput).keyboardShortcut(.defaultAction)
                    .disabled(loadingSettings || !settings.isValid || choosingOutput || job.running || !controller.acceptsInput(document))
                    .accessibilityIdentifier("export.start")
            }
        }.padding(24).frame(width: 460)
            .onChange(of: settings.format) { _, format in if format == .float32 { settings.tpdf = false } }
            .interactiveDismissDisabled(choosingOutput)
            .task {
                guard let document, let loaded = await controller.loadExportSettings(documentID: document) else { return }
                settings = loaded
                loadingSettings = false
            }
    }

    private func chooseOutput() {
        guard let document, let project, settings.isValid else { return }
        let captured = settings
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
