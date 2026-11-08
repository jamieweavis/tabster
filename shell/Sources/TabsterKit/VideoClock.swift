import Foundation

/// Turns the YouTube page's sparse `{t, perf}` samples (every ~100 ms) into a smooth video time
/// that can be read every display frame. Pure logic: callers pass host times (seconds on the
/// `CACurrentMediaTime()` axis) for both events and frame reads.
///
/// The model (see PLAN.md §0 "Smooth cursor" and the spike in `spikes/youtube/`):
/// - Between samples, extrapolate from the last anchor at the confirmed rate (the page's `rate`
///   event, never the rate we asked for).
/// - Steady playback: drift beyond `deadband` (40 ms) is corrected quickly; smaller drift is
///   trimmed so slowly (a few % of rate) that it can't be seen. Corrections are *slewed*, never
///   stepped backwards: the displayed time keeps moving forward, just slower, while it catches up.
/// - For `settleDuration` after a seek, play or rate change every sample corrects the clock, and
///   forward corrections snap.
/// - For `rateHazardDuration` after a rate change YouTube's reported time first runs ahead, then
///   jumps back 120-300 ms (the media stalls while the pipeline switches rate). In that window
///   forward drift is ignored (prefer the extrapolation) and backward drift is slewed.
/// - Frozen while paused, buffering, ended or unstarted.
/// - Only an explicit `seek`, a pause/end, or a gross discontinuity (> `discontinuity`, e.g. a
///   seek from YouTube's own controls) moves the displayed time backwards in a step.
public final class VideoClock {
    public struct Tuning: Sendable {
        /// Drift (s) below which corrections are only trimmed slowly.
        public var deadband = 0.040
        /// After a seek, play or rate change, correct on every sample for this long (s).
        public var settleDuration = 1.0
        /// After a rate change, distrust forward drift for this long (s).
        public var rateHazardDuration = 0.8
        /// Drift (s) beyond which the clock jumps straight to the sample (someone seeked).
        public var discontinuity = 1.0
        /// Forward drift (s) beyond which a correction snaps instead of slewing.
        public var forwardSnap = 0.25
        /// Slew speeds as a fraction of the playback rate: how fast a residual is absorbed.
        /// Backward slew must stay below 1 so the displayed time never runs backwards.
        public var fastBackwardSlew = 0.75
        public var fastForwardSlew = 1.0
        public var trimSlew = 0.05
        /// Fraction of a sub-deadband drift taken from each sample (a first-order low-pass on
        /// sample noise; the clock still converges because every sample nudges it).
        public var trimGain = 0.3
        /// How long (s) after a seek to discard samples that are far from its target.
        public var seekGrace = 3.0
        /// Drift (s) below which a sample changes nothing.
        public var ignoreBelow = 0.0005

        public init() {}
    }

    public let tuning: Tuning
    public private(set) var state: PlaybackState = .unstarted
    /// The confirmed playback rate.
    public private(set) var rate: Double = 1
    public var isRunning: Bool { state == .playing }

    // Model while running: displayed(h) = anchorTime + (h - anchorHost) * rate + residual(h),
    // where the residual shrinks towards 0 at `slew` seconds per second.
    private var anchorTime = 0.0
    private var anchorHost = 0.0
    private var residual = 0.0
    private var slew = 0.0
    /// The displayed time while not running.
    private var frozen = 0.0
    private var settleUntil = -Double.infinity
    private var hazardUntil = -Double.infinity
    /// Host time of the last event that re-anchored the clock; older samples are stale.
    private var lastEventHost = -Double.infinity
    /// The next resume should jump straight to the reported time (after an explicit seek).
    private var snapOnResume = false
    /// After an explicit seek (or load), samples far from the target are from before the page
    /// processed it, until one near the target arrives or this deadline passes.
    private var seekTarget: Double?
    private var seekDeadline = -Double.infinity

    public init(tuning: Tuning = Tuning()) {
        self.tuning = tuning
    }

    // MARK: Reading

    /// The latest host time a frame was read for. Frames are usually read for their (future)
    /// display time, so a sample can arrive stamped earlier than a frame already shown; it is
    /// applied as of this time so the correction can't step back from that frame.
    private var lastRead = -Double.infinity

    /// The video time to display at `hostTime`. Cheap; call it every frame.
    public func time(at hostTime: Double) -> Double {
        if hostTime > lastRead { lastRead = hostTime }
        return displayed(at: hostTime)
    }

    private func displayed(at hostTime: Double) -> Double {
        guard isRunning else { return frozen }
        let dt = max(0, hostTime - anchorHost)
        let decay = min(abs(residual), slew * dt)
        return anchorTime + dt * rate + (residual > 0 ? residual - decay : residual + decay)
    }

    // MARK: Events

    /// Forget everything, e.g. when a new video loads. The clock is frozen at `time`.
    public func reset(to time: Double = 0, at hostTime: Double = -.infinity) {
        state = .unstarted
        rate = 1
        frozen = time
        residual = 0
        slew = 0
        settleUntil = -.infinity
        hazardUntil = -.infinity
        lastEventHost = hostTime
        snapOnResume = true
        expectSeek(to: time, at: hostTime)
    }

    /// The player's state changed. `time` is the reported video time at the transition, if known.
    public func setState(_ new: PlaybackState, time reported: Double?, at h: Double) {
        let t = reported.flatMap { predatesSeek($0, at: h) ? nil : $0 }
        let wasRunning = isRunning
        if new == .playing {
            if !wasRunning {
                let target = t ?? frozen
                // Resume: a forward step to the reported time is fine; a backward one (we froze
                // ahead of the video) is slewed unless this follows an explicit seek.
                let back = frozen - target
                let snap = snapOnResume || back <= 0 || back > tuning.discontinuity
                state = .playing
                anchor(target, at: h, residual: snap ? 0 : back, slew: tuning.fastBackwardSlew)
                settleUntil = h + tuning.settleDuration
                snapOnResume = false
                lastEventHost = h
            }
            return
        }
        let shown = displayed(at: h)
        state = new
        switch new {
        case .paused, .ended, .unstarted:
            // Show exactly where the video stopped.
            frozen = t ?? shown
        case .buffering:
            // Stall: hold what is on screen unless the video is somewhere else entirely.
            frozen = wasRunning ? shown : frozen
            if let t, abs(t - frozen) > tuning.discontinuity { frozen = t }
        case .playing:
            break
        }
        lastEventHost = h
    }

    /// YouTube confirmed a new playback rate (its `onPlaybackRateChange`).
    public func rateChanged(to newRate: Double, at h: Double) {
        guard newRate > 0, newRate.isFinite else { return }
        if isRunning {
            // Change rate as of the last frame shown, so frames read since `h` stay valid.
            let at = continuityHost(h)
            let shown = displayed(at: at)
            rate = newRate
            anchor(shown, at: at, residual: 0, slew: 0)
            hazardUntil = h + tuning.rateHazardDuration
            settleUntil = h + tuning.rateHazardDuration + tuning.settleDuration / 2
            lastEventHost = h
        } else {
            rate = newRate
        }
    }

    /// An explicit seek: the displayed time jumps to `t` straight away.
    public func seek(to t: Double, at h: Double) {
        if isRunning {
            anchor(t, at: h, residual: 0, slew: 0)
            settleUntil = h + tuning.settleDuration
            hazardUntil = -.infinity
        } else {
            frozen = t
        }
        snapOnResume = true
        lastEventHost = h
        expectSeek(to: t, at: h)
    }

    /// A time sample from the page: video time `t`, taken at host time `h`.
    public func ingest(time sampleTime: Double, at sampleHost: Double) {
        guard sampleTime.isFinite, sampleHost >= lastEventHost,
              !predatesSeek(sampleTime, at: sampleHost)
        else { return }
        guard isRunning else {
            switch state {
            case .paused, .ended, .unstarted: frozen = sampleTime
            case .buffering: if abs(sampleTime - frozen) > tuning.discontinuity { frozen = sampleTime }
            case .playing: break
            }
            return
        }
        // Apply the sample as of the last frame already shown (projected at the current rate).
        let h = continuityHost(sampleHost)
        let t = sampleTime + (h - sampleHost) * rate
        let shown = displayed(at: h)
        let drift = t - shown
        let magnitude = abs(drift)
        if magnitude > tuning.discontinuity {
            anchor(t, at: h, residual: 0, slew: 0)
            settleUntil = h + tuning.settleDuration
            hazardUntil = -.infinity
            return
        }
        if magnitude < tuning.ignoreBelow { return }
        if h < hazardUntil {
            // Overshoot after a rate change: ignore the reported time running ahead, follow it
            // (slewed) when it falls back.
            if drift < 0 { anchor(t, at: h, residual: -drift, slew: tuning.fastBackwardSlew) }
            return
        }
        let settling = h < settleUntil
        if drift > 0, drift > tuning.forwardSnap || (settling && drift > tuning.deadband) {
            anchor(t, at: h, residual: 0, slew: 0)
        } else if settling || magnitude > tuning.deadband {
            anchor(t, at: h, residual: -drift,
                   slew: drift < 0 ? tuning.fastBackwardSlew : tuning.fastForwardSlew)
        } else {
            // Low-pass: move only part of the way, so one noisy sample barely registers.
            let target = shown + drift * tuning.trimGain
            anchor(target, at: h, residual: shown - target, slew: tuning.trimSlew)
        }
    }

    private func expectSeek(to t: Double, at h: Double) {
        seekTarget = t
        seekDeadline = h + tuning.seekGrace
    }

    /// Whether video time `t`, reported at `h`, is from before the pending seek took effect.
    private func predatesSeek(_ t: Double, at h: Double) -> Bool {
        guard let target = seekTarget else { return false }
        if h < seekDeadline, abs(t - target) > tuning.discontinuity { return true }
        seekTarget = nil
        return false
    }

    /// `h`, or the latest frame time read if that is later (by at most 100 ms).
    private func continuityHost(_ h: Double) -> Double {
        min(max(h, lastRead), h + 0.1)
    }

    private func anchor(_ t: Double, at h: Double, residual r: Double, slew speed: Double) {
        anchorTime = t
        anchorHost = h
        residual = r
        slew = speed * rate
    }
}

/// Maps the page's `performance.now()` (ms) onto host time. Message delivery adds a variable
/// delay (main-thread load), so the offset is the smallest `received - perf` seen: the sample
/// that arrived fastest best reflects when samples were taken.
public struct HostClockMapper: Sendable {
    private var offset: Double?
    /// A larger apparent offset than this means the page's clock restarted (page reload).
    public var resetThreshold = 0.5

    public init() {}

    public mutating func reset() { offset = nil }

    /// Host time at which a sample stamped `perfMs` was taken, given it was received at `received`.
    public mutating func hostTime(perfMs: Double, receivedAt received: Double) -> Double {
        let estimate = received - perfMs / 1000
        if let current = offset, estimate >= current, estimate - current <= resetThreshold {
            return perfMs / 1000 + current
        }
        offset = estimate
        return received
    }
}
