import AppKit
import Carbon.HIToolbox
import CoreText
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoTests {
    @Test func pitchRowsSnapAndDegreesFollowTheDisplayKey() {
        let camera = PianoCamera(topPitch: 72)
        #expect(camera.pitch(at: 0) == 72)
        #expect(camera.pitch(at: 19) == 72)
        #expect(camera.pitch(at: 20) == 71)
        #expect(camera.pitch(at: 1_000_000) == 0)
        #expect(camera.tick(at: 10) == 176_400)
        #expect(PianoCamera.ticks(.infinity) == nil)
        #expect(PianoCamera.ticks(Double(Int64.max)) == nil)
        #expect(DisplayKey().degree(60) == 1)
        #expect(DisplayKey().degreeLabel(60) == "I")
        #expect(DisplayKey.numeral(1) == "I" && DisplayKey.numeral(4) == "IV" && DisplayKey.numeral(7) == "VII")
        #expect(DisplayKey.numeral(0) == nil && DisplayKey.numeral(8) == nil)
        let minor = DisplayKey(tonic: 9, minor: true)
        #expect(minor.degree(69) == 1)
        #expect(minor.degree(72) == 3)
        #expect(minor.degreeLabel(72) == "III")
        #expect(minor.degree(73) == nil)
        #expect(minor.degreeLabel(73) == nil)
        #expect(PianoGesture.mode(at: 1, start: 0, end: 2) == .move)
        #expect(PianoGesture.mode(at: 0, start: 0, end: 2) == .leftEdge)
        #expect(PianoGesture.mode(at: 2, start: 0, end: 2) == .rightEdge)
        #expect((PianoCamera().topPitch - 60) * PianoCamera().pixelsPerPitch < 96)
    }

    @Test func degreeNumbersRepeatAcrossOctavesForEveryDisplayKey() {
        for tonic in 0..<12 {
            for minor in [false, true] {
                let key = DisplayKey(tonic: tonic, minor: minor)
                let intervals = minor ? [0, 2, 3, 5, 7, 8, 10] : [0, 2, 4, 5, 7, 9, 11]
                for octave in 0..<10 {
                    for (index, interval) in intervals.enumerated() {
                        let pitch = octave * 12 + tonic + interval
                        if pitch <= 127 { #expect(key.degree(pitch) == index + 1) }
                    }
                    let chromatic = minor ? [1, 4, 6, 9, 11] : [1, 3, 6, 8, 10]
                    for interval in chromatic {
                        let pitch = octave * 12 + tonic + interval
                        if pitch <= 127 { #expect(key.degree(pitch) == nil) }
                    }
                }
            }
        }
    }

    @Test func degreeColumnShowsRomanNumeralsThatFit() {
        let track = UUID()
        let camera = PianoCamera(topPitch: 72)
        let input = CanvasInput(snapshot: nil, selected: track, camera: ArrangeCamera(), width: 640, height: 400, scale: 2)
        let frame = PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil, camera: camera, key: DisplayKey(), preview: nil))
        let column = frame.labels.filter { $0.clip.x == 0 }.map(\.text)
        let numerals = ["I", "II", "III", "IV", "V", "VI", "VII"]
        #expect(column.contains("I–VII"))
        #expect(numerals.allSatisfy(column.contains))
        #expect(column.allSatisfy { $0 == "I–VII" || numerals.contains($0) })
        let font = CTFontCreateUIFontForLanguage(.system, 12, nil)!
        func width(_ name: String) -> Double {
            let attributes: [NSAttributedString.Key: Any] = [NSAttributedString.Key(kCTFontAttributeName as String): font]
            return CTLineGetTypographicBounds(CTLineCreateWithAttributedString(NSAttributedString(string: name, attributes: attributes)), nil, nil, nil)
        }
        #expect(width("I–VII") <= frame.layout.degreeWidth - 4)
        #expect(numerals.allSatisfy { width($0) <= frame.layout.degreeWidth - 8 })
    }

    @Test func keyboardAndGridIgnoreTheScaleAndEditingUsesTheGridOrigin() {
        let track = UUID()
        for top in [64.25, 72.0, 0.5, 127.0] {
            for height in [12.0, 20.0, 48.0] {
                let camera = PianoCamera(originBeat: 1.25, topPitch: top, pixelsPerPitch: height)
                let input = CanvasInput(snapshot: nil, selected: track, camera: ArrangeCamera(), width: 640, height: 211, scale: 2)
                let first = PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil, camera: camera, key: DisplayKey(), preview: nil))
                for key in [DisplayKey(tonic: 2), DisplayKey(tonic: 6, minor: true)] {
                    let changed = PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil, camera: camera, key: key, preview: nil))
                    #expect(first.gridInstances.map(\.rect) == changed.gridInstances.map(\.rect))
                    #expect(first.gridInstances.map(\.color) == changed.gridInstances.map(\.color))
                    #expect(first.keyboardInstances.map(\.rect) == changed.keyboardInstances.map(\.rect))
                    #expect(first.keyboardInstances.map(\.color) == changed.keyboardInstances.map(\.color))
                }
                for row in first.rows {
                    let point = first.layout.gridPoint(CGPoint(x: 135, y: Double(row.y) + height / 2))
                    #expect(camera.pitch(at: point.y) == row.pitch)
                }
                #expect(first.layout.degreeWidth == 36 && first.layout.keyboardWidth == 44)
                #expect(first.layout.gridPoint(CGPoint(x: 160, y: 44)) == CGPoint(x: 80, y: 20))
                #expect(first.layout.paneRect(CGRect(x: 1, y: 2, width: 3, height: 4)) == CGRect(x: 81, y: 26, width: 3, height: 4))
                #expect(!first.layout.grid.contains(CGPoint(x: 71, y: 100)))
                #expect(!first.layout.grid.contains(CGPoint(x: 200, y: 23)))
            }
        }
        let narrow = PianoLayout(width: 40, height: 10)
        #expect(narrow.grid.width == 0 && narrow.grid.height == 0)
    }

    @Test func aDragIsOneDurableEditAndPreservesIdentityAndVelocity() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-piano-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        await controller.insertNote(pitch: 60, beat: 0, length: 1)
        let track = try #require(controller.selection)
        let before = try #require(controller.snapshot)
        let note = try #require(before.pianoNeighbor(track: track, current: nil))
        controller.noteFocus = note.id
        let count = controller.undoCount
        let camera = PianoCamera()
        let drag = PianoGesture(id: note.id, original: NoteGeometry(note), mode: .move, x: 0, y: 40)
        let intermediate = try #require(drag.preview(x: 18, y: 20, camera: camera))
        let final = try #require(drag.preview(x: 144, y: 0, camera: camera))
        #expect(intermediate.start == 176_400)
        #expect(controller.undoCount == count)
        await controller.editNote(note.id, geometry: final)
        #expect(controller.errorMessage == nil)
        #expect(controller.undoCount == count + 1)
        let edited = try #require(controller.snapshot?.pianoNote(note.id))
        #expect(edited.id.note == note.id.note)
        #expect(edited.velocity == note.velocity)
        #expect(edited.start == 1_411_200 && edited.pitch == 62)
        #expect(before.pianoNote(note.id)?.start == 0)
        await controller.undo()
        #expect(controller.snapshot?.pianoNote(note.id)?.start == 0)
        // An abandoned preview leaves Redo and durable storage untouched.
        _ = drag.preview(x: 300, y: 0, camera: camera)
        #expect(controller.redoCount == 1)
        await controller.redo()
        await controller.close()
        await controller.open(url)
        let reopened = try #require(controller.snapshot?.pianoNote(note.id))
        #expect(reopened.start == edited.start && reopened.pitch == edited.pitch)
        await controller.deleteNote(reopened.id)
        #expect(controller.snapshot?.pianoNote(note.id) == nil)
        await controller.undo()
        #expect(controller.snapshot?.pianoNote(note.id)?.id.note == note.id.note)
        await controller.close()
    }

    @Test func resizeMergesAutoClipsAndNavigationRevealsOffscreenNotes() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-piano-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let track = try #require(controller.selection)
        await controller.insertNote(pitch: 60, beat: 0, length: 1)
        await controller.insertNote(pitch: 110, beat: 1000, length: 1)
        let first = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        let far = try #require(controller.snapshot?.pianoNeighbor(track: track, current: first.id))
        var camera = PianoCamera()
        camera.reveal(far, width: 360, height: 200)
        let shown = try #require(controller.snapshot).pianoNotes(track: track,
            originTick: try #require(PianoCamera.ticks(camera.originBeat)), widthTicks: 705_600 * 5,
            topPitch: camera.topPitch, heightPitches: 10, pixelsPerTick: 72 / 705_600, pixelsPerPitch: 20)
        #expect(shown.contains { $0.id == far.id })
        await controller.editNote(far.id, geometry: NoteGeometry(start: 705_600 * 8, length: 705_600, pitch: 64))
        let edge = PianoGesture(id: first.id, original: NoteGeometry(first), mode: .rightEdge, x: 0, y: 0)
        let lengthened = try #require(edge.preview(x: 72 * 8, y: 0, camera: PianoCamera()))
        await controller.editNote(first.id, geometry: lengthened)
        let resolved = try #require(controller.snapshot?.pianoNote(far.id))
        #expect(resolved.id.placement == first.id.placement)
        await controller.editNote(first.id, geometry: NoteGeometry(start: .max, length: 1, pitch: 60))
        #expect(controller.errorMessage == nil)
        await controller.close()
    }

    @Test func repeatedArrowKeysMoveFromTheStepAlreadySent() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-piano-steps-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let track = try #require(controller.selection)
        await controller.insertNote(pitch: 60, beat: 0, length: 1, on: track)
        let snapshot = try #require(controller.snapshot)
        let note = try #require(snapshot.pianoNeighbor(track: track, current: nil))
        let count = controller.undoCount
        let grid = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        let document = controller.documentID
        var sent: [Int] = []
        var writes: [Task<Bool, Never>] = []
        grid.onEdit = { id, geometry in
            sent.append(geometry.pitch)
            writes.append(Task { await controller.editNote(id, geometry: geometry, documentID: document) })
        }
        grid.setSnapshot(snapshot, track: track, focus: note.id, key: DisplayKey(), busy: false)
        let up = try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
            windowNumber: 0, context: nil, characters: " ", charactersIgnoringModifiers: " ", isARepeat: true, keyCode: UInt16(kVK_UpArrow)))
        grid.keyDown(with: up)
        grid.setSnapshot(snapshot, track: track, focus: note.id, key: DisplayKey(), busy: true)
        grid.keyDown(with: up)
        #expect(sent == [61, 62])
        var applied: [Bool] = []
        for write in writes { applied.append(await write.value) }
        #expect(applied == [true, true])
        #expect(controller.snapshot?.pianoNote(note.id)?.pitch == 62)
        #expect(controller.undoCount == count + 2)
        #expect(controller.errorMessage == nil)
        await controller.close()
    }
}
