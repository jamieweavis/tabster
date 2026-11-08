import Foundation
import Observation
import QuartzCore

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

    static let speeds: [Double] = stride(from: 50, through: 125, by: 5).map { Double($0) / 100 }
    static let scaleRange: ClosedRange<Float> = 0.6...2.0

    let core: Core
    let tabID: Int64

    private(set) var phase: Phase = .loading
    private(set) var info: TabInfo?
    private(set) var track: UInt32 = 0
    private(set) var scale: Float = 1
    private(set) var showRhythm: Bool = UserDefaults.standard.object(forKey: "viewer.showRhythm") as? Bool ?? true
    private(set) var isPlaying = false
    /// Playback position for the transport UI, refreshed ~10× a second while playing.
    private(set) var displayTime: Double = 0
    private(set) var speed: Double = 1
    /// Inclusive master-bar loop range.
    private(set) var loop: ClosedRange<UInt32>?
    private(set) var hasLayout = false
    var showVideo = false

    var duration: Double { info?.durationSecs ?? 0 }
    var currentTrack: TrackInfo? { info?.tracks.first { $0.index == track } }

    @ObservationIgnored private(set) var session: TabSession?
    @ObservationIgnored weak var sink: TabCanvasSink? {
        didSet { pushState() }
    }
    @ObservationIgnored private var clock: PlaybackClock = InternalClock()
    @ObservationIgnored private var playedBars: [PlayedBar] = []
    @ObservationIgnored private var position: PlaybackPosition?
    @ObservationIgnored private var list: DisplayList?
    @ObservationIgnored private var viewportWidth: Float = 0
    @ObservationIgnored private var resizeTask: Task<Void, Never>?
    @ObservationIgnored private var saveTask: Task<Void, Never>?
    @ObservationIgnored private var layoutGeneration = 0
    @ObservationIgnored private let worker = LayoutWorker()
    @ObservationIgnored private var lastDisplayUpdate: CFTimeInterval = 0
    @ObservationIgnored private var loopSpan: Range<Double>?
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
                let bars = (0..<info.playedBarCount).map { i in
                    let start = session.scoreSecsForPlayedBar(playedBar: i)
                    // Nudge past the boundary so float rounding can't land in the previous bar.
                    let master = session.positionAtScore(scoreSecs: start + 1e-6)?.masterBar ?? 0
                    return PlayedBar(startSecs: start, masterBar: master)
                }
                var list: DisplayList?
                if width > 0 {
                    let track = state.map(\.track).flatMap { t in info.tracks.contains { $0.index == t } ? t : nil } ?? 0
                    list = session.layout(options: LayoutOptions(track: track, width: width, scale: scale, showRhythm: showRhythm))
                }
                return .success(Opened(session: session, info: info, state: state, playedBars: bars, list: list,
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
            clock.seek(to: state.lastPositionSecs)
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
        position = session?.positionAtScore(scoreSecs: clock.currentTime)
        sink?.show(list, anchorBar: first ? nil : anchor)
        if first {
            updateLoopSpan(near: clock.currentTime)
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
        var t = clock.currentTime
        if t >= duration - 0.01 { t = 0 }
        updateLoopSpan(near: t)
        if let span = loopSpan, !span.contains(t) { t = span.lowerBound }
        clock.seek(to: t)
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
        displayTime = clock.currentTime
        scheduleSave(immediately: true)
    }

    func seek(to secs: Double) {
        let t = min(max(0, secs), duration)
        clock.seek(to: t)
        displayTime = t
        updateLoopSpan(near: t)
        refresh(follow: true)
        if !isPlaying { scheduleSave() }
    }

    /// Per-frame update from the canvas's display link. Returns the cursor to draw.
    func frameTick() -> Cursor? {
        guard isPlaying, let session else { return position?.cursor }
        var t = clock.currentTime
        if let span = loopSpan, t >= span.upperBound {
            t = span.lowerBound
            clock.seek(to: t)
        } else if t >= duration {
            t = duration
            pause()
            clock.seek(to: t)
        }
        position = session.positionAtScore(scoreSecs: t)
        let now = CACurrentMediaTime()
        if now - lastDisplayUpdate > 0.1 {
            lastDisplayUpdate = now
            displayTime = t
        }
        return position?.cursor
    }

    var currentTime: Double { clock.currentTime }

    private func refresh(follow: Bool) {
        position = session?.positionAtScore(scoreSecs: clock.currentTime)
        sink?.moveCursor(position?.cursor, follow: follow)
    }

    /// Seek to a click in layout coordinates.
    func seek(atX x: Float, y: Float) {
        guard let session,
              let secs = session.scoreSecsAtPoint(x: x, y: y, nearScoreSecs: clock.currentTime)
        else { return }
        seek(to: secs)
    }

    private var currentPlayedIndex: Int {
        if let p = position?.playedBar { return Int(p) }
        let t = clock.currentTime
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
        let target = clock.currentTime - playedBars[i].startSecs < 0.3 / max(speed, 0.1) ? max(0, i - 1) : i
        seek(to: playedBars[target].startSecs)
    }

    func setSpeed(_ value: Double) {
        speed = value
        clock.rate = value
        scheduleSave()
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
        updateLoopSpan(near: clock.currentTime)
        sink?.showLoop(range)
        if isPlaying, let span = loopSpan, !span.contains(clock.currentTime) {
            clock.seek(to: span.lowerBound)
        }
        scheduleSave()
    }

    /// Converts the master-bar loop into a score-time span: the pass through the loop's first bar
    /// at or before `t` (or the first pass), up to the end of the loop's last bar after it.
    private func updateLoopSpan(near t: Double) {
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
    }

    // MARK: - Practice state

    private func pushState() {
        guard let sink else { return }
        if let list { sink.show(list, anchorBar: nil) }
        sink.showLoop(loop)
        sink.moveCursor(position?.cursor, follow: false)
        sink.playingChanged(isPlaying)
    }

    private func scheduleSave(immediately: Bool = false) {
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
            lastPositionSecs: clock.currentTime
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
        if isPlaying {
            clock.pause()
            isPlaying = false
            sink?.playingChanged(false)
        }
        saveTask?.cancel()
        resizeTask?.cancel()
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
