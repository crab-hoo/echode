import EchodeBridge
import Foundation
import Synchronization
import Testing

@testable import EchodeAppCore

/// The output's automatic mutes reach the app as state, are spoken and shown
/// beside the transport, and never raise an alert (ADR 0281, ADR 0282).
@MainActor
struct AutoMuteTests {
    private func state(_ extra: String) throws -> AudioState {
        try JSONDecoder().decode(
            AudioState.self,
            from: Data(
                """
                {"playing":true,"running":true,"opening":false,"tick":0,"devices":[],"metronome":false\(extra)}
                """.utf8))
    }

    @Test func audioStateReadsAutoMutesAndOlderRepliesWithoutThem() throws {
        let (track, device) = (UUID(), UUID())
        let muted = try state(
            """
            ,"auto_muted_tracks":[{"track":"\(track.uuidString.lowercased())","device":"\(device.uuidString.lowercased())","sample":266}],
            "master_auto_mute":{"peak":null,"sample":795}
            """)
        #expect(muted.autoMutedTracks == [TrackAutoMute(track: track, device: device, sample: 266)])
        #expect(muted.masterAutoMute == MasterAutoMute(peak: nil, sample: 795))
        let older = try state("")
        #expect(older.autoMutedTracks.isEmpty && older.masterAutoMute == nil)
    }

    @Test func theTransportNamesWhatWasMutedWithoutAnAlert() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-auto-mute-\(UUID()).echode")
        defer { removeTestFile(url) }
        let (first, second) = (UUID(), UUID())
        let tracks = """
            ,"auto_muted_tracks":[{"track":"\(first.uuidString)","device":"\(UUID().uuidString)","sample":10},
            {"track":"\(second.uuidString)","device":"\(UUID().uuidString)","sample":20}]
            """
        let replies = [
            try state(tracks + #","master_auto_mute":{"peak":16.0,"sample":30}"#), try state(tracks), try state(""),
        ]
        let next = Mutex(0)
        let controller = ProjectController(sendAudio: { document, command in
            guard case .metronome = command else { return try await document.audio(command) }
            return next.withLock { index in
                defer { index = min(index + 1, replies.count - 1) }
                return replies[index]
            }
        })
        await controller.open(url, creating: true)
        await controller.toggleMetronome()
        #expect(controller.autoMuteNotice == String(format: text("Master Auto-Muted"), "+24.1 dBFS"))
        #expect(
            controller.autoMuteReasons.keys.sorted { $0.uuidString < $1.uuidString }
                == [first, second].sorted { $0.uuidString < $1.uuidString })
        #expect(controller.autoMuteReasons[first] == String(format: text("Auto-Mute Reason"), text("Unknown Plugin")))
        await controller.toggleMetronome()
        #expect(controller.autoMuteNotice == String(format: text("Tracks Auto-Muted"), 2))
        await controller.toggleMetronome()
        #expect(controller.autoMuteNotice == nil && controller.autoMuteReasons.isEmpty)
        #expect(controller.errorMessage == nil, "an automatic mute is not an alert")
        await controller.close()
    }
}
