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

    var currentTime: Double {
        guard isPlaying, let anchor = audio.state.anchor.withLock({ $0 }) else {
            return synth.positionSecs()
        }
        if anchor.countingIn { return anchor.scoreStart }
        let elapsed = max(0, CACurrentMediaTime() - anchor.heardAt)
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
        let heard = currentTime
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
}
