import AppKit
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct SamplePreviewTests {
    private func status(complete: Bool = false, decoded: Bool = false, first: UInt64 = 0) throws -> AuditionStatus {
        let json = #"""
        {"request":1,"handled":1,"path":"/samples/kick.wav","phase":"sounding",
         "history":[{"request":1,"input":"click","requested_at_ms":1,"phase":"sounding","opened_output":false,"heard_us":9\#(decoded ? #","decoded_us":12"# : "")}],
         "waveform":{"frames":48000,"loaded_frames":24000,"first_frame":\#(first),"sample_rate":48000,
                     "frames_per_peak":12000,"complete":\#(complete),"peaks":[[-0.5,1.0],[-0.25,0.5]]}}
        """#
        return try JSONDecoder().decode(AuditionStatus.self, from: Data(json.utf8))
    }

    @Test func contractPreviewUsesProgressiveSourcePeaksAndRejectsAnotherSelection() throws {
        let view = SamplePreviewView()
        view.frame = NSRect(x: 0, y: 0, width: 200, height: 86)
        let url = URL(fileURLWithPath: "/samples/kick.wav")
        view.show(url: url, status: try status())
        let waveform = try #require(view.waveform)
        #expect(!waveform.complete && waveform.loadedFrames == 24000 && waveform.frames == 48000)
        let bars = view.bars(in: NSRect(x: 0, y: 0, width: 200, height: 40))
        #expect(bars == [NSRect(x: 0, y: 0, width: 50, height: 30), NSRect(x: 50, y: 10, width: 50, height: 15)])
        #expect((view.accessibilityValue() as? String)?.contains(text("Loading sample")) == true)
        view.show(url: url, status: try status(complete: true))
        #expect((view.accessibilityValue() as? String)?.contains(text("Loading sample")) == false)
        view.show(url: url, status: try status(complete: true, first: 12000))
        #expect(view.bars(in: NSRect(x: 0, y: 0, width: 200, height: 40)) == [bars[1]])
        #expect((view.accessibilityValue() as? String)?.contains(text("Partial waveform")) == true)
        view.show(url: URL(fileURLWithPath: "/samples/snare.wav"), status: try status())
        #expect(view.waveform == nil && view.bars(in: view.bounds).isEmpty)
        view.show(url: url, status: nil)
        #expect(view.waveform == nil)
    }

    @Test func theLogWaitsForStreamingDecodeEvenAfterTheFirstSound() throws {
        let log = AuditionLog()
        var entries: [AuditionEntry] = []
        log.write = { entries.append($0) }
        log.record(try status())
        #expect(entries.isEmpty)
        log.record(try status(complete: true, decoded: true))
        #expect(entries.count == 1 && entries[0].decodedUs == 12 && entries[0].heardUs == 9)
    }
}
