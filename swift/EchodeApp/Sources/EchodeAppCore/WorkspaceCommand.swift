import AppKit
import SwiftUI
import UniformTypeIdentifiers

public enum WorkspaceCommand: String, CaseIterable, Identifiable {
    case newProject = "New Project", openProject = "Open Project", addTrack = "Add Track"
    case undo = "Undo", redo = "Redo", history = "History", inspector = "Toggle Inspector", browser = "Toggle Files", editor = "Toggle Note Editor"
    case playback = "Play / Pause", stop = "Stop", loop = "Loop", metronome = "Metronome", plugins = "Plugins", output = "Audio Output", palette = "Command Palette"
    public var id: Self { self }
    public var title: String { text(rawValue) }
    public var icon: String {
        switch self {
        case .newProject: "doc.badge.plus"
        case .openProject: "folder"
        case .addTrack: "plus"
        case .undo: "arrow.uturn.backward"
        case .redo: "arrow.uturn.forward"
        case .history: "clock.arrow.circlepath"
        case .inspector: "sidebar.left"
        case .browser: "sidebar.right"
        case .editor: "pianokeys"
        case .playback: "playpause"
        case .stop: "stop.fill"
        case .loop: "repeat"
        case .metronome: "metronome"
        case .plugins: "puzzlepiece.extension"
        case .output: "speaker.wave.2"
        case .palette: "command"
        }
    }
    public var shortcut: KeyboardShortcut? {
        switch self {
        case .newProject: KeyboardShortcut("n")
        case .openProject: KeyboardShortcut("o")
        case .addTrack: KeyboardShortcut("t")
        case .undo: KeyboardShortcut("z")
        case .redo: KeyboardShortcut("z", modifiers: [.command, .shift])
        case .inspector: KeyboardShortcut("1", modifiers: [.command, .option])
        case .browser: KeyboardShortcut("2", modifiers: [.command, .option])
        case .editor: KeyboardShortcut("3", modifiers: [.command, .option])
        case .playback: KeyboardShortcut(.space, modifiers: [])
        case .stop: KeyboardShortcut(".")
        case .palette: KeyboardShortcut("p", modifiers: [.command, .shift])
        default: nil
        }
    }
    var shortcutLabel: String {
        switch self {
        case .newProject: "⌘N"
        case .openProject: "⌘O"
        case .addTrack: "⌘T"
        case .undo: "⌘Z"
        case .redo: "⇧⌘Z"
        case .inspector: "⌥⌘1"
        case .browser: "⌥⌘2"
        case .editor: "⌥⌘3"
        case .playback: text("Space")
        case .stop: "⌘."
        case .palette: "⇧⌘P"
        default: ""
        }
    }
}

/// Menus, the toolbar and the palette share capability checks and routing.
/// Controllers own durable edits; this layer owns window presentation only.
@MainActor
final class WorkspaceSession {
    let controller: ProjectController
    let state: WorkspaceState
    init(controller: ProjectController, state: WorkspaceState) { self.controller = controller; self.state = state }

    func enabled(_ command: WorkspaceCommand, fromPalette: Bool = false) -> Bool {
        guard !state.choosingFile, state.presentation == nil || (fromPalette && state.presentation == .palette) else { return false }
        let editable = controller.snapshot != nil && !controller.busy
        switch command {
        case .newProject, .openProject: return !controller.busy && !controller.audioBusy
        case .undo: return editable && controller.undoCount > 0
        case .redo: return editable && controller.redoCount > 0
        case .history: return editable
        case .addTrack: return editable
        case .plugins: return editable && controller.selection != nil
        case .playback, .stop: return editable
        case .loop, .metronome, .output: return editable && !controller.audioBusy
        default: return true
        }
    }

    func perform(_ command: WorkspaceCommand) {
        guard enabled(command) else { return }
        switch command {
        case .newProject: chooseProject(creating: true)
        case .openProject: chooseProject(creating: false)
        case .addTrack:
            Task { await controller.addTrack(name: "\(text("Track")) \(controller.tracks.count + 1)") }
        case .undo: Task { await controller.undo() }
        case .redo: Task { await controller.redo() }
        case .history: state.presentation = .history
        case .playback: Task { await controller.togglePlayback() }
        case .stop: Task { await controller.stopPlayback() }
        case .loop: Task { await controller.toggleLoop() }
        case .metronome: Task { await controller.toggleMetronome() }
        case .inspector: state.toggle(.inspector)
        case .browser: state.toggle(.browser)
        case .editor: state.toggle(.editor)
        case .plugins: if let track = controller.selection, let document = controller.documentID { state.presentation = .plugins(track: track, document: document) }
        case .output: state.presentation = .output
        case .palette: state.presentation = .palette
        }
    }

    func chooseFolder() {
        guard !state.choosingFile else { return }
        state.choosingFile = true
        let panel = NSOpenPanel()
        panel.title = text("Choose Folder")
        panel.canChooseDirectories = true; panel.canChooseFiles = false
        panel.begin { [self] response in
            state.choosingFile = false
            guard response == .OK, let url = panel.url else { return }
            Task { await state.browser.load(url) }
        }
    }

    private func chooseProject(creating: Bool) {
        state.choosingFile = true
        let panel: NSSavePanel
        if creating {
            let save = NSSavePanel()
            save.nameFieldStringValue = "Untitled.echode"
            save.allowedContentTypes = [UTType(exportedAs: "io.echode.project", conformingTo: .package)]
            save.canCreateDirectories = true
            panel = save
        } else {
            let open = NSOpenPanel()
            open.canChooseDirectories = true; open.canChooseFiles = true; open.allowsMultipleSelection = false
            panel = open
        }
        panel.title = text(creating ? "New Project" : "Open Project")
        panel.begin { [self] response in
            guard response == .OK, let url = panel.url else { state.choosingFile = false; return }
            Task {
                await controller.open(url, creating: creating)
                state.choosingFile = false
            }
        }
    }
}

public struct WorkspaceActions {
    public let perform: (WorkspaceCommand) -> Void
    public let enabled: Set<WorkspaceCommand>
}
private struct WorkspaceActionsKey: FocusedValueKey { typealias Value = WorkspaceActions }
private struct WorkspaceTextEntryKey: FocusedValueKey { typealias Value = Bool }
extension FocusedValues {
    public var workspaceActions: WorkspaceActions? {
        get { self[WorkspaceActionsKey.self] }
        set { self[WorkspaceActionsKey.self] = newValue }
    }
    public var workspaceTextEntry: Bool? {
        get { self[WorkspaceTextEntryKey.self] }
        set { self[WorkspaceTextEntryKey.self] = newValue }
    }
}

public func text(_ key: String) -> String { String(localized: String.LocalizationValue(key), bundle: .module) }
func noteName(_ pitch: Int) -> String {
    guard (0...127).contains(pitch) else { return String(pitch) }
    let names = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"]
    return "\(names[pitch % 12])\(pitch / 12 - 1)"
}
func fuzzyMatch(_ query: String, in title: String) -> Bool {
    var remaining = Array(query.lowercased().filter { !$0.isWhitespace })[...]
    for letter in title.lowercased() where letter == remaining.first { remaining = remaining.dropFirst() }
    return remaining.isEmpty
}
