import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@Suite @MainActor
struct TypedTrackDeviceEditTests {
    @Test func contractTrackAndDeviceEditsPreserveSelectionHistoryAndPersistence() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-typed-track-device-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        await controller.addTrack(name: "first\n音源")
        let first = try #require(controller.selection)
        await controller.addTrack(name: "second")
        let second = try #require(controller.selection)
        let insertion = TrackInsertionRequest(document: document, after: first, name: "between", intoFolder: false)
        #expect(await controller.insertTrack(insertion))
        let between = try #require(controller.selection)
        #expect(controller.tracks.map(\.id) == [first, between, second])
        #expect(controller.selectedTracks == [between])

        let before = controller.undoCount
        #expect(await controller.insertPlugin("org.example.typed", on: [second, first], documentID: document))
        let a = try #require(controller.deviceRows(on: first).first)
        let b = try #require(controller.deviceRows(on: second).first)
        #expect(a.id != b.id && a.plugin == b.plugin)
        #expect(controller.deviceRows(on: between).isEmpty && controller.selection == between)
        #expect(controller.undoCount == before + 1)
        await controller.undo()
        #expect(controller.deviceRows(on: first).isEmpty && controller.deviceRows(on: second).isEmpty)
        await controller.redo()
        #expect(controller.deviceRows(on: first) == [a] && controller.deviceRows(on: second) == [b])
        let parameter = UInt32.max - 1 // The largest valid CLAP id; UInt32.max is reserved.
        #expect(await controller.setPinned(parameter, device: a.id, pinned: true, documentID: document))
        #expect(await controller.editDeviceChain(.bypass(device: a.id, bypassed: true), documentID: document))
        #expect(await controller.editDeviceChain(.offline(device: a.id, offline: true), documentID: document))

        let duplication = TrackDuplicationRequest(document: document, sources: [first, second])
        let undo = controller.undoCount
        #expect(await controller.duplicateTracks(duplication))
        let roots = controller.tracks.map(\.id)
        try #require(roots.count == 5)
        let firstCopy = roots[1], secondCopy = roots[4]
        #expect(roots == [first, firstCopy, between, second, secondCopy])
        #expect(controller.selectedTracks == [firstCopy, secondCopy] && controller.selection == firstCopy)
        #expect(controller.undoCount == undo + 1)
        let copied = try #require(controller.deviceRows(on: firstCopy).first)
        #expect(copied.id != a.id && copied.pinned == [parameter] && copied.bypassed && copied.offline)
        #expect(controller.deviceRows(on: secondCopy).first?.id != b.id)
        await controller.undo()
        #expect(controller.tracks.map(\.id) == [first, between, second])
        await controller.redo()
        #expect(controller.tracks.map(\.id) == roots)

        #expect(await controller.insertPlugin("org.example.tail", on: first, documentID: document))
        let tail = try #require(controller.deviceRows(on: first).last)
        #expect(await controller.editDeviceChain(.copy(source: a.id, track: first, before: tail.id), documentID: document))
        let clone = try #require(controller.deviceRows(on: first).dropFirst().first)
        #expect(clone.id != a.id && clone.id != tail.id)
        #expect(clone.pinned == [parameter] && clone.bypassed && clone.offline)
        #expect(await controller.editDeviceChain(.reorder(device: tail.id, before: a.id), documentID: document))
        #expect(controller.deviceRows(on: first).map(\.id) == [tail.id, a.id, clone.id])
        #expect(await controller.editDeviceChain(.reorder(device: tail.id, before: nil), documentID: document))
        #expect(controller.deviceRows(on: first).map(\.id) == [a.id, clone.id, tail.id])
        await controller.undo()
        #expect(controller.deviceRows(on: first).map(\.id) == [tail.id, a.id, clone.id])
        await controller.redo()
        #expect(controller.deviceRows(on: first).map(\.id) == [a.id, clone.id, tail.id])
        #expect(await controller.editDeviceChain(.remove(clone.id), documentID: document))
        await controller.undo()
        let kept = controller.deviceRows(on: first), count = controller.undoCount
        #expect(!(await controller.setPinned(parameter, device: a.id, pinned: true, documentID: document)))
        #expect(!(await controller.editDeviceChain(.bypass(device: a.id, bypassed: true), documentID: document)))
        #expect(!(await controller.editDeviceChain(.offline(device: a.id, offline: true), documentID: document)))
        #expect(!(await controller.editDeviceChain(.reorder(device: a.id, before: clone.id), documentID: document)))
        #expect(!(await controller.editDeviceChain(.copy(source: a.id, track: first, before: UUID()), documentID: document)))
        #expect(!(await controller.setPinned(UInt32.max, device: a.id, pinned: true, documentID: document)))
        #expect(controller.errorMessage != nil)
        controller.errorMessage = nil
        #expect(!(await controller.insertPlugin("org.example.invalid", on: [first, UUID()], documentID: document)))
        #expect(!(await controller.duplicateTracks(TrackDuplicationRequest(document: document, sources: [first, first]))))
        #expect(controller.deviceRows(on: first) == kept && controller.tracks.map(\.id) == roots)
        #expect(controller.undoCount == count && controller.redoCount == 1)
        await controller.redo()
        #expect(controller.deviceRows(on: first).map(\.id) == [a.id, tail.id])
        await controller.removeDevice(tail.id, documentID: document)
        #expect(controller.deviceRows(on: first).map(\.id) == [a.id])
        await controller.undo()
        #expect(controller.deviceRows(on: first).map(\.id) == [a.id, tail.id])
        await controller.redo()

        let saved = Dictionary(uniqueKeysWithValues: roots.map { ($0, controller.deviceRows(on: $0)) })
        await controller.close()
        await controller.open(url)
        #expect(controller.errorMessage == nil && controller.tracks.map(\.id) == roots)
        #expect(controller.tracks.first?.name == "first\n音源")
        for track in roots { #expect(controller.deviceRows(on: track) == saved[track]) }
        #expect(!(await controller.insertTrack(insertion)))
        #expect(!(await controller.duplicateTracks(duplication)))
        #expect(!(await controller.insertPlugin("org.example.stale", on: first, documentID: document)))
        #expect(!(await controller.insertPlugin("org.example.stale", on: [first, second], documentID: document)))
        #expect(!(await controller.editDeviceChain(.remove(a.id), documentID: document)))
        #expect(!(await controller.setPinned(parameter, device: a.id, pinned: false, documentID: document)))
        await controller.removeDevice(a.id, documentID: document)
        #expect(controller.undoCount == 0 && controller.redoCount == 0)
        for track in roots { #expect(controller.deviceRows(on: track) == saved[track]) }
        let current = try #require(controller.documentID)
        #expect(await controller.setPinned(parameter, device: a.id, pinned: false, documentID: current))
        #expect(controller.deviceRows(on: first).first?.pinned == [])
        await controller.undo()
        #expect(controller.deviceRows(on: first).first?.pinned == [parameter])
        await controller.close()
    }
}
