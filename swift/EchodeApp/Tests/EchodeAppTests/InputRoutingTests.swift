import AppKit
import Carbon.HIToolbox
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
            let event = try #require(NSEvent.keyEvent(with: .keyDown, location: .zero,
                modifierFlags: flags, timestamp: 0, windowNumber: 0, context: nil,
                characters: characters, charactersIgnoringModifiers: characters,
                isARepeat: false, keyCode: UInt16(code)))
            let before = receiver.events.count
            header.keyDown(with: event)
            #expect(receiver.events.count == before + 1)
            #expect(receiver.events.last === event)
        }
    }

    @Test func pianoRollForwardsEscapeWhenNothingIsBeingDragged() throws {
        let grid = PianoGridView(frame: .zero)
        let receiver = KeyReceiver()
        grid.nextResponder = receiver
        let event = try #require(NSEvent.keyEvent(with: .keyDown, location: .zero,
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
