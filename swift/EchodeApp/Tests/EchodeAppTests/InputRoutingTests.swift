import AppKit
import Carbon.HIToolbox
import EchodeBridge
import Foundation
import Synchronization
import Testing

@testable import EchodeAppCore

@MainActor
struct InputRoutingTests {
    @Test func contractHeaderForwardsSpaceAndUnownedKeys() throws {
        let header = ArrangeHeaderView(frame: .zero)
        let receiver = KeyReceiver()
        header.nextResponder = receiver
        // Exercise the native event entry point, not a selection/action helper.
        for (code, characters, flags) in [
            (kVK_Space, " ", NSEvent.ModifierFlags()),
            (kVK_ANSI_Z, "z", NSEvent.ModifierFlags.command),
        ] {
            let event = try #require(
                NSEvent.keyEvent(
                    with: .keyDown, location: .zero,
                    modifierFlags: flags, timestamp: 0, windowNumber: 0, context: nil,
                    characters: characters, charactersIgnoringModifiers: characters,
                    isARepeat: false, keyCode: UInt16(code)))
            let before = receiver.events.count
            header.keyDown(with: event)
            #expect(receiver.events.count == before + 1)
            #expect(receiver.events.last === event)
        }
    }

    /// With a sample row focused, Space plays and stops the song through the
    /// workspace session. It does not move the selection or audition (ADR 0160).
    /// Play is answered by a stand-in so no output opens.
    @Test func contractSampleListSpaceTogglesSongPlaybackAndKeepsTheSelection() async throws {
        let sent = Mutex<[String]>([])
        let controller = ProjectController(
            sendAudio: { document, command in
                switch command {
                case .play:
                    sent.withLock { $0.append("play") }
                    return try JSONDecoder().decode(
                        AudioState.self,
                        from: Data(
                            """
                            {"playing":true,"running":false,"opening":false,"tick":0,"devices":[],"metronome":false}
                            """.utf8))
                case .stop: sent.withLock { $0.append("stop") }
                default: break
                }
                return try await document.audio(command)
            }, sendAudition: { _, url, _ in sent.withLock { $0.append("audition \(url?.lastPathComponent ?? "-")") } })
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-list-space-\(UUID()).echode")
        defer { removeTestFile(url) }
        await controller.open(url, creating: true)
        let session = WorkspaceSession(controller: controller, state: WorkspaceState(defaults: .init()))
        let list = FileListView()
        let files = ["kick 01.wav", "kick 02.wav", "snare.wav"].map {
            BrowserFile(url: URL(fileURLWithPath: "/samples/\($0)"), directory: false)
        }
        list.show(files, searching: false, selection: files[0].url)
        var chosen: [URL?] = []
        list.onSelect = { url, _ in chosen.append(url) }
        list.onAudition = { _ in sent.withLock { $0.append("audition") } }
        var played: [Task<Void, Never>] = []
        // As `SampleFileList` wires it in the workspace.
        list.table.onPlayback = { played.append(Task { await controller.togglePlayback() }) }
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 240, height: 200), styleMask: [.titled], backing: .buffered,
            defer: true)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        list.frame = NSRect(x: 0, y: 0, width: 240, height: 200)
        window.contentView = list
        #expect(window.makeFirstResponder(list.table))
        func space(repeating: Bool = false) throws -> NSEvent {
            try #require(
                NSEvent.keyEvent(
                    with: .keyDown, location: .zero,
                    modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil,
                    characters: " ", charactersIgnoringModifiers: " ",
                    isARepeat: repeating, keyCode: UInt16(kVK_Space)))
        }
        list.table.keyDown(with: try space())
        for task in played { await task.value }
        #expect(controller.playbackRequested && session.enabled(.playback))
        list.table.keyDown(with: try space(repeating: true))  // a held key toggles once
        list.table.keyDown(with: try space())
        for task in played { await task.value }
        #expect(sent.withLock { $0 } == ["play", "stop"])
        #expect(!controller.playbackRequested)
        #expect(list.table.selectedRow == 0 && chosen.isEmpty)
        await controller.close()
    }

    @Test func pianoRollForwardsEscapeWhenNothingIsBeingDragged() throws {
        let grid = PianoGridView(frame: .zero)
        let receiver = KeyReceiver()
        grid.nextResponder = receiver
        let event = try #require(
            NSEvent.keyEvent(
                with: .keyDown, location: .zero,
                modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil,
                characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}",
                isARepeat: false, keyCode: UInt16(kVK_Escape)))
        grid.keyDown(with: event)
        #expect(receiver.events.count == 1)
        #expect(receiver.events.last === event)
    }
}

@MainActor
private final class KeyReceiver: NSResponder {
    var events: [NSEvent] = []
    override func keyDown(with event: NSEvent) { events.append(event) }
}
