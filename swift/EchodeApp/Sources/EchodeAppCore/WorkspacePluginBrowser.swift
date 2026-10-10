import EchodeBridge
import SwiftUI

struct WorkspacePluginBrowser: View {
    let controller: ProjectController
    let track: UUID
    let document: UUID?
    let close: () -> Void
    @State private var query = ""
    @State private var plugins: [PluginListing] = []
    @State private var selected: String?
    @State private var message: String?
    @State private var loading = false
    @FocusState private var searching: Bool
    private var filtered: [PluginListing] { plugins.filter { fuzzyMatch(query, in: "\($0.name) \($0.vendor) \($0.id)") } }
    private var targetExists: Bool { controller.documentID == document && controller.tracks.contains { $0.id == track } }
    private var canInsert: Bool { targetExists && !controller.busy && selected != nil && !loading }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(text("Plugin Browser")).font(.headline)
            LabeledContent(text("Track"), value: controller.tracks.first { $0.id == track }?.name ?? text("Select a track"))
            TextField(text("Search plugins"), text: $query).focused($searching).focusedValue(\.workspaceTextEntry, true)
                .onSubmit { insert(closeAfter: false) }.onKeyPress(.downArrow) { move(1); return .handled }.onKeyPress(.upArrow) { move(-1); return .handled }
            if loading { ProgressView().controlSize(.small) }
            if let message { Text(message).font(.caption).foregroundStyle(.secondary) }
            List(selection: $selected) {
                ForEach(filtered) { plugin in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(plugin.name)
                        Text([plugin.vendor, plugin.id].filter { !$0.isEmpty }.joined(separator: " · "))
                            .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }.tag(plugin.id).contentShape(Rectangle()).onTapGesture(count: 2) { selected = plugin.id; insert(closeAfter: true) }
                }
            }.onMoveCommand { if $0 == .down { move(1) }; if $0 == .up { move(-1) } }
                .onKeyPress(.return) { insert(closeAfter: false); return .handled }
            HStack {
                Button(text("Refresh")) { Task { await reload() } }.disabled(loading || controller.busy)
                Spacer()
                Button(text("Close"), action: close).keyboardShortcut(.cancelAction)
                Button(text("Insert")) { insert(closeAfter: true) }.disabled(!canInsert)
            }
        }.padding(20).frame(width: 460, height: 520).task { searching = true; await reload() }
            .onChange(of: query) { _, _ in selected = filtered.first?.id }
    }
    private func move(_ offset: Int) { selected = movedSelection(selected, in: filtered.map(\.id), offset: offset) }
    private func insert(closeAfter: Bool) {
        guard canInsert, let selected, filtered.contains(where: { $0.id == selected }) else { return }
        Task {
            guard controller.documentID == document else { return }
            if await controller.insertPlugin(selected, on: track, documentID: document) {
                if closeAfter { close() }
            } else {
                message = controller.errorMessage ?? text("Could not complete the operation")
                controller.errorMessage = nil
            }
        }
    }
    private func reload() async {
        guard !loading else { return }
        loading = true
        defer { loading = false }
        do {
            plugins = try await Task.detached { try PluginCatalog.listings() }.value
            selected = filtered.first?.id
            message = plugins.isEmpty ? text("No plugins found") : nil
        } catch { plugins = []; selected = nil; message = text("Plugin scan failed") }
    }
}
