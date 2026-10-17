import SwiftUI

struct WorkspacePalette: View {
    let session: WorkspaceSession
    @State private var query = ""
    @State private var selected: WorkspaceCommand?
    @FocusState private var searching: Bool
    private var commands: [WorkspaceCommand] { WorkspaceCommand.allCases.filter { $0 != .palette && fuzzyMatch(query, in: $0.title) } }
    private var available: [WorkspaceCommand] { commands.filter { session.enabled($0, fromPalette: true) } }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(text("Command Palette")).font(.headline)
            TextField(text("Search commands"), text: $query).focused($searching).focusedValue(\.workspaceTextEntry, true)
                .onSubmit(run).onKeyPress(.downArrow) { move(1); return .handled }.onKeyPress(.upArrow) { move(-1); return .handled }
            List(selection: $selected) {
                ForEach(commands) { command in
                    HStack { Label(command.title, systemImage: command.icon); Spacer(); Text(command.shortcutLabel).foregroundStyle(.secondary) }
                        .tag(command).contentShape(Rectangle())
                        .foregroundStyle(session.enabled(command, fromPalette: true) ? .primary : .secondary)
                        .onTapGesture(count: 2) { selected = command; run() }
                }
            }.onMoveCommand { if $0 == .down { move(1) }; if $0 == .up { move(-1) } }
                .onKeyPress(.return) { run(); return .handled }
            HStack {
                Button(text("Close")) { session.state.presentation = nil }.keyboardShortcut(.cancelAction)
                Spacer()
                Button(text("Run"), action: run).disabled(selected.map { !session.enabled($0, fromPalette: true) } ?? true)
            }
        }.padding(20).frame(width: 460, height: 460)
            .onAppear { searching = true; selected = available.first }
            .onChange(of: query) { _, _ in selected = retainedSelection(selected, in: available) }
    }
    private func move(_ offset: Int) { selected = movedSelection(selected, in: available, offset: offset) }
    private func run() {
        guard let selected, session.enabled(selected, fromPalette: true) else { return }
        session.state.presentation = nil
        session.perform(selected)
    }
}

func movedSelection<Value: Equatable>(_ selected: Value?, in items: [Value], offset: Int) -> Value? {
    guard !items.isEmpty else { return nil }
    guard let selected, let index = items.firstIndex(of: selected) else { return offset < 0 ? items.last : items.first }
    return items[min(items.count - 1, max(0, index + offset))]
}

func retainedSelection<Value: Equatable>(_ selected: Value?, in items: [Value]) -> Value? {
    if let selected, items.contains(selected) { selected } else { items.first }
}
