import EchodeAppCore
import SwiftUI

@main
struct EchodeApp: App {
    var body: some Scene {
        Window("Echode", id: "workspace") { ProjectWindow() }
            .defaultSize(width: 1200, height: 740)
            .commands { WorkspaceCommands() }
    }
}

struct ProjectWindow: View {
    @State private var controller = ProjectController()
    @State private var pendingOpen: URL?
    var body: some View {
        WorkspaceView(controller: controller)
            .onOpenURL { url in
                if controller.busy { pendingOpen = url }
                else { Task { await controller.open(url) } }
            }
            .onChange(of: controller.busy) { _, busy in
                if !busy, let url = pendingOpen {
                    pendingOpen = nil
                    Task { await controller.open(url) }
                }
            }
            .task {
                if let path = UserDefaults.standard.string(forKey: "lastProjectPath") {
                    await controller.open(URL(fileURLWithPath: path))
                }
            }
            .onChange(of: controller.projectURL) { _, url in
                if let url { UserDefaults.standard.set(url.path, forKey: "lastProjectPath") }
            }
            .onDisappear {
                Task {
                    while controller.busy || controller.audioBusy {
                        try? await Task.sleep(for: .milliseconds(10))
                    }
                    await controller.close()
                }
            }
    }
}

struct WorkspaceCommands: Commands {
    @FocusedValue(\.workspaceActions) private var actions
    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button(text("New Project")) { actions?.create() }.keyboardShortcut("n").disabled(actions?.canOpen != true)
            Button(text("Open Project")) { actions?.open() }.keyboardShortcut("o").disabled(actions?.canOpen != true)
        }
        CommandGroup(replacing: .undoRedo) {
            Button(text("Undo")) { actions?.undo() }.keyboardShortcut("z").disabled(actions?.canUndo != true)
            Button(text("Redo")) { actions?.redo() }.keyboardShortcut("z", modifiers: [.command, .shift]).disabled(actions?.canRedo != true)
        }
        CommandMenu(text("Track")) {
            Button(text("Add Track")) { actions?.addTrack() }.keyboardShortcut("t").disabled(actions?.canEdit != true)
        }
        CommandMenu(text("Playback")) {
            Button(text("Play / Pause")) { actions?.playback() }.keyboardShortcut(.space, modifiers: []).disabled(actions?.canPlay != true)
            Button(text("Stop")) { actions?.stop() }.keyboardShortcut(".", modifiers: .command).disabled(actions?.canPlay != true)
        }
        CommandGroup(after: .sidebar) {
            Button(text("Toggle Inspector")) { actions?.inspector() }.keyboardShortcut("1", modifiers: [.command, .option])
            Button(text("Toggle Files")) { actions?.browser() }.keyboardShortcut("2", modifiers: [.command, .option])
            Button(text("Toggle Note Editor")) { actions?.editor() }.keyboardShortcut("3", modifiers: [.command, .option])
            Button(text("Command Palette")) { actions?.palette() }.keyboardShortcut("p", modifiers: [.command, .shift])
        }
    }
}
