import AppKit
import EchodeAppCore
import SwiftUI

@main
struct EchodeApp: App {
    var body: some Scene {
        Window("Echode", id: "workspace") { ProjectWindow() }
            .defaultSize(width: 1200, height: 740)
            .windowResizability(.contentMinSize)
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
    @FocusedValue(\.workspaceTextEntry) private var textEntry
    var body: some Commands {
        CommandGroup(replacing: .newItem) { item(.newProject); item(.openProject) }
        CommandGroup(replacing: .undoRedo) { item(.undo); item(.redo) }
        CommandMenu(text("Track")) { item(.addTrack); item(.plugins) }
        CommandMenu(text("Playback")) { item(.playback); item(.stop); item(.output) }
        CommandGroup(after: .sidebar) { item(.inspector); item(.browser); item(.editor); item(.palette) }
    }
    private func item(_ command: WorkspaceCommand) -> some View {
        Button(command.title) {
            if textEntry == true, command == .undo { NSApp.keyWindow?.firstResponder?.undoManager?.undo() }
            else if textEntry == true, command == .redo { NSApp.keyWindow?.firstResponder?.undoManager?.redo() }
            else { actions?.perform(command) }
        }
            .keyboardShortcut(command.shortcut)
            // Text editing owns Space and its local undo stack.
            .disabled(textEntry == true && (command == .undo || command == .redo) ? false :
                actions?.enabled.contains(command) != true || (command == .playback && textEntry == true))
    }
}
