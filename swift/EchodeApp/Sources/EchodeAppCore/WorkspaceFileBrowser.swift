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
            if let folder = browser.folder {
                HStack {
                    Button { Task { await browser.load(folder.deletingLastPathComponent()) } } label: { Image(systemName: "arrow.up") }
                        .help(text("Parent Folder")).accessibilityLabel(text("Parent Folder")).disabled(browser.loading || !browser.canOpenParent)
                    Text(folder.lastPathComponent).font(.caption).lineLimit(1).help(folder.path)
                    Spacer()
                    Button { Task { await browser.load(folder) } } label: { Image(systemName: "arrow.clockwise") }
                        .help(text("Refresh")).accessibilityLabel(text("Refresh")).disabled(browser.loading)
                }
                TextField(text("Search files"), text: $browser.query).focusedValue(\.workspaceTextEntry, true)
            }
            if browser.loading { ProgressView().controlSize(.small) }
            if let error = browser.error { Text(error).font(.caption).foregroundStyle(.secondary) }
            if browser.folder == nil {
                Text(text("Choose a folder to browse projects")).font(.caption).foregroundStyle(.secondary)
                Spacer()
            } else {
                List(selection: $browser.selection) {
                    ForEach(browser.filtered) { file in
                        Label(file.url.lastPathComponent, systemImage: file.icon).tag(file.url).help(file.url.path)
                            .contentShape(Rectangle()).onTapGesture(count: 2) { open(file) }
                    }
                }.onKeyPress(.return) { openSelected(); return .handled }
                HStack {
                    Button(text("Open"), action: openSelected).disabled(!canOpenSelection)
                    Button(text("Import Audio"), action: importSelected).disabled(!canImportSelection)
                }
            }
        }.padding(12)
    }
    private var selected: BrowserFile? { browser.filtered.first { $0.url == browser.selection } }
    private var canOpenSelection: Bool { selected.map { ($0.directory || $0.project) && !browser.loading && !session.controller.busy } ?? false }
    private var canImportSelection: Bool {
        selected.map { $0.audio && !browser.loading && !session.controller.busy && session.controller.selection != nil } ?? false
    }
    private func openSelected() { if let selected { open(selected) } }
    private func importSelected() { if let selected { importAudio(selected) } }
    private func open(_ file: BrowserFile) {
        guard !browser.loading, !session.controller.busy else { return }
        if file.audio { importAudio(file) }
        else if file.project { Task { await session.controller.open(file.url) } }
        else if file.directory { Task { await browser.load(file.url) } }
    }
    private func importAudio(_ file: BrowserFile) {
        guard file.audio, !browser.loading, !session.controller.busy else { return }
        Task { await session.controller.importAudio(file.url) }
    }
}
