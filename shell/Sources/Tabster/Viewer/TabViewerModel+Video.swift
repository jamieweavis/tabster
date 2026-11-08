import Foundation
import QuartzCore
import TabsterKit

/// The video side of the viewer: which video plays, which clock drives the cursor, and the sync
/// editor's actions.
///
/// While a video is shown and healthy, YouTube is the clock (`VideoPlaybackClock`, in video
/// seconds) and every score ↔ media conversion goes through the session's sync map. With no
/// video, the pane hidden, or a player failure, the `InternalClock` (score seconds) takes over
/// at the same score position.
extension TabViewerModel {
    static let showVideoKey = "viewer.showVideo"

    var selectedVideo: Video? { videos.first { $0.id == selectedVideoID } }

    // MARK: - Videos

    /// The session opened: show the default video (the default is how a choice persists).
    func videosLoaded(_ list: [Video]) {
        videos = list
        showVideo = UserDefaults.standard.object(forKey: Self.showVideoKey) as? Bool ?? true
        if let video = list.first(where: \.isDefault) ?? list.first {
            select(video)
        }
    }

    /// Show or hide the video pane (remembered for the app). Hidden means no video playback.
    func setVideoVisible(_ visible: Bool) {
        UserDefaults.standard.set(visible, forKey: Self.showVideoKey)
        guard visible != showVideo else { return }
        showVideo = visible
        if !visible { syncEditing = false }
        updateClockSource()
    }

    func select(_ video: Video) {
        let score = currentTime
        let wasPlaying = isPlaying
        selectedVideoID = video.id
        syncKeyframes = []
        applySync(video.sync, persist: false)
        if usingVideoClock, let player {
            // Another video (or the same one with another sync): keep the score position.
            let start = max(0, mediaTime(fromScore: score))
            if player.videoID != video.youtubeId || player.failure != nil {
                player.load(videoID: video.youtubeId, startSeconds: start, autoplay: wasPlaying)
            } else {
                player.seek(to: start)
            }
            refresh(follow: false)
        } else {
            updateClockSource()
        }
    }

    func addVideo(youtubeID: String, label: String) {
        do {
            let id = try core.addVideo(tab: tabID, urlOrId: youtubeID, label: label)
            reloadVideos()
            if let video = videos.first(where: { $0.id == id }) {
                if videos.count == 1 { try? core.setDefaultVideo(id: id) }
                setVideoVisible(true)
                select(video)
            }
        } catch {
            ViewerLog.log.error("Attaching video failed: \(error)")
        }
    }

    func removeVideo(_ video: Video) {
        do {
            if video.id == selectedVideoID { flushSyncSave(synchronously: true) }
            try core.removeVideo(id: video.id)
            reloadVideos()
            guard video.id == selectedVideoID else { return }
            selectedVideoID = nil
            if let next = videos.first(where: \.isDefault) ?? videos.first {
                select(next)
            } else {
                applySync(.offset(offsetSecs: 0, stretch: 1), persist: false)
                syncEditing = false
                updateClockSource()
            }
        } catch {
            ViewerLog.log.error("Removing video failed: \(error)")
        }
    }

    /// Make `video` the tab's default (what opens next time) and show it.
    func makeDefault(_ video: Video) {
        do {
            try core.setDefaultVideo(id: video.id)
            reloadVideos()
            if video.id != selectedVideoID, let v = videos.first(where: { $0.id == video.id }) { select(v) }
        } catch {
            ViewerLog.log.error("Setting the default video failed: \(error)")
        }
    }

    private func reloadVideos() {
        flushSyncSave(synchronously: true)
        do {
            videos = try core.videos(tab: tabID)
        } catch {
            ViewerLog.log.error("Loading videos failed: \(error)")
        }
    }

    // MARK: - Clock source

    /// YouTube drives playback when a video is selected, shown, and hasn't failed.
    private var wantsVideoClock: Bool {
        showVideo && selectedVideo != nil && player?.failure == nil
    }

    func updateClockSource() {
        if wantsVideoClock, !usingVideoClock {
            adoptVideoClock()
        } else if !wantsVideoClock, usingVideoClock {
            adoptInternalClock()
        }
    }

    private func ensurePlayer() -> YouTubePlayerController {
        if let player { return player }
        let player = YouTubePlayerController(core: core)
        if startMuted { player.setMuted(true) }
        player.onStateChange = { [weak self] state in self?.playerStateChanged(state) }
        player.onFailure = { [weak self] failure in self?.playerFailed(failure) }
        self.player = player
        return player
    }

    /// Hand playback to the video at the current score position.
    private func adoptVideoClock() {
        guard session != nil, let video = selectedVideo else { return }
        let score = currentTime
        let wasPlaying = isPlaying
        clock.pause()
        let player = ensurePlayer()
        let videoClock = VideoPlaybackClock(controller: player)
        clock = videoClock
        usingVideoClock = true
        let start = max(0, mediaTime(fromScore: score))
        if player.videoID != video.youtubeId {
            player.load(videoID: video.youtubeId, startSeconds: start, autoplay: false)
        } else {
            player.seek(to: start)
        }
        speed = Self.snap(speed, to: Self.videoSpeeds)
        if trainer != nil {
            trainer?.setRates(Self.videoSpeeds)
            speed = trainer?.speed ?? speed
        }
        if abs(player.rate - speed) > 0.001 { videoClock.rate = speed }
        if wasPlaying { videoClock.play() }
        refresh(follow: false)
        ViewerLog.log.info("Video clock: \(video.youtubeId, privacy: .public) from \(start, format: .fixed(precision: 2)) s")
    }

    /// Hand playback to the tab's own clock (the synth when tab audio is on, otherwise a silent
    /// internal clock) without moving the cursor, and keep playing if it was.
    func adoptInternalClock() {
        let score = clampedScore(currentTime)
        let wasPlaying = isPlaying
        clock.pause()
        usingVideoClock = false
        let internalClock: PlaybackClock
        if tabAudio, let synthClock {
            internalClock = synthClock
        } else {
            internalClock = InternalClock()
        }
        speed = Self.snap(speed, to: Self.speeds)
        if trainer != nil {
            trainer?.setRates(nil)
            speed = trainer?.speed ?? speed
        }
        internalClock.rate = speed
        internalClock.seek(to: score)
        if wasPlaying { internalClock.play() }
        clock = internalClock
        refresh(follow: false)
    }

    private func playerFailed(_ failure: YouTubePlayerFailure) {
        ViewerLog.log.notice("Video failed (\(String(describing: failure), privacy: .public)); using the internal clock")
        if usingVideoClock { adoptInternalClock() }
    }

    /// The player started, paused or ended, whether we asked or its own controls did.
    private func playerStateChanged(_ state: PlaybackState) {
        guard usingVideoClock, let videoClock = clock as? VideoPlaybackClock else {
            // Recovered after a failure (e.g. Retry): take over again.
            if state == .playing, wantsVideoClock { adoptVideoClock() }
            return
        }
        switch state {
        case .playing:
            if !isPlaying {
                videoClock.playerStateChanged(playing: true)
                isPlaying = true
                updateLoopSpan(near: currentTime)
                sink?.playingChanged(true)
            }
            // A new video starts at YouTube's default rate.
            if let player, abs(player.rate - speed) > 0.001 { videoClock.rate = speed }
        case .paused, .ended:
            if isPlaying {
                videoClock.playerStateChanged(playing: false)
                isPlaying = false
                sink?.playingChanged(false)
                refresh(follow: false)
                displayTime = clampedScore(currentTime)
                scheduleSave(immediately: true)
            }
        case .buffering, .unstarted:
            break
        }
    }

    /// The video time now, if a video is the clock.
    var videoNow: Double? { usingVideoClock ? clock.currentTime : nil }

    /// The video time for a display frame, if a video is the clock.
    func videoTime(at hostTime: CFTimeInterval) -> Double? {
        usingVideoClock ? clock.time(at: hostTime) : nil
    }

    // MARK: - Sync

    var isPerBarSync: Bool {
        if case .points = sync { true } else { false }
    }

    var hasFileSync: Bool { session?.fileSync() != nil }

    /// Apply `spec` to the session now; persist it for the selected video (debounced 300 ms).
    func applySync(_ spec: SyncSpec, persist: Bool = true) {
        sync = spec
        session?.setSync(spec: spec)
        barVideoTimes = session?.syncPoints().map(\.videoSecs) ?? []
        syncRevision &+= 1
        if let id = selectedVideoID, let i = videos.firstIndex(where: { $0.id == id }), videos[i].sync != spec {
            videos[i].sync = spec
        }
        if persist, let id = selectedVideoID { scheduleSyncSave(id: id, spec: spec) }
        if usingVideoClock {
            // The video stays put; the cursor moves to where the new sync says it is.
            updateLoopSpan(near: currentTime)
            if !isPlaying {
                refresh(follow: false)
                displayTime = clampedScore(currentTime)
            }
        }
    }

    private func scheduleSyncSave(id: Int64, spec: SyncSpec) {
        pendingSyncSave = (id, spec)
        syncSaveTask?.cancel()
        syncSaveTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled else { return }
            self?.flushSyncSave(synchronously: false)
        }
    }

    func flushSyncSave(synchronously: Bool) {
        syncSaveTask?.cancel()
        guard let (id, spec) = pendingSyncSave else { return }
        pendingSyncSave = nil
        let core = core
        let write: @Sendable () -> Void = {
            do {
                try core.setVideoSync(id: id, sync: spec)
            } catch {
                ViewerLog.log.error("Saving sync failed: \(error)")
            }
        }
        if synchronously { write() } else { Task.detached(operation: write) }
    }

    /// ±10 / ±100 ms: offset mode moves the offset; per-bar mode shifts every point.
    func nudgeSync(by delta: Double) {
        switch sync {
        case let .offset(offset, stretch):
            applySync(.offset(offsetSecs: offset + delta, stretch: stretch))
        case let .points(points):
            applySync(.points(points: points.map { SyncPoint(playedBar: $0.playedBar, videoSecs: $0.videoSecs + delta) }))
        }
    }

    func setStretch(_ stretch: Double) {
        guard case let .offset(offset, _) = sync else { return }
        applySync(.offset(offsetSecs: offset, stretch: stretch))
    }

    /// "Tap bar 1": bar 1 starts at the current video time.
    func tapBarOne() { tap(playedBar: 0) }

    /// "Tap": the bar boundary nearest the cursor starts at the current video time. In offset
    /// mode that sets the offset; in per-bar mode it records that bar (tap along on downbeats).
    func tapHere() {
        let starts = playedBars.map(\.startSecs)
        guard let k = SyncEditing.nearestIndex(in: starts, to: currentTime) else { return }
        tap(playedBar: k)
    }

    private func tap(playedBar k: Int) {
        guard let v = videoNow, playedBars.indices.contains(k) else { return }
        switch sync {
        case let .offset(_, stretch):
            let offset = SyncEditing.offset(placing: playedBars[k].startSecs, at: v, stretch: stretch)
            applySync(.offset(offsetSecs: offset, stretch: stretch))
        case .points:
            moveBar(k, toVideo: v)
        }
        lastTap = (k, v, CACurrentMediaTime())
    }

    /// Per-bar mode: place played bar `k` at video time `v` (a keyframe) and regenerate its
    /// neighbours up to the next keyframes. Returns false if that would break monotonicity.
    @discardableResult
    func moveBar(_ k: Int, toVideo v: Double) -> Bool {
        guard isPerBarSync,
              let out = SyncEditing.retime(videos: barVideoTimes, scoreStarts: playedBars.map(\.startSecs), index: k,
                                           to: v, keyframes: syncKeyframes)
        else { return false }
        syncKeyframes.insert(k)
        applySync(.points(points: out.enumerated().map { SyncPoint(playedBar: UInt32($0.offset), videoSecs: $0.element) }))
        return true
    }

    /// Switch between offset mode and per-bar points, keeping the current mapping as closely as
    /// the target mode allows.
    func setPerBarSync(_ on: Bool) {
        guard on != isPerBarSync, let session else { return }
        syncKeyframes = []
        if on {
            applySync(.points(points: session.syncPoints()))
        } else {
            // Straight line through the first and last bar starts.
            let starts = playedBars.map(\.startSecs)
            guard let v0 = barVideoTimes.first, let v1 = barVideoTimes.last, let s1 = starts.last, s1 > 0 else {
                applySync(.offset(offsetSecs: barVideoTimes.first ?? 0, stretch: 1))
                return
            }
            let s0 = starts[0]
            let stretch = min(max((v1 - v0) / (s1 - s0), SyncNudgeBar.stretchRange.lowerBound),
                              SyncNudgeBar.stretchRange.upperBound)
            applySync(.offset(offsetSecs: v0 - s0 * stretch, stretch: stretch))
        }
    }

    func importFileSync() {
        guard let spec = session?.fileSync() else { return }
        syncKeyframes = []
        applySync(spec)
    }

    func importBarPoints(_ points: [Double]) {
        syncKeyframes = []
        applySync(barPointsSync(points: points))
    }

    func resetSync() {
        syncKeyframes = []
        applySync(.offset(offsetSecs: 0, stretch: 1))
    }

    /// Bar starts whose video time falls in `range`, for the sync ruler.
    func rulerMarks(in range: ClosedRange<Double>) -> [SyncRulerMark] {
        guard !barVideoTimes.isEmpty, barVideoTimes.count == playedBars.count else { return [] }
        let first = barVideoTimes.partitionIndex { $0 < range.lowerBound }
        var marks: [SyncRulerMark] = []
        var i = first
        while i < barVideoTimes.count, barVideoTimes[i] <= range.upperBound {
            marks.append(SyncRulerMark(playedBar: i, masterBar: playedBars[i].masterBar, video: barVideoTimes[i],
                                       isKeyframe: syncKeyframes.contains(i)))
            i += 1
        }
        return marks
    }
}

extension Array where Element == Double {
    /// First index whose element is not `belongs` (elements must be partitioned).
    func partitionIndex(where belongs: (Double) -> Bool) -> Int {
        var lo = 0
        var hi = count
        while lo < hi {
            let mid = (lo + hi) / 2
            if belongs(self[mid]) { lo = mid + 1 } else { hi = mid }
        }
        return lo
    }
}
