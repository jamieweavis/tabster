import QuartzCore
import TabsterKit

/// The time source that drives the tab cursor, in the clock's own *media* seconds: score seconds
/// for `InternalClock`, video seconds for `VideoPlaybackClock`. `TabViewerModel` converts media
/// time to score time (through the session's sync map when the clock is a video).
@MainActor
protocol PlaybackClock: AnyObject {
    /// Position at host time `hostTime` (`CACurrentMediaTime()` axis). Cheap: called every
    /// display frame with the frame's target timestamp.
    func time(at hostTime: CFTimeInterval) -> Double
    var isPlaying: Bool { get }
    /// Playback speed multiplier (1 = normal). For a video clock, the rate the player confirmed.
    var rate: Double { get set }
    func play()
    func pause()
    func seek(to time: Double)
}

extension PlaybackClock {
    /// Position now.
    var currentTime: Double { time(at: CACurrentMediaTime()) }
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
            let now = CACurrentMediaTime()
            reanchor(at: time(at: now, rate: oldValue), wall: now)
        }
    }

    func time(at hostTime: CFTimeInterval) -> Double { time(at: hostTime, rate: rate) }

    func play() {
        guard !isPlaying else { return }
        reanchor(at: anchorTime, wall: CACurrentMediaTime())
        isPlaying = true
    }

    func pause() {
        guard isPlaying else { return }
        reanchor(at: currentTime, wall: CACurrentMediaTime())
        isPlaying = false
    }

    func seek(to time: Double) {
        reanchor(at: time, wall: CACurrentMediaTime())
    }

    private func time(at hostTime: CFTimeInterval, rate: Double) -> Double {
        guard isPlaying else { return anchorTime }
        // A display link's target timestamp is slightly in the future; never extrapolate behind
        // the anchor (right after a seek).
        return anchorTime + max(0, hostTime - anchorWall) * rate
    }

    private func reanchor(at time: Double, wall: CFTimeInterval) {
        anchorTime = time
        anchorWall = wall
    }
}

/// The YouTube player as the playback clock, in video seconds. Reads the controller's smooth
/// `VideoClock` per frame (never WebKit). `isPlaying` is the transport's intent (play was asked
/// for), while the video itself may still be buffering; `rate` reads back the rate YouTube
/// confirmed, and setting it requests the nearest rate YouTube offers.
@MainActor
final class VideoPlaybackClock: PlaybackClock {
    let controller: YouTubePlayerController
    private(set) var isPlaying = false

    init(controller: YouTubePlayerController) {
        self.controller = controller
    }

    func time(at hostTime: CFTimeInterval) -> Double { controller.time(at: hostTime) }

    var rate: Double {
        get { controller.rate }
        set { controller.setRate(Self.snap(newValue, to: controller.availableRates)) }
    }

    /// YouTube's rates (0.25–2 in 0.25 steps) unless the player reported others.
    static let defaultRates: [Double] = stride(from: 0.25, through: 2, by: 0.25).map { $0 }

    static func snap(_ rate: Double, to rates: [Double]) -> Double {
        SyncEditing.snapRate(rate, to: rates.isEmpty ? defaultRates : rates)
    }

    func play() {
        isPlaying = true
        controller.play()
    }

    func pause() {
        isPlaying = false
        controller.pause()
    }

    func seek(to time: Double) { controller.seek(to: max(0, time)) }

    /// The player started or stopped on its own (its own controls, end of video): adopt it.
    func playerStateChanged(playing: Bool) { isPlaying = playing }
}
