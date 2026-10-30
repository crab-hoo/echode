import Foundation
import XCTest
@testable import EchodeBridge

final class EditActionTests: XCTestCase {
    func testEveryCatalogActionMatchesTheRustAcceptedWireFixture() throws {
        let track = "00000000-0000-0000-0000-000000000001"
        let clip = "00000000-0000-0000-0000-000000000002"
        let note = "00000000-0000-0000-0000-000000000003"
        let device = "00000000-0000-0000-0000-000000000004"
        let actions: [EditAction] = [
            .addTrack(name: "音色 \"A\"\n"),
            .insertTrack(after: track, intoFolder: true, name: "child"),
            .duplicateTrack(source: track),
            .setMeter(numerator: 7, denominator: 8),
            .setTempo(at: 9_007_199_254_740_993, numerator: 121, denominator: 1),
            .insertNote(track: track, pitch: 60, velocity: 100, start: 9_007_199_254_740_993, length: 705_600),
            .duplicateNote(track: track, clip: clip, note: note, pitch: 62, start: 10),
            .editNote(track: track, clip: clip, note: note, pitch: 64, start: 11, length: 12),
            .setNoteVelocity(track: track, clip: clip, note: note, velocity: 127),
            .deleteNote(track: track, note: note),
            .resizeClip(track: track, clip: clip, start: 20, length: 21),
            .addChild(parent: track, name: "child"),
            .setParent(track: track),
            .linkClip(source: clip, track: track, start: 22),
            .moveClip(clip: clip, track: track, start: 23),
            .duplicateClip(clip: clip, track: track, start: 24),
            .setClipGain(clip: clip, gain: 0.125),
            .setParameter(parameter: device, value: -0.25),
            .setPluginState(device: device),
            .setClipFades(clip: clip, fadeIn: 9_007_199_254_740_993, fadeOut: .max),
            .addDevice(track: track, plugin: "org.example.fx"),
            .bypassDevice(device: device, bypassed: true),
            .offlineDevice(device: device, offline: false),
            .pinParameter(device: device, parameter: .max, pinned: true),
            .reorderDevice(device: device),
            .copyDevice(source: device, track: track),
            .removeDevice(device: device),
            .restore(revision: "0123456789abcdef"),
            .placeAudio(track: track, start: 25, hash: String(repeating: "a", count: 64), sampleRate: 48_000, channels: 2, frames: .max),
            .importAudio(track: track, start: 26, path: "/tmp/音声 \"one\".wav")
        ]
        let fixture = try XCTUnwrap(Bundle.module.url(forResource: "edit-actions", withExtension: "json"))
        let expected = try JSONSerialization.jsonObject(with: Data(contentsOf: fixture))
        let actual = try JSONSerialization.jsonObject(with: JSONEncoder().encode(actions))
        XCTAssertEqual(try canonical(actual), try canonical(expected))
    }

    func testIntegerExtremesAndOptionalValuesAreNotConvertedThroughDouble() throws {
        let actions: [EditAction] = [
            .setClipFades(clip: "clip", fadeIn: .min, fadeOut: .max),
            .placeAudio(track: "track", start: 9_007_199_254_740_993, hash: "hash", sampleRate: .max, channels: .max, frames: .max),
            .duplicateNote(track: "track", clip: "clip", note: "note", pitch: .max, start: -1, copy: "copy"),
            .setParent(track: "track", parent: "parent"),
            .setPluginState(device: "device", state: "state"),
            .reorderDevice(device: "device", before: "before"),
            .copyDevice(source: "source", track: "track", before: "before")
        ]
        let encoded = try JSONEncoder().encode(actions)
        let text = String(decoding: encoded, as: UTF8.self)
        for literal in ["-9223372036854775808", "9223372036854775807", "9007199254740993", "18446744073709551615"] {
            XCTAssertTrue(text.contains(literal), "Missing exact integer \(literal): \(text)")
        }
        let objects = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [[String: Any]])
        for (index, key, value) in [(2, "copy", "copy"), (3, "parent", "parent"), (4, "state", "state"), (5, "before", "before"), (6, "before", "before")] {
            XCTAssertEqual(objects[index][key] as? String, value)
        }
    }

    func testFloatBitsSurviveEncodingAndNonFiniteValuesAreRejected() throws {
        struct Parameter: Decodable { let value: Float }
        for value in [Float.leastNonzeroMagnitude, Float.greatestFiniteMagnitude, -Float.leastNormalMagnitude, -0.25] {
            let bytes = try JSONEncoder().encode(EditAction.setParameter(parameter: "parameter", value: value))
            XCTAssertEqual(try JSONDecoder().decode(Parameter.self, from: bytes).value.bitPattern, value.bitPattern)
        }
        for value in [Float.nan, Float.infinity, -Float.infinity] {
            XCTAssertThrowsError(try JSONEncoder().encode(EditAction.setClipGain(clip: "clip", gain: value)))
        }
    }

    private func canonical(_ object: Any) throws -> Data {
        try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
    }
}
