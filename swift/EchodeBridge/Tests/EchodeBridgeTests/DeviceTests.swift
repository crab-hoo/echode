import Foundation
import Testing

@testable import EchodeBridge

struct DeviceTests {
    @Test func latencyMetadataDecodesWithoutBreakingOlderReports() throws {
        let decoder = JSONDecoder()
        #expect(try decoder.decode(DeviceReport.self, from: Data(#"{"parameters":[]}"#.utf8)).latencySamples == nil)
        for samples: UInt32 in [0, 512, .max] {
            let report = try decoder.decode(
                DeviceReport.self, from: Data("{\"parameters\":[],\"latency_samples\":\(samples)}".utf8))
            #expect(report.latencySamples == samples)
            let activity = try decoder.decode(
                PluginActivity.self,
                from: Data(
                    """
                    {"device":"00000000-0000-0000-0000-0000000000d0","events":[],"values":[],"commands":[],
                     "rescan":false,"lost":false,"dirty":false,"closed":false,"window":false,"latency_samples":\(samples)}
                    """.utf8))
            #expect(activity.latencySamples == samples)
        }
    }
    /// Window commands go to the device lane; closing never starts a helper,
    /// and a quiet document reports no plugin activity without waiting long.
    @Test func pluginWindowCommandsAndActivityPages() async throws {
        let device = UUID(uuidString: "00000000-0000-0000-0000-0000000000D0")!
        let encoded = try JSONEncoder().encode(DeviceCommand.showWindow(device: device, title: "fx — Gain"))
        let object = try #require(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        #expect(object["type"] as? String == "device.gui.show")
        #expect(object["title"] as? String == "fx — Gain")
        let page = try JSONDecoder().decode(
            PluginActivityPage.self,
            from: Data(
                #"""
                {"next":3,"closed":false,"entries":[{"device":"00000000-0000-0000-0000-0000000000d0",
                "events":[{"id":1,"kind":"begin"},{"id":1,"kind":"end"}],"values":[{"id":1,"value":1.5,"text":"1.50x"}],
                "commands":["Play / Stop","Undo"],"rescan":false,"lost":false,"dirty":false,"closed":false,"window":true}]}
                """#.utf8))
        #expect(page.next == 3)
        #expect(page.entries.first?.device == device)
        #expect(page.entries.first?.events.map(\.kind) == [.begin, .end])
        #expect(page.entries.first?.windowOpen == true)
        #expect(page.entries.first?.commands == ["Play / Stop", "Undo"])
        #expect(page.entries.first?.windowState == nil, "an older page has no lifetime change")
        let lifetime = try JSONDecoder().decode(
            PluginActivityPage.self,
            from: Data(
                #"""
                {"next":4,"closed":false,"entries":[{"device":"00000000-0000-0000-0000-0000000000d0",
                "events":[],"values":[],"commands":[],"rescan":false,"lost":false,"dirty":false,
                "closed":true,"window":true,"window_state":false}]}
                """#.utf8))
        #expect(lifetime.entries.first?.windowState == false)
        #expect(lifetime.entries.first?.windowOpen == true, "edit context is independent of lifetime")

        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-window-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let document = try Document(creating: url.path)
        let quiet = try document.pluginActivity(after: 0, timeout: 0.01)
        #expect(quiet == PluginActivityPage(next: 0))
        let added = try await document.perform(#"{"type":"track.add","name":"fx"}"#)
        let track = try #require(added.trackID(at: 0))
        let inserted = try await document.perform(
            #"{"type":"device.add","track":"\#(track.uuidString)","plugin":"org.example.fx"}"#)
        let fx = try #require(inserted.deviceID(onTrack: 0, at: 1) ?? inserted.deviceID(onTrack: 0, at: 0))
        let closed = try await document.device(.closeWindow(device: fx))
        #expect(try closed.deviceReport().closed == true)
        document.shutdown()
        #expect(try document.pluginActivity(after: 0, timeout: 5).closed, "a closed document ends the reader")
    }

    @Test func deviceCommandsEncodeAndRefuseDevicesWithoutAPlugin() async throws {
        let device = UUID(uuidString: "00000000-0000-0000-0000-0000000000D0")!
        let encoded = try JSONEncoder().encode(
            DeviceCommand.set(device: device, changes: [DeviceParameterChange(id: 2, value: 0.5)]))
        let object = try #require(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        #expect(object["type"] as? String == "device.parameter.set")
        #expect(object["device"] as? String == device.uuidString)
        #expect((object["changes"] as? [[String: Any]])?.first?["value"] as? Double == 0.5)

        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-device-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let document = try Document(creating: url.path)
        defer { document.shutdown() }
        let added = try await document.perform(#"{"type":"track.add","name":"fx"}"#)
        #expect(try added.deviceReport() == DeviceReport())
        let mixer = try #require(added.deviceID(onTrack: 0, at: 0))
        // Neither request reaches the output device or a helper.
        for (command, reason) in [
            (DeviceCommand.parameters(device: mixer), "mixer"),
            (DeviceCommand.commit(device: device), "no longer"),
        ] {
            do {
                _ = try await document.device(command)
                Issue.record("\(command) must be refused")
            } catch let error as BridgeError {
                #expect(error.message.contains(reason), "\(error.message)")
            }
        }
        let unchanged = try await document.snapshot()
        #expect(unchanged.trackCount == 1)
        #expect(try unchanged.deviceReport() == DeviceReport())

        let track = try #require(unchanged.trackID(at: 0))
        _ = try await document.perform(
            #"{"type":"device.add","track":"\#(track.uuidString)","plugin":"org.example.fx"}"#)
        let inserted = try await document.snapshot()
        let fx = try #require(inserted.deviceID(onTrack: 0, at: 0))
        for parameter in [9, 2] {
            _ = try await document.perform(
                #"{"type":"device.pin_parameter","device":"\#(fx.uuidString)","parameter":\#(parameter),"pinned":true}"#
            )
        }
        let pinned = try await document.snapshot()
        #expect(pinned.devicePins(onTrack: 0, at: 0) == [2, 9])
        #expect(pinned.devicePins(onTrack: 0, at: 1) == [])
        #expect(pinned.devicePins(onTrack: 0, at: 2) == nil)
        #expect(pinned.devicePins(onTrack: -1, at: 0) == nil)
    }

    @Test func reportsDecodeParametersAndFlags() throws {
        let json =
            #"{"parameters":[{"id":2,"flags":65541,"name":"Mode","module":"Out","min":0,"max":2,"default":0,"value":1,"text":"Half"}]}"#
        let report = try JSONDecoder().decode(DeviceReport.self, from: Data(json.utf8))
        let mode = try #require(report.parameters?.first)
        #expect(mode.stepped && mode.hidden && mode.isEnum && !mode.readOnly)
        #expect(mode.defaultValue == 0)
        #expect(mode.text == "Half")
        for (json, parsed) in [
            (
                #"{"parsed":{"id":2,"value":1.5,"converts":true}}"#,
                DeviceParameterParse(id: 2, value: 1.5, converts: true)
            ),
            (
                #"{"parsed":{"id":2,"value":null,"converts":false}}"#,
                DeviceParameterParse(id: 2, value: nil, converts: false)
            ),
        ] {
            #expect(try JSONDecoder().decode(DeviceReport.self, from: Data(json.utf8)).parsed == parsed)
        }
        let device = UUID(uuidString: "00000000-0000-0000-0000-0000000000D0")!
        let encoded = try JSONEncoder().encode(DeviceCommand.parse(device: device, id: 2, text: "Mute"))
        let object = try #require(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        #expect(object["type"] as? String == "device.parameter.parse")
        #expect(object["id"] as? Int == 2)
        #expect(object["text"] as? String == "Mute")
    }
}
