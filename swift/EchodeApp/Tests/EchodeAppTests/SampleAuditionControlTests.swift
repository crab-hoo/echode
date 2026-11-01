import AppKit
import EchodeBridge
import Foundation
import Synchronization
import Testing
@testable import EchodeAppCore

@MainActor
struct SampleAuditionControlTests {
    @Test func contractManualAuditionAndVolumeWorkWithAutomaticAuditionOffWithoutEditing() async throws {
        let root = FileManager.default.temporaryDirectory.resolvingSymlinksInPath().appendingPathComponent("echode-audition-controls-\(UUID())")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { removeTestFile(root) }
        for name in ["kick.wav", "snare.wav", "song.mid"] { try Data().write(to: root.appendingPathComponent(name)) }
        let defaultsName = "echode-audition-controls-\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: defaultsName))
        defer { defaults.removePersistentDomain(forName: defaultsName) }
        let requests = Mutex<[(URL?, AuditionOrigin)]>([])
        let settings = Mutex<[(Bool, Float)]>([])
        let seeks = Mutex<[(URL, UInt64, AuditionOrigin)]>([])
        let controller = ProjectController(sendAudition: { _, url, origin in requests.withLock { $0.append((url, origin)) } })
        controller.sendAuditionSettings = { _, visible, gain in settings.withLock { $0.append((visible, gain)) } }
        controller.sendAuditionAt = { _, url, frame, origin in seeks.withLock { $0.append((url, frame, origin)) } }
        let state = WorkspaceState(defaults: defaults)
        let session = WorkspaceSession(controller: controller, state: state)
        await state.browser.load(root)
        await controller.open(root.appendingPathComponent("Song.echode"), creating: true)
        let undo = controller.undoCount
        state.layout.browser = true
        session.synchronizeAuditionOutput()
        #expect(settings.withLock { $0.count == 1 && $0[0].0 && $0[0].1 == 1 })
        #expect(requests.withLock { $0.isEmpty })

        let controls = AuditionControlsView()
        controls.onAutomatic = session.setAutomaticAudition
        controls.onGain = session.setAuditionGain
        controls.onPlay = { session.auditionSelected() }
        controls.onStop = { controller.audition(nil) }
        controls.show(preferences: state.auditionPreferences, canPlay: true)
        controls.automatic.performClick(nil)
        #expect(!state.auditionPreferences.automatic)
        let kick = try #require(state.browser.filtered.first { $0.url.lastPathComponent == "kick.wav" }?.url)
        let snare = try #require(state.browser.filtered.first { $0.url.lastPathComponent == "snare.wav" }?.url)
        #expect(session.chooseInBrowser(kick, origin: .other) == nil)
        #expect(state.browser.selection == kick)
        #expect(requests.withLock { $0.allSatisfy { $0.0 == nil } })
        requests.withLock { $0 = [] }

        let list = FileListView()
        list.onSelect = { session.chooseInBrowser($0, origin: $1) }
        list.onAudition = { session.auditionSelected(origin: $0) }
        list.show(state.browser.filtered, searching: false, selection: kick)
        let right = try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: 0, context: nil,
            characters: "\u{F703}", charactersIgnoringModifiers: "\u{F703}", isARepeat: false, keyCode: 124))
        list.table.keyDown(with: right)
        #expect(requests.withLock { $0.count == 1 && $0[0].0 == kick && $0[0].1.input == .key })
        #expect(state.browser.selection == kick)
        controls.play.performClick(nil)
        #expect(requests.withLock { $0.count == 2 && $0[1].0 == kick })
        controls.volume.floatValue = 0.25
        let action = try #require(controls.volume.action)
        let sent = controls.volume.sendAction(action, to: controls.volume.target)
        #expect(sent)
        #expect(state.auditionPreferences.gain == 0.25)
        #expect(settings.withLock { $0.last?.1 == 0.25 })
        #expect(requests.withLock { $0.count == 2 })

        let preview = SamplePreviewView()
        preview.frame = NSRect(x: 0, y: 0, width: 200, height: 86)
        let json = #"""
        {"request":1,"handled":1,"path":"\#(kick.path)","phase":"sounding","history":[],
         "waveform":{"frames":48000,"loaded_frames":24000,"sample_rate":48000,"frames_per_peak":12000,
                     "complete":false,"peaks":[[-0.5,1],[-0.25,0.5]]}}
        """#
        let status = try JSONDecoder().decode(AuditionStatus.self, from: Data(json.utf8))
        preview.show(url: kick, status: status)
        preview.onSeek = { frame, origin in session.auditionSelected(origin: origin, from: frame, expected: kick) }
        func click(_ point: NSPoint) throws {
            preview.mouseDown(with: try #require(NSEvent.mouseEvent(with: .leftMouseDown, location: preview.convert(point, to: nil),
                modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: 0, context: nil,
                eventNumber: 1, clickCount: 1, pressure: 1)))
        }
        try click(NSPoint(x: 75, y: 40))
        #expect(seeks.withLock { $0.count == 1 && $0[0].0 == kick && $0[0].1 == 18000 && $0[0].2.input == .click })
        try click(NSPoint(x: 75, y: 10))
        #expect(seeks.withLock { $0.count == 1 })
        #expect(preview.accessibilityPerformPress())
        #expect(requests.withLock { $0.count == 3 && $0.last?.0 == kick })
        // An old view cannot seek the new selection, even before SwiftUI updates it.
        state.browser.selection = snare
        try click(NSPoint(x: 100, y: 40))
        #expect(seeks.withLock { $0.count == 1 })
        preview.show(url: snare, status: status)
        #expect(!preview.accessibilityPerformPress())
        controls.stop.performClick(nil)
        #expect(requests.withLock { $0.last?.0 == nil })

        let restored = WorkspaceState(defaults: defaults)
        #expect(!restored.auditionPreferences.automatic && restored.auditionPreferences.gain == 0.25)
        #expect(controller.undoCount == undo && controller.snapshot?.trackCount == 0)
        session.synchronizeAuditionOutput(visible: false)
        #expect(settings.withLock { $0.last?.0 == false })
        await controller.close()
        #expect(session.auditionSelected() == nil)
    }

    @Test func invalidSavedAuditionGainCannotReachANativeSlider() throws {
        let name = "echode-audition-invalid-\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: name))
        defer { defaults.removePersistentDomain(forName: name) }
        defaults.set(Data(#"{"automatic":false,"gain":2}"#.utf8), forKey: "workspace.audition.v1")
        let state = WorkspaceState(defaults: defaults)
        #expect(state.auditionPreferences.isValid && state.auditionPreferences.gain == 1)
    }
}
