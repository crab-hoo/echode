import AppKit
import EchodeBridge
import Foundation
import Synchronization
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

    @Test func contractAuditionFailureRemainsVisibleWithACompleteWaveform() throws {
        let view = SamplePreviewView()
        let url = URL(fileURLWithPath: "/samples/kick.wav")
        let ready = try status(complete: true)
        let waveform = try #require(ready.waveform)
        let error = "sample audition muted: output exceeded +18 dBFS or became non-finite; select a sample to try again"
        let json: [String: Any] = [
            "request": 1, "handled": 1, "path": url.path, "phase": "failed", "error": error, "history": [],
        ]
        let failed = try JSONDecoder().decode(AuditionStatus.self, from: JSONSerialization.data(withJSONObject: json))
        view.show(url: url, status: failed, overview: .ready(waveform))
        #expect(view.waveform == waveform)
        #expect(view.toolTip == error)
        #expect((view.accessibilityValue() as? String)?.contains(text("Sample audition stopped")) == true)
        #expect((view.accessibilityValue() as? String)?.contains(error) == true)
        view.show(url: URL(fileURLWithPath: "/samples/snare.wav"), status: failed, overview: .ready(waveform))
        #expect(view.toolTip == nil)
        #expect((view.accessibilityValue() as? String)?.contains(error) == false)
        view.show(url: url, status: ready, overview: .ready(waveform))
        #expect(view.toolTip == nil)
        #expect((view.accessibilityValue() as? String)?.contains(error) == false)
    }

    /// Selecting a sample reads its whole waveform without an audition. It
    /// stays after the audition stops, switching back shows it without reading
    /// again, and a changed file is read again (ADR 0163). The reads go through
    /// the product bridge; only the count is recorded.
    @Test func contractSelectedSampleShowsItsWholeWaveformBeforeAndAfterAudition() async throws {
        let folder = FileManager.default.temporaryDirectory.resolvingSymlinksInPath().appendingPathComponent(
            "echode-overview-\(UUID())")
        defer { removeTestFile(folder) }
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        for (name, frames) in [("kick.wav", 96_000), ("snare.wav", 4_800), ("hat.wav", 2_400)] {
            try wavMono(rate: 48_000, frames: frames).write(to: folder.appendingPathComponent(name))
        }
        let reads = Mutex<[String]>([])
        let overviews = SampleOverviews(keep: 2) { url, channel, request in
            reads.withLock { $0.append(url.lastPathComponent) }
            return SampleOverview.read(url, channel: channel, request: request)
        }
        let browser = FileBrowserState(overviews: overviews)
        await browser.load(folder)
        let rows = Dictionary(uniqueKeysWithValues: browser.filtered.map { ($0.url.lastPathComponent, $0.url) })
        let kick = try #require(rows["kick.wav"])
        let snare = try #require(rows["snare.wav"])
        let hat = try #require(rows["hat.wav"])
        func select(_ url: URL) async {
            browser.selection = url
            await browser.overviewTask?.value
        }

        await select(kick)
        guard case .ready(let whole) = overviews.entry(for: kick) else {
            Issue.record("no overview")
            return
        }
        #expect(whole.complete && whole.frames == 96_000 && whole.loadedFrames == 96_000 && whole.sampleRate == 48_000)
        let view = SamplePreviewView()
        view.frame = NSRect(x: 0, y: 0, width: 200, height: 86)
        // Nothing has sounded yet; the whole file is drawn.
        view.show(url: kick, status: nil, overview: overviews.entry(for: kick))
        #expect(view.waveform == whole && !view.bars(in: NSRect(x: 0, y: 0, width: 200, height: 40)).isEmpty)
        #expect((view.accessibilityValue() as? String)?.contains("2.00 s") == true)
        // A partial audition of the same file does not replace it, nor does its end.
        let playing = try JSONDecoder().decode(
            AuditionStatus.self,
            from: Data(
                #"""
                {"request":1,"handled":1,"path":"\#(kick.path)","phase":"sounding",
                 "history":[{"request":1,"input":"click","requested_at_ms":1,"phase":"sounding","opened_output":false,"start_frame":24000}],
                 "waveform":{"frames":96000,"loaded_frames":4800,"first_frame":0,"sample_rate":48000,"frames_per_peak":188,"complete":false,"peaks":[[0,0]]}}
                """#.utf8))
        view.show(url: kick, status: playing, overview: overviews.entry(for: kick))
        #expect(view.waveform == whole)
        view.show(url: kick, status: nil, overview: overviews.entry(for: kick))
        #expect(view.waveform == whole)

        await select(snare)
        await select(kick)
        #expect(reads.withLock { $0 } == ["kick.wav", "snare.wav"])
        // A changed file is read again.
        try FileManager.default.removeItem(at: kick)
        try wavMono(rate: 48_000, frames: 48_000).write(to: kick)
        try FileManager.default.setAttributes(
            [.modificationDate: Date().addingTimeInterval(60)], ofItemAtPath: kick.path)
        await select(snare)
        await select(kick)
        guard case .ready(let changed) = overviews.entry(for: kick) else {
            Issue.record("no reread")
            return
        }
        #expect(changed.frames == 48_000)
        // Two are kept here; the oldest is read again when it comes back.
        await select(hat)
        await select(snare)
        #expect(reads.withLock { $0 } == ["kick.wav", "snare.wav", "kick.wav", "hat.wav", "snare.wav"])
        // A folder or a missing file has no overview and no read.
        browser.selection = nil
        #expect(overviews.entry(for: folder) == nil)
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
