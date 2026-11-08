import Foundation
import Observation
import QuartzCore
import TabsterKit

/// What the canvas needs pushed to it imperatively (not through SwiftUI, so per-frame cursor
/// updates never re-evaluate a view body).
@MainActor
protocol TabCanvasSink: AnyObject {
    /// A new layout. `anchorBar`: keep this master bar in view (nil = leave the scroll position).
    func show(_ list: DisplayList, anchorBar: UInt32?)
    /// Move the cursor. `follow`: scroll so the cursor's system is comfortably in view.
    func moveCursor(_ cursor: Cursor?, follow: Bool)
    func showLoop(_ bars: ClosedRange<UInt32>?)
    func playingChanged(_ playing: Bool)
}

/// One played bar (repeats unrolled): where it starts and which master bar it plays.
struct PlayedBar: Sendable {
    let startSecs: Double
    let masterBar: UInt32
}

/// Serialises layout calls so the session's stored layout (used for the cursor and hit-testing)
/// is always the last one the shell applies.
actor LayoutWorker {
    func layout(_ session: TabSession, _ options: LayoutOptions) -> (DisplayList, Double) {
        let start = CACurrentMediaTime()
        let list = session.layout(options: options)
        return (list, (CACurrentMediaTime() - start) * 1000)
    }
}

/// State and behaviour of one open tab: session, layout, transport, loop and practice state.
@MainActor
@Observable
final class TabViewerModel {
    enum Phase {
        case loading
        case ready
        case failed(String)
    }

    /// Speeds on the internal clock: 50–125 % in 5 % steps.
    static let speeds: [Double] = stride(from: 50, through: 125, by: 5).map { Double($0) / 100 }
    /// Speeds while a YouTube video is the clock. YouTube only plays its own rates (0.25–2 in
    /// 0.25 steps; it accepted 1.3 off-list in the spike, but only the listed rates are
    /// documented), and below 50 % its audio is no use for practice.
    static let videoSpeeds: [Double] = [0.5, 0.75, 1, 1.25]
    static let scaleRange: ClosedRange<Float> = 0.6...2.0

    let core: Core
    let tabID: Int64

    private(set) var phase: Phase = .loading
    private(set) var info: TabInfo?
    private(set) var track: UInt32 = 0
    private(set) var scale: Float = 1
    private(set) var showRhythm: Bool = UserDefaults.standard.object(forKey: "viewer.showRhythm") as? Bool ?? true
    var isPlaying = false {
        didSet {
            guard isPlaying != oldValue else { return }
            practicePlayingChanged()
            if !isPlaying, usingVideoClock { synthClock?.stopFollowing() }
        }
    }
    /// Playback position for the transport UI, refreshed ~10× a second while playing.
    var displayTime: Double = 0
    var speed: Double = 1
    /// Inclusive master-bar loop range.
    private(set) var loop: ClosedRange<UInt32>?
    private(set) var hasLayout = false

    // Video and sync (see TabViewerModel+Video.swift).
    /// The tab's attached YouTube videos.
    var videos: [Video] = []
    var selectedVideoID: Int64?
    /// The selected video's sync, as applied to the session.
    var sync: SyncSpec = .offset(offsetSecs: 0, stretch: 1)
    /// The player, created when the tab first shows a video.
    var player: YouTubePlayerController?
    /// A YouTube video drives playback (false: the internal clock, e.g. no video or it failed).
    var usingVideoClock = false
    /// The sync editor (nudge bar, ruler) is open.
    var syncEditing = false
    var showAttachSheet = false
    var showPointsSheet = false
    /// Bumped on every sync change, so views that draw the sync (the ruler) refresh.
    var syncRevision = 0
    /// The last sync tap (played bar, video time, host time), for feedback in the sync bar.
    var lastTap: (bar: Int, video: Double, at: Double)?

    var duration: Double { info?.durationSecs ?? 0 }
    var currentTrack: TrackInfo? { info?.tracks.first { $0.index == track } }

    @ObservationIgnored private(set) var session: TabSession?
    @ObservationIgnored weak var sink: TabCanvasSink? {
        didSet { pushState() }
    }
    @ObservationIgnored var clock: PlaybackClock = InternalClock()
    /// The tab synth, once its SoundFont has loaded (nil if the app has none).
    @ObservationIgnored var synthClock: SynthPlaybackClock?
    var synthAvailable = false
    /// Hear the tab through the synth when no video is the clock. Remembered across tabs.
    var tabAudio = UserDefaults.standard.object(forKey: "tabAudio") as? Bool ?? true {
        didSet {
            UserDefaults.standard.set(tabAudio, forKey: "tabAudio")
            configureSynthLayers()
            if !usingVideoClock { adoptInternalClock() }
        }
    }
    /// Click along: over the tab audio, on its own, or over a video's audio (the synth then
    /// follows the video clock with its notes muted).
    var metronome = false {
        didSet {
            configureSynthLayers()
            if !usingVideoClock, !tabAudio { adoptInternalClock() }
        }
    }
    @ObservationIgnored private(set) var playedBars: [PlayedBar] = []
    @ObservationIgnored var position: PlaybackPosition?

    // Practice tools (see TabViewerModel+Practice.swift).
    /// The tab's saved loop sections.
    var savedLoops: [SavedLoop] = []
    /// The running speed trainer (needs a loop).
    var trainer: SpeedTrainer?
    var trainerSettings = SpeedTrainer.Settings(
        string: UserDefaults.standard.string(forKey: "viewer.trainer") ?? ""
    ) ?? SpeedTrainer.Settings() {
        didSet { UserDefaults.standard.set(trainerSettings.string, forKey: "viewer.trainer") }
    }
    /// A bar of clicks before playing (synth clock only). Remembered across tabs.
    var countIn = UserDefaults.standard.bool(forKey: "viewer.countIn") {
        didSet {
            UserDefaults.standard.set(countIn, forKey: "viewer.countIn")
            synthClock?.countInBars = countIn ? 1 : 0
        }
    }
    var practiceStats: PracticeStats?
    /// The practice popover is open.
    var showPractice = false
    @ObservationIgnored var practiceTimer = PracticeTimer()
    @ObservationIgnored var practiceFlushTask: Task<Void, Never>?
    /// Score time of the last frame, to notice the clock wrapping the loop on its own (the synth).
    @ObservationIgnored var lastTickScore: Double?
    /// Debug harness: called on every loop pass the trainer counts.
    @ObservationIgnored var trainerObserver: ((SpeedTrainer, SpeedTrainer.Event) -> Void)?
    /// Video time of every played bar's start under the current sync (for taps and the ruler).
    @ObservationIgnored var barVideoTimes: [Double] = []
    /// Bars the user placed explicitly in per-bar mode (taps, drags).
    @ObservationIgnored var syncKeyframes: Set<Int> = []
    @ObservationIgnored var syncSaveTask: Task<Void, Never>?
    @ObservationIgnored var pendingSyncSave: (id: Int64, spec: SyncSpec)?
    /// Start the player muted (debug harness).
    @ObservationIgnored var startMuted = false
    /// Debug harness: called every display frame with (host time, clock time, score time, played bar).
    @ObservationIgnored var frameObserver: ((Double, Double, Double, UInt32?) -> Void)?
    @ObservationIgnored private var list: DisplayList?
    @ObservationIgnored private var viewportWidth: Float = 0
    @ObservationIgnored private var resizeTask: Task<Void, Never>?
    @ObservationIgnored private var saveTask: Task<Void, Never>?
    @ObservationIgnored private var layoutGeneration = 0
    @ObservationIgnored private let worker = LayoutWorker()
    @ObservationIgnored private var lastDisplayUpdate: CFTimeInterval = 0
    @ObservationIgnored private(set) var loopSpan: Range<Double>?
    @ObservationIgnored private var restoredPosition: Double?
    @ObservationIgnored private let openedAt = CACurrentMediaTime()
    @ObservationIgnored private var firstLayoutDone = false
    /// Debug harness: start playing as soon as the first layout lands.
    @ObservationIgnored var autoplay = false

    init(core: Core, tabID: Int64) {
        self.core = core
        self.tabID = tabID
    }

    // MARK: - Opening

    func open() async {
        guard session == nil else { return }
        let core = core
        let id = tabID
        // Lay out in the same background hop when the canvas width is already known, so the tab
        // appears in the same main-thread pass as the rest of the UI.
        let width = viewportWidth
        let scale = scale
        let showRhythm = showRhythm
        let opened = await Task.detached { () -> Result<Opened, Error> in
            do {
                let start = CACurrentMediaTime()
                let session = try core.openTab(id: id)
                let openTabMs = (CACurrentMediaTime() - start) * 1000
                let info = session.info()
                let state = try? core.practiceState(tab: id)
                let bars = session.playedBars().map { PlayedBar(startSecs: $0.startSecs, masterBar: $0.masterBar) }
                let videos = (try? core.videos(tab: id)) ?? []
                var list: DisplayList?
                if width > 0 {
                    let track = state.map(\.track).flatMap { t in info.tracks.contains { $0.index == t } ? t : nil } ?? 0
                    list = session.layout(options: LayoutOptions(track: track, width: width, scale: scale, showRhythm: showRhythm))
                }
                return .success(Opened(session: session, info: info, state: state, playedBars: bars, videos: videos,
                                       list: list,
                                       openTabMs: openTabMs, workMs: (CACurrentMediaTime() - start) * 1000))
            } catch {
                return .failure(error)
            }
        }.value
        switch opened {
        case let .success(opened):
            session = opened.session
            info = opened.info
            playedBars = opened.playedBars
            if let state = opened.state { restore(state, info: opened.info) }
            phase = .ready
            loadPracticeData()
            videosLoaded(opened.videos)
            loadSynth(opened.session)
            ViewerMetrics.openMs = (CACurrentMediaTime() - openedAt) * 1000
            ViewerMetrics.openWork = (opened.openTabMs, opened.workMs)
            ViewerLog.log.info("Opened tab \(id) in \((CACurrentMediaTime() - self.openedAt) * 1000, format: .fixed(precision: 1)) ms")
            if let list = opened.list, viewportWidth == width {
                layoutGeneration += 1
                apply(list, anchor: nil)
            } else {
                relayout(anchor: nil)
            }
        case let .failure(error):
            phase = .failed("\(error)")
            ViewerLog.log.error("Failed to open tab \(id): \(error)")
        }
    }

    private struct Opened: Sendable {
        let session: TabSession
        let info: TabInfo
        let state: PracticeState?
        let playedBars: [PlayedBar]
        let videos: [Video]
        let list: DisplayList?
        let openTabMs: Double
        let workMs: Double
    }

    private func restore(_ state: PracticeState, info: TabInfo) {
        if info.tracks.contains(where: { $0.index == state.track }) { track = state.track }
        if state.speed > 0 {
            speed = Self.speeds.min { abs($0 - state.speed) < abs($1 - state.speed) } ?? 1
            clock.rate = speed
        }
        if let a = state.loopStartBar, let b = state.loopEndBar, a <= b, b < info.barCount {
            loop = a...b
        }
        if state.lastPositionSecs > 0, state.lastPositionSecs < info.durationSecs {
            restoredPosition = state.lastPositionSecs
            seekClock(toScore: state.lastPositionSecs)
            displayTime = state.lastPositionSecs
        }
    }

    // MARK: - Layout

    /// The canvas's usable width changed; relayout after it settles (~50 ms).
    func viewportWidthChanged(_ width: Float) {
        guard width > 0, abs(width - viewportWidth) >= 0.5 else { return }
        let first = viewportWidth == 0
        viewportWidth = width
        resizeTask?.cancel()
        if first {
            relayout(anchor: nil)
            return
        }
        resizeTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(50))
            guard !Task.isCancelled else { return }
            self?.relayout(anchor: nil)
        }
    }

    private func relayout(anchor: UInt32?) {
        guard let session, viewportWidth > 0 else { return }
        layoutGeneration += 1
        let generation = layoutGeneration
        let options = LayoutOptions(track: track, width: viewportWidth, scale: scale, showRhythm: showRhythm)
        let state = ViewerLog.signposter.beginInterval("layout")
        Task {
            let (list, ms) = await worker.layout(session, options)
            ViewerLog.signposter.endInterval("layout", state)
            guard generation == layoutGeneration else { return }
            ViewerMetrics.layout.add(ms)
            ViewerLog.log.debug("Layout \(list.systems.count) systems in \(ms, format: .fixed(precision: 2)) ms")
            apply(list, anchor: anchor)
        }
    }

    private func apply(_ list: DisplayList, anchor: UInt32?) {
        self.list = list
        hasLayout = true
        let first = !firstLayoutDone
        if first { ViewerMetrics.firstLayoutMs = (CACurrentMediaTime() - openedAt) * 1000 }
        firstLayoutDone = true
        position = cursorPosition(atScore: currentTime)
        sink?.show(list, anchorBar: first ? nil : anchor)
        if first {
            updateLoopSpan(near: currentTime)
            sink?.showLoop(loop)
        }
        sink?.moveCursor(position?.cursor, follow: first ? restoredPosition != nil : isPlaying)
        if first, autoplay { play() }
    }

    func setTrack(_ index: UInt32) {
        guard index != track else { return }
        track = index
        relayout(anchor: nil)
        scheduleSave()
    }

    func zoom(by delta: Float) { setScale(scale + delta) }
    func resetZoom() { setScale(1) }

    private func setScale(_ value: Float) {
        let clamped = min(max((value * 10).rounded() / 10, Self.scaleRange.lowerBound), Self.scaleRange.upperBound)
        guard clamped != scale else { return }
        scale = clamped
        relayout(anchor: nil)
    }

    func setShowRhythm(_ on: Bool) {
        guard on != showRhythm else { return }
        showRhythm = on
        UserDefaults.standard.set(on, forKey: "viewer.showRhythm")
        relayout(anchor: nil)
    }

    // MARK: - Transport

    func togglePlay() { isPlaying ? pause() : play() }

    func play() {
        guard session != nil, !isPlaying else { return }
        let now = currentTime
        var t = now
        if t >= duration - 0.01 { t = 0 }
        updateLoopSpan(near: t)
        if let span = loopSpan, !span.contains(t) { t = span.lowerBound }
        // A video keeps its own position (e.g. still in the intro, before bar 1) unless the
        // transport has to move it.
        if !usingVideoClock || t != now { seekClock(toScore: t) }
        lastTickScore = nil
        clock.play()
        isPlaying = true
        sink?.playingChanged(true)
        refresh(follow: true)
    }

    func pause() {
        guard isPlaying else { return }
        clock.pause()
        isPlaying = false
        sink?.playingChanged(false)
        refresh(follow: false)
        displayTime = clampedScore(currentTime)
        scheduleSave(immediately: true)
    }

    func seek(to secs: Double) {
        let t = min(max(0, secs), duration)
        seekClock(toScore: t)
        displayTime = t
        updateLoopSpan(near: t)
        lastTickScore = nil
        refresh(follow: true)
        if !isPlaying { scheduleSave() }
    }

    /// Per-frame update from the canvas's display link, for the frame shown at `hostTime` (the
    /// link's target timestamp). Returns the cursor to draw.
    func frameTick(at hostTime: CFTimeInterval) -> Cursor? {
        guard isPlaying, session != nil else { return position?.cursor }
        let media = clock.time(at: hostTime)
        var t = scoreTime(fromMedia: media)
        if let span = loopSpan, t >= span.upperBound {
            // Seek on crossing; the video clock jumps to the target at once and holds there
            // while YouTube catches up (~0.5 s).
            t = span.lowerBound
            seekClock(toScore: t)
            loopWrapped(span)
        } else if let span = loopSpan, let last = lastTickScore, Self.wrappedOnItsOwn(from: last, to: t, in: span) {
            // The synth loops by itself.
            loopWrapped(span)
        } else if t >= duration, !usingVideoClock {
            t = duration
            pause()
            seekClock(toScore: t)
        }
        lastTickScore = t
        if usingVideoClock { followVideo(score: t, media: media, at: hostTime) }
        // A video plays on past the tab's ends (intro, outro); the cursor waits at the edge.
        position = cursorPosition(atScore: t)
        frameObserver?(hostTime, media, t, position?.playedBar)
        let now = CACurrentMediaTime()
        if now - lastDisplayUpdate > 0.1 {
            lastDisplayUpdate = now
            displayTime = clampedScore(t)
        }
        return position?.cursor
    }

    /// Current position in score seconds (outside 0...duration while a video plays its intro or
    /// outro).
    var currentTime: Double { scoreTime(fromMedia: clock.currentTime) }

    func refresh(follow: Bool) {
        position = cursorPosition(atScore: currentTime)
        sink?.moveCursor(position?.cursor, follow: follow)
    }

    // MARK: - Time axes

    /// Score seconds for the clock's media time.
    func scoreTime(fromMedia media: Double) -> Double {
        guard usingVideoClock, let session else { return media }
        return session.videoToScore(videoSecs: media)
    }

    /// The clock's media time for score seconds.
    func mediaTime(fromScore score: Double) -> Double {
        guard usingVideoClock, let session else { return score }
        return session.scoreToVideo(scoreSecs: score)
    }

    /// Every seek goes through here, so it lands on the clock's own time axis.
    func seekClock(toScore score: Double) {
        clock.seek(to: mediaTime(fromScore: score))
    }

    func clampedScore(_ t: Double) -> Double { min(max(0, t), duration) }

    func cursorPosition(atScore t: Double) -> PlaybackPosition? {
        guard let session else { return nil }
        // Keep the cursor on the tab while the video is before bar 1 or after the last bar.
        return session.positionAtScore(scoreSecs: min(max(0, t), max(0, duration - 0.0005)))
    }

    /// Seek to a click in layout coordinates.
    func seek(atX x: Float, y: Float) {
        guard let session,
              let secs = session.scoreSecsAtPoint(x: x, y: y, nearScoreSecs: currentTime)
        else { return }
        seek(to: secs)
    }

    private var currentPlayedIndex: Int {
        if let p = position?.playedBar { return Int(p) }
        let t = currentTime
        return max(0, (playedBars.lastIndex { $0.startSecs <= t } ?? 0))
    }

    func nextBar() {
        let i = currentPlayedIndex + 1
        guard i < playedBars.count else { return }
        seek(to: playedBars[i].startSecs)
    }

    func previousBar() {
        let i = currentPlayedIndex
        guard i < playedBars.count else { return }
        // Near the start of a bar, go to the previous one; otherwise restart this one.
        let target = currentTime - playedBars[i].startSecs < 0.3 / max(speed, 0.1) ? max(0, i - 1) : i
        seek(to: playedBars[target].startSecs)
    }

    /// The speeds the current clock offers.
    var speedOptions: [Double] { usingVideoClock ? Self.videoSpeeds : Self.speeds }

    func setSpeed(_ value: Double) {
        speed = Self.snap(value, to: speedOptions)
        clock.rate = speed
        scheduleSave()
    }

    static func snap(_ value: Double, to options: [Double]) -> Double {
        options.min { abs($0 - value) < abs($1 - value) } ?? 1
    }

    // MARK: - Loop

    var cursorBar: UInt32? { position?.masterBar }

    func setLoopStart() {
        guard let bar = cursorBar else { return }
        let end = max(loop?.upperBound ?? bar, bar)
        setLoop(bar...end)
    }

    func setLoopEnd() {
        guard let bar = cursorBar else { return }
        let start = min(loop?.lowerBound ?? bar, bar)
        setLoop(start...bar)
    }

    func setLoop(_ range: ClosedRange<UInt32>?) {
        loop = range
        if range == nil { stopTrainer() }
        lastTickScore = nil
        updateLoopSpan(near: currentTime)
        sink?.showLoop(range)
        if isPlaying, let span = loopSpan, !span.contains(currentTime) {
            seekClock(toScore: span.lowerBound)
        }
        scheduleSave()
    }

    /// Converts the master-bar loop into a score-time span: the pass through the loop's first bar
    /// at or before `t` (or the first pass), up to the end of the loop's last bar after it.
    func updateLoopSpan(near t: Double) {
        guard let loop, !playedBars.isEmpty else {
            loopSpan = nil
            return
        }
        let starts = playedBars.indices.filter { playedBars[$0].masterBar == loop.lowerBound }
        guard let first = starts.last(where: { playedBars[$0].startSecs <= t + 1e-6 }) ?? starts.first else {
            loopSpan = nil
            return
        }
        let endIndex = playedBars[first...].firstIndex { $0.masterBar == loop.upperBound } ?? first
        let end = endIndex + 1 < playedBars.count ? playedBars[endIndex + 1].startSecs : duration
        loopSpan = playedBars[first].startSecs..<end
        synthClock?.synth.setLoop(startSecs: loopSpan?.lowerBound, endSecs: loopSpan?.upperBound)
    }

    /// Load the synth in the background; adopt it as the clock if nothing else is driving.
    private func loadSynth(_ session: TabSession) {
        Task { [weak self] in
            guard let clock = await SynthPlaybackClock.make(session: session), let self else { return }
            clock.countInBars = self.countIn ? 1 : 0
            clock.synth.setLoop(startSecs: self.loopSpan?.lowerBound, endSecs: self.loopSpan?.upperBound)
            self.synthClock = clock
            self.synthAvailable = true
            self.configureSynthLayers()
            if !self.usingVideoClock && (self.tabAudio || self.metronome) { self.adoptInternalClock() }
        }
    }

    /// What the synth sounds: the metronome if on, and the tab's notes only when tab audio is
    /// on and no video is playing its own audio.
    func configureSynthLayers() {
        guard let synth = synthClock?.synth else { return }
        synth.setMetronome(enabled: metronome)
        synth.setNotesMuted(muted: usingVideoClock || !tabAudio)
    }

    /// While a video is the clock, keep the synth's metronome on the video's beat.
    private func followVideo(score t: Double, media: Double, at hostTime: CFTimeInterval) {
        guard let synthClock else { return }
        guard metronome, player?.state == .playing else {
            synthClock.follow(target: nil, rate: 0, at: hostTime)
            return
        }
        // Score seconds per second: the video's rate × the sync map's local slope.
        let slope = (scoreTime(fromMedia: media + 0.1) - scoreTime(fromMedia: media)) / 0.1
        synthClock.follow(target: t, rate: clock.rate * slope, at: hostTime)
    }

    // MARK: - Practice state

    private func pushState() {
        guard let sink else { return }
        if let list { sink.show(list, anchorBar: nil) }
        sink.showLoop(loop)
        sink.moveCursor(position?.cursor, follow: false)
        sink.playingChanged(isPlaying)
    }

    func scheduleSave(immediately: Bool = false) {
        saveTask?.cancel()
        saveTask = Task { [weak self] in
            if !immediately {
                try? await Task.sleep(for: .milliseconds(500))
                guard !Task.isCancelled else { return }
            }
            self?.save()
        }
    }

    /// Persist track, speed, loop and position now (also called when the viewer disappears).
    /// `synchronously` for app termination, when a background task wouldn't get to run.
    func save(synchronously: Bool = false) {
        guard session != nil else { return }
        let state = PracticeState(
            track: track,
            speed: speed,
            loopStartBar: loop?.lowerBound,
            loopEndBar: loop?.upperBound,
            lastPositionSecs: clampedScore(currentTime)
        )
        let core = core
        let id = tabID
        let write: @Sendable () -> Void = {
            do {
                try core.setPracticeState(tab: id, state: state)
            } catch {
                ViewerLog.log.error("Saving practice state failed: \(error)")
            }
        }
        if synchronously {
            write()
        } else {
            Task.detached(operation: write)
        }
    }

    func close(synchronously: Bool = false) {
        // Before pausing, which would otherwise write it in the background.
        flushPractice(synchronously: synchronously)
        if isPlaying || usingVideoClock {
            clock.pause()
            isPlaying = false
            sink?.playingChanged(false)
        }
        saveTask?.cancel()
        resizeTask?.cancel()
        flushSyncSave(synchronously: synchronously)
        save(synchronously: synchronously)
    }

    /// Called by the canvas after the first draw of the first layout.
    func didFirstRender() {
        guard ViewerMetrics.firstRenderMs == nil else { return }
        let ms = (CACurrentMediaTime() - openedAt) * 1000
        ViewerMetrics.firstRenderMs = ms
        ViewerLog.log.info("First render \(ms, format: .fixed(precision: 1)) ms after open")
    }
}
