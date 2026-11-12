import AppKit
import EchodeBridge
import Testing

@testable import EchodeAppCore

@MainActor
struct ClipDuplicationBoundaryTests {
    private func key(flags: NSEvent.ModifierFlags = .command, repeated: Bool = false) throws -> NSEvent {
        try #require(
            NSEvent.keyEvent(
                with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0,
                windowNumber: 0, context: nil, characters: "d", charactersIgnoringModifiers: "d", isARepeat: repeated,
                keyCode: 2))
    }

    @Test func contractCommandDStaysLocalAndRejectsUnavailableOrUncommittedTargets() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(
            "echode-clip-copy-keys-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "notes")
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 1))
        let track = try #require(controller.selection)
        let snapshot = try #require(controller.snapshot)
        let note = try #require(snapshot.pianoNeighbor(track: track, current: nil))
        let clip = try #require(snapshot.clipMix(note.id.placement))
        let view = ArrangeView(frame: CGRect(x: 0, y: 0, width: 900, height: 200))
        let window = NSWindow(contentRect: view.bounds, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        let root = NSView(frame: view.bounds)
        let field = NSTextField(frame: CGRect(x: 0, y: 0, width: 80, height: 24))
        root.addSubview(view)
        root.addSubview(field)
        // Never order this input-only window on screen or open an audio device.
        window.contentView = root
        view.stop()
        defer {
            view.stop()
            window.contentView = nil
            window.close()
        }
        view.audioDocument = controller.documentID
        view.setSnapshot(snapshot, selected: track)
        view.setControlSelection(clip: clip.id, preview: nil)
        view.setRuler(start: 0, loop: nil, busy: false)
        var requests: [ClipMove] = []
        view.onClipMove = { requests.append($0) }
        #expect(window.makeFirstResponder(field))
        #expect(!view.performKeyEquivalent(with: try key()))
        view.keyDown(with: try key())
        #expect(requests.isEmpty, "text entry must not duplicate the arrange's selected clip")
        #expect(window.makeFirstResponder(view))
        #expect(view.performKeyEquivalent(with: try key()))
        try #require(requests.count == 1, "the focused arrange must handle its native Command-D equivalent")
        #expect(requests[0].clip == clip.id && requests[0].start == clip.start + clip.length && requests[0].copy)
        for flags: NSEvent.ModifierFlags in [
            [], .shift, [.command, .shift], [.command, .option], [.command, .control],
        ] {
            view.keyDown(with: try key(flags: flags))
        }
        view.keyDown(with: try key(repeated: true))
        view.setRuler(start: 0, loop: nil, busy: true)
        view.keyDown(with: try key())
        view.setRuler(start: 0, loop: nil, busy: false)
        view.setControlSelection(clip: nil, preview: nil)
        view.keyDown(with: try key())
        view.setControlSelection(clip: UUID(), preview: nil)
        view.keyDown(with: try key())
        view.setControlSelection(clip: clip.id, preview: nil)
        view.audioDocument = nil
        view.keyDown(with: try key())
        view.audioDocument = controller.documentID
        view.setSnapshot(snapshot, selected: UUID())
        view.keyDown(with: try key())
        view.setSnapshot(snapshot, selected: track)
        view.pressClip(clip, at: CGPoint(x: 200, y: 60))
        view.keyDown(with: try key())
        view.finishClipMove(at: nil, flags: [])
        #expect(view.pressRuler(at: CGPoint(x: 200, y: 12)))
        view.keyDown(with: try key())
        view.cancelRuler()
        #expect(requests.count == 1, "unavailable targets, held keys and uncommitted gestures must not copy a clip")
        #expect(controller.undoCount == 2 && controller.redoCount == 0)
        await controller.close()
    }

    @Test func adjacentWindowsUseCheckedIntegerTimeAndNeverRound() {
        #expect(ClipMoveTarget.adjacentStart(start: 17, length: 3) == 20)
        #expect(ClipMoveTarget.adjacentStart(start: 0, length: 1) == 1)
        #expect(ClipMoveTarget.adjacentStart(start: Int64.max - 2, length: 1) == Int64.max - 1)
        #expect(ClipMoveTarget.adjacentStart(start: Int64.max - 1, length: 1) == nil)
        #expect(ClipMoveTarget.adjacentStart(start: Int64.max, length: 1) == nil)
        #expect(ClipMoveTarget.adjacentStart(start: 0, length: Int64.max) == nil)
        #expect(ClipMoveTarget.adjacentStart(start: -1, length: 1) == nil)
        #expect(ClipMoveTarget.adjacentStart(start: 0, length: 0) == nil)
        #expect(ClipMoveTarget.adjacentStart(start: 0, length: -1) == nil)
    }
}
