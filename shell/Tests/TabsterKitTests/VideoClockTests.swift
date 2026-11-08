import Foundation
import Testing

@testable import TabsterKit

// MARK: Replays of recorded YouTube sessions

/// One event from a recorded session (`spikes/youtube/results/*.jsonl` or the app's debug
/// harness): time samples, state changes, confirmed rate changes and explicit seeks.
private enum Event {
    case sample(t: Double)
    case state(PlaybackState, t: Double?)
    case rate(Double)
    case seek(Double)
}

private struct Recording {
    /// `arrival`: when the shell received it; `host`: when it was taken (they differ for the
    /// harness's samples, whose `ts` is the page's sample time mapped to host time).
    var events: [(arrival: Double, host: Double, event: Event)] = []

    init(resource name: String) throws {
        let url = try #require(Bundle.module.url(forResource: name, withExtension: "jsonl",
                                                  subdirectory: "Resources"))
        for line in try String(contentsOf: url, encoding: .utf8).split(separator: "\n") {
            guard let obj = try JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any],
                  let type = obj["type"] as? String, let ts = obj["ts"] as? Double
            else { continue }
            let event: Event
            switch type {
            case "currentTime", "sample":
                guard let t = obj["t"] as? Double else { continue }
                event = .sample(t: t)
            case "stateChange":
                guard let code = obj["state"] as? Int else { continue }
                event = .state(PlaybackState(youTubeCode: code), t: obj["t"] as? Double)
            case "rate":
                guard let r = obj["rate"] as? Double else { continue }
                event = .rate(r)
            case "seek":
                guard let to = obj["to"] as? Double else { continue }
                event = .seek(to)
            case "cmd":
                guard let cmd = obj["cmd"] as? String, cmd.hasPrefix("seekTo("),
                      let to = Double(cmd.dropFirst(7).dropLast())
                else { continue }
                event = .seek(to)
            default:
                continue
            }
            events.append((obj["recv"] as? Double ?? ts, ts, event))
        }
        events.sort { $0.arrival < $1.arrival }
    }

    struct Result {
        var trace: ClockTrace
        var samples: [(host: Double, t: Double)]  // while playing
        var boundaries: [Double]                  // state, rate and seek events
    }

    /// Feeds the events through a clock as they arrive, reading it at 60 Hz in between. Like a
    /// display link, each frame is read for its display time, `lead` after the current time.
    func replay(_ clock: VideoClock = VideoClock(), lead: Double = 1.0 / 60) -> Result {
        var trace = ClockTrace()
        var samples: [(Double, Double)] = []
        var boundaries: [Double] = []
        var i = 0
        guard let first = events.first?.arrival, let last = events.last?.arrival else {
            return Result(trace: trace, samples: [], boundaries: [])
        }
        var frame = first
        while frame <= last {
            while i < events.count, events[i].arrival <= frame {
                let (_, h, e) = events[i]
                switch e {
                case .sample(let t):
                    if clock.isRunning { samples.append((h, t)) }
                    clock.ingest(time: t, at: h)
                case .state(let s, let t):
                    if s != clock.state { trace.markDiscontinuity(); boundaries.append(h) }
                    clock.setState(s, time: t, at: h)
                case .rate(let r):
                    boundaries.append(h)
                    clock.rateChanged(to: r, at: h)
                case .seek(let to):
                    trace.markDiscontinuity()
                    boundaries.append(h)
                    clock.seek(to: to, at: h)
                }
                i += 1
            }
            let shown = frame + lead
            trace.record(host: shown, time: clock.time(at: shown), running: clock.isRunning, rate: clock.rate)
            frame += 1.0 / 60
        }
        return Result(trace: trace, samples: samples, boundaries: boundaries)
    }
}

/// Worst error (s) of the displayed time against a linear fit of the samples, over the steady part
/// of every uninterrupted playing segment (`settle` s after its start).
private func steadyStateError(_ r: Recording.Result, settle: Double = 1.5) -> (error: Double, frames: Int) {
    let edges = [-Double.infinity] + r.boundaries + [Double.infinity]
    var worst = 0.0, frames = 0
    for (start, end) in zip(edges, edges.dropFirst()) {
        let points = r.samples.filter { $0.host >= start + 1.0 && $0.host < end }.map { (x: $0.host, y: $0.t) }
        guard points.count >= 4, let fit = LinearFit(points) else { continue }
        let lo = max(start + settle, points.first!.x), hi = min(end, points.last!.x)
        for f in r.trace.frames where f.running && f.host >= lo && f.host < hi {
            worst = max(worst, abs(f.time - fit(f.host)))
            frames += 1
        }
    }
    return (worst, frames)
}

@Suite("VideoClock replays")
struct VideoClockReplayTests {
    /// The spike's runs poll every ~250 ms (`ts` = receive time). The harness runs push a sample
    /// every 100 ms; in the throttled one WebKit slowed the page's timer to 1 Hz (window hidden)
    /// and a pre-seek sample arrived after the seek.
    static let dense = ["target-html-baseurl", "localhost", "localhost-interval100", "harness-cJRFMfztgBg"]
    static let recordings = dense + ["harness-cJRFMfztgBg-throttled"]

    @Test(arguments: dense)
    func steadyStateErrorIsSmall(name: String) throws {
        let result = try Recording(resource: name).replay()
        let (error, frames) = steadyStateError(result)
        #expect(frames > 300, "too little steady playback in \(name)")
        #expect(error < 0.005, "steady-state error \(error * 1000) ms in \(name)")
    }

    @Test(arguments: recordings)
    func neverStepsBackwardsWhilePlaying(name: String) throws {
        let result = try Recording(resource: name).replay()
        let back = result.trace.maxBackwardStep()
        #expect(back < 0.003, "stepped back \(back * 1000) ms in \(name)")
    }

    @Test(arguments: recordings)
    func steadyFramesAreSmooth(name: String) throws {
        let result = try Recording(resource: name).replay()
        // Away from events, a frame's step may differ from Δhost × rate only by trim slewing.
        let edges = result.boundaries
        let jitter = result.trace.maxFrameJitter { f in
            !edges.contains { f.host >= $0 && f.host < $0 + 1.5 }
        }
        #expect(jitter < 0.002, "frame jitter \(jitter * 1000) ms in \(name)")
    }
}

// MARK: Synthetic behaviour

@Suite("VideoClock")
struct VideoClockTests {
    /// Starts playing at video time `t0`, host 0, and lets the settle window pass with samples.
    func playing(from t0: Double = 10, rate: Double = 1) -> VideoClock {
        let c = VideoClock()
        c.rateChanged(to: rate, at: 0)
        c.setState(.playing, time: t0, at: 0)
        var h = 0.1
        while h < 2 { c.ingest(time: t0 + h * rate, at: h); h += 0.1 }
        return c
    }

    @Test func extrapolatesBetweenSamples() {
        let c = playing()
        #expect(abs(c.time(at: 2.05) - 12.05) < 1e-9)
        #expect(c.isRunning)
    }

    @Test func extrapolatesAtConfirmedRate() {
        let c = playing(from: 0, rate: 0.75)
        #expect(abs(c.time(at: 2.0) - 1.5) < 1e-9)
    }

    @Test func freezesWhilePausedAndBuffering() {
        let c = playing()
        c.setState(.paused, time: 12.3, at: 2.3)
        #expect(c.time(at: 2.4) == 12.3)
        #expect(c.time(at: 9.0) == 12.3)
        #expect(!c.isRunning)

        let d = playing()
        d.setState(.buffering, time: 12.29, at: 2.3)
        #expect(abs(d.time(at: 5) - 12.3) < 1e-9, "buffering holds what was on screen")
    }

    @Test func smallDriftIsTrimmedWithoutJumping() {
        let c = playing()
        let before = c.time(at: 2.1)
        c.ingest(time: 12.12, at: 2.1)  // 20 ms ahead: inside the deadband
        #expect(abs(c.time(at: 2.1) - before) < 1e-9, "no step")
        #expect(abs(c.time(at: 3.0) - 13.006) < 1e-6, "took 30% of the drift")
    }

    @Test func largeBackwardDriftIsSlewedNotStepped() {
        let c = playing()
        c.ingest(time: 12.0, at: 2.1)  // 100 ms behind
        var last = c.time(at: 2.1)
        var h = 2.1
        while h < 3 {
            h += 1.0 / 60
            let now = c.time(at: h)
            #expect(now > last, "keeps moving forward")
            last = now
        }
        #expect(abs(c.time(at: 3) - 12.9) < 1e-6)
    }

    @Test func largeForwardDriftSnaps() {
        let c = playing()
        c.ingest(time: 12.5, at: 2.1)
        #expect(abs(c.time(at: 2.1) - 12.5) < 1e-9)
    }

    @Test func explicitSeekJumpsImmediately() {
        let c = playing()
        c.seek(to: 60, at: 2.2)
        #expect(abs(c.time(at: 2.2) - 60) < 1e-9)
        c.setState(.buffering, time: 60, at: 2.21)
        #expect(abs(c.time(at: 2.5) - 60.01) < 1e-9)
        c.setState(.playing, time: 60.0, at: 2.7)  // resumes slightly behind the frozen time: snaps
        #expect(abs(c.time(at: 2.8) - 60.1) < 1e-9)
    }

    @Test func ignoresRateChangeOvershootThenFollowsTheJumpBack() {
        let c = playing()
        c.rateChanged(to: 1.5, at: 2.0)
        // The reported time runs ahead...
        c.ingest(time: 12.25, at: 2.1)
        #expect(abs(c.time(at: 2.1) - 12.15) < 1e-9, "overshoot ignored")
        // ...then falls back 300 ms: the clock slows down to meet it but never steps back.
        var last = c.time(at: 2.2)
        c.ingest(time: 12.0, at: 2.2)
        var h = 2.2
        while h < 4 {
            let s = 12.0 + (h - 2.2) * 1.5
            if Int((h * 1000).rounded()) % 100 == 0 { c.ingest(time: s, at: h) }
            let now = c.time(at: h)
            #expect(now >= last - 1e-9)
            last = now
            h += 0.005
        }
        #expect(abs(c.time(at: 4) - (12.0 + 1.8 * 1.5)) < 0.002)
    }

    @Test func grossDiscontinuityJumps() {
        let c = playing()
        c.ingest(time: 100, at: 2.1)  // seeked from YouTube's own controls
        #expect(abs(c.time(at: 2.1) - 100) < 1e-9)
    }

    @Test func staleSamplesAreIgnored() {
        let c = playing()
        c.seek(to: 60, at: 2.2)
        c.ingest(time: 12.15, at: 2.15)
        #expect(abs(c.time(at: 2.2) - 60) < 1e-9)
    }

    /// Recorded live: a sample taken after Swift's seek but before the page processed it.
    @Test func samplesFromBeforeTheSeekLandedAreIgnored() {
        let c = playing()
        c.seek(to: 60, at: 2.2)
        c.ingest(time: 12.21, at: 2.21)
        #expect(abs(c.time(at: 2.25) - 60.05) < 1e-9)
        c.setState(.buffering, time: 12.22, at: 2.22)
        #expect(abs(c.time(at: 2.5) - 60.02) < 1e-9)
        c.setState(.playing, time: 60.0, at: 2.7)
        #expect(abs(c.time(at: 2.8) - 60.1) < 1e-9)
        // Once the seek has landed, a big jump is a real discontinuity again.
        c.ingest(time: 30, at: 2.9)
        #expect(abs(c.time(at: 2.9) - 30) < 1e-9)
    }

    @Test func pauseShowsTheReportedPosition() {
        let c = playing()
        c.setState(.paused, time: 12.25, at: 2.3)
        c.ingest(time: 12.26, at: 2.31)
        #expect(c.time(at: 3) == 12.26)
        c.setState(.playing, time: 12.26, at: 4)
        #expect(abs(c.time(at: 4.5) - 12.76) < 1e-9)
    }
}

@Suite("HostClockMapper")
struct HostClockMapperTests {
    @Test func usesTheFastestDelivery() {
        var m = HostClockMapper()
        #expect(m.hostTime(perfMs: 1000, receivedAt: 101.005) == 101.005)
        #expect(abs(m.hostTime(perfMs: 1100, receivedAt: 101.101) - 101.101) < 1e-9)  // faster: new offset
        #expect(abs(m.hostTime(perfMs: 1200, receivedAt: 101.220) - 101.201) < 1e-9)  // late delivery
    }

    @Test func resetsWhenThePageReloads() {
        var m = HostClockMapper()
        _ = m.hostTime(perfMs: 50_000, receivedAt: 150)
        #expect(m.hostTime(perfMs: 10, receivedAt: 160) == 160)
    }
}

@Suite("YouTube types")
struct YouTubeTypesTests {
    @Test func failureCodes() {
        #expect(YouTubePlayerFailure(code: 150) == .embeddingDisabled(code: 150))
        #expect(YouTubePlayerFailure(code: 101).isPermanent)
        #expect(YouTubePlayerFailure(code: 100) == .notFound)
        #expect(YouTubePlayerFailure(code: 153) == .missingReferrer)
        #expect(YouTubePlayerFailure(code: 7) == .other(7))
        #expect(!YouTubePlayerFailure(code: 5).isPermanent)
    }

    @Test func states() {
        #expect(PlaybackState(youTubeCode: 1) == .playing)
        #expect(PlaybackState(youTubeCode: 5) == .unstarted)
        #expect(PlaybackState(youTubeCode: 3) == .buffering)
    }

    @Test func watchURL() {
        #expect(YouTubeLinks.watchURL(videoID: "cJRFMfztgBg", at: 61.7).absoluteString
            == "https://www.youtube.com/watch?v=cJRFMfztgBg&t=61s")
        #expect(YouTubeLinks.watchURL(videoID: "cJRFMfztgBg").absoluteString
            == "https://www.youtube.com/watch?v=cJRFMfztgBg")
    }
}
