import SwiftUI

struct WorkspaceFileBrowser: View {
    let session: WorkspaceSession
    private var browser: FileBrowserState { session.state.browser }
    var body: some View {
        @Bindable var browser = browser
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(text("Files")).font(.headline)
                Spacer()
                Button { session.chooseFolder() } label: { Image(systemName: "folder.badge.plus") }
                    .help(text("Choose Folder")).accessibilityLabel(text("Choose Folder")).disabled(session.state.choosingFile)
            }
            if !browser.roots.isEmpty {
                Menu(text("Sample Folders")) {
                    ForEach(browser.roots, id: \.self) { root in
                        Button(root.path) { Task { await browser.load(root) } }
                    }
                    Button(text("Refresh")) { Task { await browser.refresh() } }
                    Menu(text("Remove Sample Folder")) {
                        ForEach(browser.roots, id: \.self) { root in
                            Button(root.path) { Task { await browser.removeRoot(root) } }
                        }
                    }
                }
            }
            if let folder = browser.folder {
                HStack {
                    Button { Task { await browser.load(folder.deletingLastPathComponent()) } } label: { Image(systemName: "arrow.up") }
                        .help(text("Parent Folder")).accessibilityLabel(text("Parent Folder")).disabled(browser.loading || !browser.canOpenParent)
                    Text(folder.lastPathComponent).font(.caption).lineLimit(1).help(folder.path)
                    Spacer()
                    Button { Task { await browser.refresh() } } label: { Image(systemName: "arrow.clockwise") }
                        .help(text("Refresh")).accessibilityLabel(text("Refresh")).disabled(browser.loading)
                }
            }
            if browser.folder != nil || !browser.roots.isEmpty {
                TextField(text(browser.roots.isEmpty ? "Search files" : "Search all samples"), text: $browser.query)
                    .focusedValue(\.workspaceTextEntry, true)
            }
            if browser.loading || browser.indexing { ProgressView().controlSize(.small) }
            if let error = browser.error { Text(error).font(.caption).foregroundStyle(.secondary) }
            if let error = browser.libraryError {
                Text(text("Some sample folders could not be read")).font(.caption).foregroundStyle(.secondary).help(error)
            }
            if browser.folder == nil && !browser.searchingLibrary {
                Text(text("Choose a folder to browse projects")).font(.caption).foregroundStyle(.secondary)
                Spacer()
            } else {
                SampleFileList(files: browser.filtered, searching: browser.searchingLibrary, selection: browser.selection,
                               onSelect: { session.chooseInBrowser($0, origin: $1) }, onOpen: open,
                               onAudition: { session.auditionSelected(origin: $0) })
                if let selected, selected.audio {
                    SamplePreview(url: selected.url, status: session.controller.audio.audition,
                                  onSeek: { frame, origin in session.auditionSelected(origin: origin, from: frame, expected: selected.url) })
                }
                SampleAuditionControls(preferences: session.state.auditionPreferences,
                    canPlay: selected?.audio == true && session.controller.document != nil,
                    onAutomatic: session.setAutomaticAudition, onGain: session.setAuditionGain,
                    onPlay: { session.auditionSelected() }, onStop: { session.controller.audition(nil) })
                if let error = session.controller.audio.audition?.outputError {
                    Text(text("Sample output unavailable")).font(.caption).foregroundStyle(.secondary).help(error)
                }
                HStack {
                    Button(text("Open"), action: openSelected).disabled(!canOpenSelection)
                    Button(text("Import Audio"), action: importSelected).disabled(!canImportSelection)
                }
            }
        }.padding(12).task { await browser.prepare() }
    }
    private var selected: BrowserFile? { browser.filtered.first { $0.url == browser.selection } }
    private var canOpenSelection: Bool { selected.map { ($0.directory || $0.project) && !browser.loading && !session.controller.busy } ?? false }
    private var canImportSelection: Bool {
        selected.map { $0.audio && !browser.loading && !session.controller.busy && session.controller.selection != nil } ?? false
    }
    private func openSelected() { if let selected { open(selected) } }
    private func importSelected() { if let selected { session.importSample(selected) } }
    private func open(_ file: BrowserFile) { session.openInBrowser(file) }
}

extension WorkspaceSession {
    /// A double click or Return on a row: places a sample, opens a project
    /// or enters a folder. The returned task finishes with that work.
    @discardableResult
    func openInBrowser(_ file: BrowserFile) -> Task<Void, Never>? {
        let browser = state.browser
        guard !browser.loading, !controller.busy else { return nil }
        if file.audio {
            guard let pending = importSample(file) else { return nil }
            return Task { _ = await pending.value }
        }
        if file.project { return Task { await controller.open(file.url) } }
        if file.directory { return Task { await browser.load(file.url) } }
        return nil
    }
}
