import QuartzCore

/// The time source that drives the tab cursor, in score seconds. Phase 2 uses `InternalClock`;
/// Phase 3 adds a YouTube-backed clock (video time mapped through the session's sync map).
@MainActor
protocol PlaybackClock: AnyObject {
    /// Current position in score seconds. Cheap: called every frame while playing.
    var currentTime: Double { get }
    var isPlaying: Bool { get }
    /// Playback speed multiplier (1 = normal).
    var rate: Double { get set }
    func play()
    func pause()
    func seek(to time: Double)
}

/// A free-running clock on `CACurrentMediaTime`: position = anchor + (now − anchorWallClock) × rate.
/// Re-anchors on every play, pause, seek and rate change so none of them jumps the position.
@MainActor
final class InternalClock: PlaybackClock {
    private var anchorTime: Double = 0
    private var anchorWall: CFTimeInterval = 0
    private(set) var isPlaying = false

    var rate: Double = 1 {
        didSet {
            guard rate != oldValue else { return }
            reanchor(at: time(withRate: oldValue))
        }
    }

    var currentTime: Double { time(withRate: rate) }

    func play() {
        guard !isPlaying else { return }
        reanchor(at: anchorTime)
        isPlaying = true
    }

    func pause() {
        guard isPlaying else { return }
        reanchor(at: currentTime)
        isPlaying = false
    }

    func seek(to time: Double) {
        reanchor(at: max(0, time))
    }

    private func time(withRate rate: Double) -> Double {
        guard isPlaying else { return anchorTime }
        return anchorTime + (CACurrentMediaTime() - anchorWall) * rate
    }

    private func reanchor(at time: Double) {
        anchorTime = time
        anchorWall = CACurrentMediaTime()
    }
}
