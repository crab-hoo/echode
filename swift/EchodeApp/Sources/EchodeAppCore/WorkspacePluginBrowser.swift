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
    @State private var scans = CoalescedReload()
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
                        HStack(spacing: 6) {
                            Text(plugin.name)
                            if plugin.fresh { Text(text("New plugin")).font(.caption).foregroundStyle(.secondary) }
                        }
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
        }.padding(20).frame(width: 460, height: 520).task {
            searching = true
            await reload()
            var seen = await Task.detached { PluginCatalog.wait(after: 0, timeout: 0) }.value
            while !Task.isCancelled {
                let current = seen
                let next = await Task.detached { PluginCatalog.wait(after: current, timeout: 0.5) }.value
                if Task.isCancelled { break }
                if next != seen {
                    seen = next
                    await reload()
                }
            }
        }
            .onChange(of: query) { _, _ in selected = retainedSelection(selected, in: filtered.map(\.id)) }
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
        guard let started = scans.begin() else { return }
        loading = true
        defer { loading = false }
        var current = started
        while !Task.isCancelled {
            let keep = selected
            do {
                let next = try await Task.detached {
                    try PluginCatalog.startWatch()
                    return try PluginCatalog.listings()
                }.value
                plugins = next
                selected = retainedSelection(keep, in: filtered.map(\.id))
                message = plugins.isEmpty ? text("No plugins found") : nil
            } catch {
                if plugins.isEmpty { selected = nil; message = text("Plugin scan failed") }
            }
            guard let again = scans.finish(current) else { return }
            current = again
        }
        scans.abandon()
    }
}

/// Joins catalog reloads. A request that arrives during a scan is read again
/// after that scan, instead of being dropped with the older result.
struct CoalescedReload {
    private var generation = 0
    private var active = 0

    mutating func begin() -> Int? {
        generation += 1
        guard active == 0 else { return nil }
        active = generation
        return generation
    }

    mutating func finish(_ started: Int) -> Int? {
        if generation != started {
            active = generation
            return generation
        }
        active = 0
        return nil
    }

    mutating func abandon() { active = 0 }
}
