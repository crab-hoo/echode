import Foundation
import XCTest
@testable import EchodeBridge

final class DeviceTests: XCTestCase {
    func testDeviceCommandsEncodeAndRefuseDevicesWithoutAPlugin() async throws {
        let device = UUID(uuidString: "00000000-0000-0000-0000-0000000000D0")!
        let encoded = try JSONEncoder().encode(DeviceCommand.set(device: device, changes: [DeviceParameterChange(id: 2, value: 0.5)]))
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        XCTAssertEqual(object["type"] as? String, "device.parameter.set")
        XCTAssertEqual(object["device"] as? String, device.uuidString)
        XCTAssertEqual((object["changes"] as? [[String: Any]])?.first?["value"] as? Double, 0.5)

        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-device-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let document = try Document(creating: url.path)
        defer { document.shutdown() }
        let added = try await document.perform(#"{"type":"track.add","name":"fx"}"#)
        XCTAssertEqual(try added.deviceReport(), DeviceReport())
        let mixer = try XCTUnwrap(added.deviceID(onTrack: 0, at: 0))
        // Neither request reaches the output device or a helper.
        for (command, reason) in [(DeviceCommand.parameters(device: mixer), "mixer"),
                                  (DeviceCommand.commit(device: device), "no longer")] {
            do {
                _ = try await document.device(command)
                XCTFail("\(command) must be refused")
            } catch let error as BridgeError {
                XCTAssertTrue(error.message.contains(reason), error.message)
            }
        }
        let unchanged = try await document.snapshot()
        XCTAssertEqual(unchanged.trackCount, 1)
        XCTAssertEqual(try unchanged.deviceReport(), DeviceReport())

        let track = try XCTUnwrap(unchanged.trackID(at: 0))
        _ = try await document.perform(#"{"type":"device.add","track":"\#(track.uuidString)","plugin":"org.example.fx"}"#)
        let inserted = try await document.snapshot()
        let fx = try XCTUnwrap(inserted.deviceID(onTrack: 0, at: 0))
        for parameter in [9, 2] {
            _ = try await document.perform(#"{"type":"device.pin_parameter","device":"\#(fx.uuidString)","parameter":\#(parameter),"pinned":true}"#)
        }
        let pinned = try await document.snapshot()
        XCTAssertEqual(pinned.devicePins(onTrack: 0, at: 0), [2, 9])
        XCTAssertEqual(pinned.devicePins(onTrack: 0, at: 1), [])
        XCTAssertNil(pinned.devicePins(onTrack: 0, at: 2))
        XCTAssertNil(pinned.devicePins(onTrack: -1, at: 0))
    }

    func testReportsDecodeParametersAndFlags() throws {
        let json = #"{"parameters":[{"id":2,"flags":65541,"name":"Mode","module":"Out","min":0,"max":2,"default":0,"value":1,"text":"Half"}]}"#
        let report = try JSONDecoder().decode(DeviceReport.self, from: Data(json.utf8))
        let mode = try XCTUnwrap(report.parameters?.first)
        XCTAssertTrue(mode.stepped && mode.hidden && mode.isEnum && !mode.readOnly)
        XCTAssertEqual(mode.defaultValue, 0)
        XCTAssertEqual(mode.text, "Half")
    }
}
