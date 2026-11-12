import EchodeBridge
import Foundation
import Observation

/// One open document. Each open makes a new value, so reopening a project never
/// matches input captured from the earlier one. Operations on a document take
/// this without a default: nothing can mean "whatever is open by then" (ADR 0203).
public struct DocumentID: Hashable, Sendable, Codable {
    private let value: UUID
    init() { value = UUID() }
}

/// Everything that lasts exactly as long as one open document. `open` and `close`
/// replace the whole object, so nothing is reset by hand, and work still running
/// for an old document can only reach that document's state.
@MainActor @Observable
final class DocumentSession {
    let id: DocumentID?
    let projectURL: URL?
    let playback: PlaybackFeed?
    @ObservationIgnored let document: Document?
    @ObservationIgnored private let auditionLog: AuditionLog

    private(set) var snapshot: Snapshot?
    private(set) var tracks: [TrackRow] = []
    var undoCount = 0
    var redoCount = 0

    private var trackSelection = TrackSelection()
    var selectedTracks: Set<UUID> { trackSelection.ids }
    var selection: UUID? {
        get { trackSelection.focus }
        set {
            let previous = selection
            trackSelection.replace(newValue.flatMap { id in tracks.contains(where: { $0.id == id }) ? id : nil })
            if selection != previous { clipFocus = nil }
        }
    }
    /// The piano input target is independent of arrange/device selection.
    var pianoTrack: UUID?
    var pianoLocation: PianoLocation?
    /// Hidden reference tracks for this document; new tracks start visible.
    var hiddenPianoTracks: Set<UUID> = []
    /// Shared by all piano tracks; never serialized into the project.
    var noteInput = NoteInputLength()
    var noteVelocity = NoteInputVelocity()
    private(set) var noteSelection: Set<PianoNote.ID> = []
    var noteFocus: PianoNote.ID? {
        get { noteSelection.count == 1 ? noteSelection.first : nil }
        set { selectNotes(Set(newValue.map { [$0] } ?? [])) }
    }
    var clipFocus: UUID? {
        didSet { if let clipFocus, snapshot?.clipMix(clipFocus)?.track != selection { self.clipFocus = nil } }
    }

    var faderPreview: FaderPreview?
    @ObservationIgnored var previewPending: FaderPreview?
    @ObservationIgnored var previewTask: Task<Void, Never>?
    @ObservationIgnored var previewFinishing = false
    @ObservationIgnored var previewFinishTask: Task<Void, Never>?

    /// The output's reply is kept in parts observed apart (ADR 0289). `receive`
    /// is their only writer. `@Observable` does not notify for an equal value,
    /// so a playback poll or an audition request leaves the other parts' readers alone.
    private(set) var audio: AudioStatus
    /// Each sample audition request changes this; the browser reads it.
    private(set) var audition: AuditionStatus?
    /// The polled position in ticks. Playback polls change only this, and only
    /// the transport's position display reads it.
    private(set) var position: Int64
    /// Observed apart from the status so play and stop do not rebuild the device strip.
    private(set) var outputOpening: Bool
    /// The open output prepares a replacement; observed apart like `outputOpening`.
    private(set) var outputUpdating: Bool
    /// The remembered start in ticks, observed apart so the rulers redraw only
    /// when it changes (ADR 0155).
    private(set) var playbackStart: Int64
    /// An audio command is in flight. It never disables controls: commands
    /// keep their order instead (ADR 0160).
    var audioBusy = false
    /// Integer loop bounds shared by the rulers and transport. Not part of the project.
    var loopRange: LoopRange = .initial
    /// Bumps on play, pause, stop, and locate. Position polling does not.
    var transportGeneration: UInt64 = 0
    var pendingPlayback: Bool?
    @ObservationIgnored var audioRequests = 0
    /// The last audio command sent or waiting. Ordered commands start after it.
    @ObservationIgnored var audioTail: Task<Void, Never>?
    @ObservationIgnored var audioRevision: UInt64 = 0
    @ObservationIgnored var reportedAudioError: String?

    /// Parameter lists the device view has shown in this document, by device.
    /// Values change inside each state; this map changes only when a list is
    /// added or a device leaves the document.
    var parameterStates: [UUID: DeviceParameterState] = [:]
    /// Presentation only, keyed by device identity for this open document.
    var collapsedDevices: Set<UUID> = []
    /// Confirmed helper window lifetimes. Never saved or restored by Undo.
    var openPluginWindows: Set<UUID> = []
    /// Undo, Redo or a restored revision may load plugin bytes. A parameter
    /// drag waits for that instead of racing it; other edits do not.
    var historyBusy = false
    @ObservationIgnored var parameterGesture: ParameterGesture?
    @ObservationIgnored var parameterTask: Task<Void, Never>?

    @ObservationIgnored var pianoStepTail: Task<Bool, Never>?
    @ObservationIgnored var mixerSwitchTail: Task<Bool, Never>?
    /// Reads plugin windows' activity for this document (ADR 0132).
    @ObservationIgnored var pluginActivity: PluginActivityReader?
    /// Parameters each plugin reports as being adjusted in its own window.
    @ObservationIgnored var pluginGestures: [UUID: Set<UInt32>] = [:]
    /// A plugin-made change without a gesture waits for quiet.
    @ObservationIgnored var pluginQuiet: [UUID: Task<Void, Never>] = [:]
    /// Plugin-made edits are recorded one after another, in arrival order.
    @ObservationIgnored var pluginRecordTask: Task<Void, Never>?
    /// The window a single add opens, after the add finished.
    @ObservationIgnored var pluginWindowTask: Task<Void, Never>?

    /// The state while no document is open.
    init(auditionLog: AuditionLog) {
        id = nil
        projectURL = nil
        playback = nil
        document = nil
        self.auditionLog = auditionLog
        audio = AudioStatus()
        audition = nil
        position = 0
        outputOpening = false
        outputUpdating = false
        playbackStart = 0
    }

    init(opening document: Document, at url: URL, audio: AudioState, auditionLog: AuditionLog) {
        id = DocumentID()
        projectURL = url
        playback = document.playbackFeed
        self.document = document
        self.auditionLog = auditionLog
        self.audio = audio.status
        audition = audio.audition
        position = audio.tick
        outputOpening = audio.opening
        outputUpdating = audio.updating
        playbackStart = audio.start
    }

    /// Takes one reply of the output. Every reply goes through here.
    func receive(_ state: AudioState) {
        audio = state.status
        if audition != state.audition { auditionLog.record(state.audition) }
        audition = state.audition
        position = state.tick
        outputOpening = state.opening
        outputUpdating = state.updating
        playbackStart = state.start
    }

    /// Stops what would otherwise keep running for a replaced document. Replies
    /// still on their way find no gesture or current revision to act on.
    func retire() {
        audioRevision &+= 1
        pluginActivity?.stop()
        pluginActivity = nil
        for task in pluginQuiet.values { task.cancel() }
        pluginQuiet = [:]
        parameterGesture = nil
        previewPending = nil
    }

    func selectTrack(_ id: UUID, change: TrackSelectionChange, visibleOrder: [UUID]?) {
        let previous = selection
        let order =
            visibleOrder.map { visible in
                let existing = Set(tracks.map(\.id))
                return visible.filter { existing.contains($0) }
            } ?? tracks.map(\.id)
        trackSelection.select(id, change: change, order: order)
        if selection != previous { clipFocus = nil }
    }

    func selectNotes(_ ids: Set<PianoNote.ID>) {
        let previous = noteSelection
        noteSelection = ids.filter { $0.track == pianoTrack && snapshot?.pianoNote($0) != nil }
        if let noteFocus, let note = snapshot?.pianoNote(noteFocus) {
            noteInput.remember(note.length)
            noteVelocity.remember(Int(note.velocity >> 9))
        }
        if noteSelection != previous, noteFocus == nil { noteVelocity.invalidate() }
    }

    /// Selects notes an edit just made, without changing what input remembers.
    func selectEdited(_ ids: Set<PianoNote.ID>, in edited: Snapshot) {
        noteSelection = ids.filter { $0.track == pianoTrack && edited.pianoNote($0) != nil }
    }

    /// Selects each added track whose parent was not added with it, focusing the first.
    func selectAddedRoots(previous: Set<UUID>) {
        guard let snapshot else { return }
        var added: Set<UUID> = []
        var roots: [UUID] = []
        for row in 0..<snapshot.trackCount {
            guard let id = snapshot.trackID(at: row), !previous.contains(id) else { continue }
            added.insert(id)
            if snapshot.trackHierarchy(at: row)?.parent.map({ !added.contains($0) }) ?? true { roots.append(id) }
        }
        guard let last = roots.last else { return }
        // Toggling ends on the first root, so it becomes the focus.
        selection = last
        for id in roots.dropLast().reversed() { selectTrack(id, change: .toggle, visibleOrder: nil) }
    }

    func publish(_ snapshot: Snapshot) {
        self.snapshot = snapshot
        tracks = (0..<snapshot.trackCount).compactMap { index in
            guard let id = snapshot.trackID(at: index), let name = snapshot.trackName(at: index) else { return nil }
            return TrackRow(id: id, name: name)
        }
        let previous = selection
        trackSelection.reconcile(order: tracks.map(\.id))
        if selection != previous { clipFocus = nil }
        if !tracks.contains(where: { $0.id == pianoTrack }) { pianoTrack = selection }
        hiddenPianoTracks.formIntersection(Set(tracks.map(\.id)))
        noteSelection = noteSelection.filter { $0.track == pianoTrack && snapshot.pianoNote($0) != nil }
        if let clipFocus, snapshot.clipMix(clipFocus) == nil { self.clipFocus = nil }
        if !parameterStates.isEmpty || !openPluginWindows.isEmpty {
            // A removed or offline device has no running plugin to list.
            var running: Set<UUID> = []
            for track in 0..<snapshot.trackCount {
                for device in 0..<snapshot.deviceCount(onTrack: track)
                where snapshot.deviceOffline(onTrack: track, at: device) == false {
                    if snapshot.devicePlugin(onTrack: track, at: device) != nil,
                        let id = snapshot.deviceID(onTrack: track, at: device)
                    {
                        running.insert(id)
                    }
                }
            }
            openPluginWindows.formIntersection(running)
            for id in parameterStates.keys where !running.contains(id) { parameterStates[id] = nil }
            if let gesture = parameterGesture, !running.contains(gesture.device) { parameterGesture = nil }
        }
    }
}
