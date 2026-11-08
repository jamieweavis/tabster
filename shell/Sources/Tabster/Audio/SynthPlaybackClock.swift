import AVFoundation
import QuartzCore

/// A `PlaybackClock` driven by the tab synth: the score position is what's being heard, i.e.
/// the synth's own score clock, interpolated between audio buffers and delayed by the output
/// latency. Use it when there's no video (the synth audio is then the time source).
@MainActor
final class SynthPlaybackClock: PlaybackClock {
    let audio: SynthAudioEngine
    var synth: TabSynth { audio.synth }
    private(set) var isPlaying = false
    /// Bars of metronome count-in before each `play()` (0 = none).
    var countInBars: UInt32 = 0
    private var idleTask: Task<Void, Never>?
    /// Host time of the last hard resync while following another clock.
    private var lastResync: CFTimeInterval = 0

    init(synth: TabSynth) {
        audio = SynthAudioEngine(synth: synth)
    }

    /// The bundled SoundFont, if the app was built with one (`just soundfont`).
    nonisolated static var soundFontURL: URL? {
        Bundle.main.url(forResource: "SoundFont", withExtension: "sf2")
    }

    /// Create the synth for a session off the main thread (the first call loads the ~32 MB
    /// SoundFont). `nil` when there's no SoundFont: tab audio is unavailable.
    static func make(session: TabSession) async -> SynthPlaybackClock? {
        guard let url = soundFontURL else {
            SynthAudioEngine.log.info("No SoundFont bundled; tab audio unavailable")
            return nil
        }
        let result = await Task.detached { () -> Result<TabSynth, Error> in
            Result { try session.synth(soundfontPath: url.path) }
        }.value
        switch result {
        case let .success(synth): return SynthPlaybackClock(synth: synth)
        case let .failure(error):
            SynthAudioEngine.log.error("Couldn't load the SoundFont: \(error.userMessage, privacy: .public)")
            return nil
        }
    }

    var rate: Double = 1 {
        didSet {
            guard rate != oldValue else { return }
            synth.setSpeed(speed: rate)
            audio.state.rate.withLock { $0 = rate }
        }
    }

    func time(at hostTime: CFTimeInterval) -> Double {
        guard isPlaying, let anchor = audio.state.anchor.withLock({ $0 }) else {
            return synth.positionSecs()
        }
        if anchor.countingIn { return anchor.scoreStart }
        let elapsed = max(0, hostTime - anchor.heardAt)
        let rate = audio.state.rate.withLock { $0 }
        // Extrapolate at most one more buffer past what's been rendered, so a late callback
        // doesn't make the cursor run ahead of the audio.
        let slack = (anchor.scoreEnd - anchor.scoreStart) + 0.02 * rate
        return min(anchor.scoreStart + elapsed * rate, anchor.scoreEnd + slack)
    }

    func play() {
        guard !isPlaying else { return }
        idleTask?.cancel()
        do {
            try audio.start()
        } catch {
            SynthAudioEngine.log.error("Couldn't start audio: \(error.localizedDescription, privacy: .public)")
            return
        }
        audio.state.anchor.withLock { $0 = nil }
        if countInBars > 0 { synth.countIn(bars: countInBars) }
        synth.setPlaying(playing: true)
        isPlaying = true
    }

    func pause() {
        guard isPlaying else { return }
        // Freeze where the listener is, not where the renderer got to.
        let heard = time(at: CACurrentMediaTime())
        isPlaying = false
        synth.setPlaying(playing: false)
        synth.seek(scoreSecs: heard)
        audio.state.anchor.withLock { $0 = nil }
        // Let the release tails finish, then stop the device so idle costs nothing.
        idleTask?.cancel()
        idleTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(1))
            guard let self, !Task.isCancelled, !self.isPlaying else { return }
            self.audio.pause()
        }
    }

    func seek(to time: Double) {
        synth.seek(scoreSecs: max(0, time))
        audio.state.anchor.withLock { $0 = nil }
    }

    // MARK: - Following another clock

    /// Drive the synth as a follower of another clock (a video): keep what's heard at
    /// `target` (score seconds heard at `hostTime`) advancing at `rate` score seconds per
    /// second. `nil` stops it. Called every display frame; small drift is pulled in by nudging
    /// the speed, large drift (a seek, a stall) by seeking. No count-in.
    func follow(target: Double?, rate: Double, at hostTime: CFTimeInterval) {
        guard let target, target >= 0, target < synth.durationSecs(), rate > 0 else {
            if isPlaying { pause() }
            return
        }
        let lead = audio.state.outputLatency.withLock { $0 } * rate + 0.01
        if !isPlaying {
            idleTask?.cancel()
            do {
                try audio.start()
            } catch {
                SynthAudioEngine.log.error("Couldn't start audio: \(error.localizedDescription, privacy: .public)")
                return
            }
            setFollowSpeed(rate)
            synth.seek(scoreSecs: target + lead)
            audio.state.anchor.withLock { $0 = nil }
            synth.setPlaying(playing: true)
            isPlaying = true
            lastResync = hostTime
            return
        }
        // Until the first buffer after a (re)start or seek, there's nothing reliable to compare.
        guard audio.state.anchor.withLock({ $0 }) != nil else {
            setFollowSpeed(rate)
            return
        }
        let drift = target - time(at: hostTime)
        if abs(drift) > 0.08, hostTime - lastResync > 0.3 {
            setFollowSpeed(rate)
            synth.seek(scoreSecs: target + lead)
            audio.state.anchor.withLock { $0 = nil }
            lastResync = hostTime
        } else {
            // Close the gap over about two seconds, at most ±5 %.
            let nudge = min(max(drift / 2, -0.05 * rate), 0.05 * rate)
            setFollowSpeed(rate + (abs(drift) > 0.005 ? nudge : 0))
        }
    }

    /// Set the synth's speed without touching `rate` (the internal-clock speed).
    private func setFollowSpeed(_ speed: Double) {
        synth.setSpeed(speed: speed)
        audio.state.rate.withLock { $0 = speed }
    }

    /// Stop following and restore the internal-clock speed.
    func stopFollowing() {
        if isPlaying { pause() }
        synth.setSpeed(speed: rate)
        audio.state.rate.withLock { $0 = rate }
    }
}
