import Foundation

/// High-rate time sample from the page.
struct TimeSample {
    let t: Double          // getCurrentTime(), video seconds
    let perf: Double       // JS performance.now(), ms
    let epoch: Double      // JS performance.timeOrigin + perf, ms since 1970
    let rate: Double
    let state: Int
    let received: Double   // Swift CACurrentMediaTime(), s
    let receivedEpoch: Double  // Swift Date(), ms since 1970
}

/// One Swift → JS → Swift probe (`tabster.snapshot()`), used to map JS perf time to CACurrentMediaTime.
struct Ping {
    let sent: Double, returned: Double, perf: Double
    var rtt: Double { returned - sent }
    /// jsSeconds - caSeconds, assuming the JS clock was read halfway through the round trip.
    var offset: Double { perf / 1000 - (sent + returned) / 2 }
}

func percentile(_ xs: [Double], _ p: Double) -> Double {
    guard !xs.isEmpty else { return .nan }
    let s = xs.sorted()
    let i = min(s.count - 1, max(0, Int((p / 100 * Double(s.count - 1)).rounded())))
    return s[i]
}

func summary(_ xs: [Double], scale: Double = 1, places: Int = 2) -> [String: Any] {
    guard !xs.isEmpty else { return ["n": 0] }
    let v = xs.map { $0 * scale }
    return [
        "n": v.count,
        "min": v.min()!.rounded(places: places),
        "p50": percentile(v, 50).rounded(places: places),
        "p90": percentile(v, 90).rounded(places: places),
        "p99": percentile(v, 99).rounded(places: places),
        "max": v.max()!.rounded(places: places),
        "mean": (v.reduce(0, +) / Double(v.count)).rounded(places: places),
    ]
}

enum Analysis {
    /// Splits playing samples into continuous segments (no seek, rate change or stall).
    static func segments(_ samples: [TimeSample]) -> [[TimeSample]] {
        var out: [[TimeSample]] = []
        var cur: [TimeSample] = []
        for s in samples.sorted(by: { $0.perf < $1.perf }) {
            guard s.state == 1 else { if cur.count > 1 { out.append(cur) }; cur = []; continue }
            if let p = cur.last {
                let dt = (s.perf - p.perf) / 1000
                let expected = p.t + dt * p.rate
                if s.rate != p.rate || abs(s.t - expected) > 0.5 || dt > 0.5 {
                    if cur.count > 1 { out.append(cur) }
                    cur = []
                }
            }
            cur.append(s)
        }
        if cur.count > 1 { out.append(cur) }
        return out.filter { ($0.last!.perf - $0.first!.perf) > 1000 }
    }

    /// Least squares t = a + b * perfSeconds.
    static func fit(_ seg: [TimeSample]) -> (a: Double, b: Double) {
        let xs = seg.map { $0.perf / 1000 }, ys = seg.map(\.t)
        let n = Double(xs.count)
        let mx = xs.reduce(0, +) / n, my = ys.reduce(0, +) / n
        var sxy = 0.0, sxx = 0.0
        for (x, y) in zip(xs, ys) { sxy += (x - mx) * (y - my); sxx += (x - mx) * (x - mx) }
        let b = sxx > 0 ? sxy / sxx : 1
        return (my - b * mx, b)
    }

    static func report(samples: [TimeSample], pings: [Ping], polls: [Ping]) -> [String: Any] {
        var r: [String: Any] = ["type": "summary", "samples": samples.count]
        let allPings = pings + polls
        r["callAsyncJavaScriptRTT_ms"] = summary(allPings.map(\.rtt), scale: 1000)

        // JS perf -> CA offset from the tightest round trip.
        let best = allPings.min(by: { $0.rtt < $1.rtt })
        if let best {
            r["clockOffsetUncertainty_ms"] = (best.rtt * 500).rounded(places: 3)
            let oneWay = samples.map { $0.received - ($0.perf / 1000 - best.offset) }
            r["jsToSwiftLatency_ms"] = summary(oneWay, scale: 1000)
        }
        r["jsToSwiftLatencyEpoch_ms"] = summary(samples.map { $0.receivedEpoch - $0.epoch })

        let segs = segments(samples)
        r["segments"] = segs.count
        r["all"] = timing(segs)
        // Drop the first second of every segment: getCurrentTime() overshoots then snaps back for
        // ~0.5 s after a rate change (the API extrapolates at the new rate before the media does).
        let steady = segs.map { seg in seg.filter { $0.perf - seg[0].perf >= 1000 } }.filter { $0.count > 30 }
        r["steady"] = timing(steady)
        return r
    }

    static func timing(_ segs: [[TimeSample]]) -> [String: Any] {
        var r: [String: Any] = ["segments": segs.count]
        var changeIntervals: [Double] = [], steps: [Double] = [], unchanged = 0, total = 0
        var residuals: [Double] = [], slopes: [Double] = []
        var sim: [String: [Double]] = [:]
        var jumps: [String: [Double]] = [:]
        for seg in segs {
            var lastChange = seg[0]
            for (p, s) in zip(seg, seg.dropFirst()) {
                total += 1
                if s.t == p.t { unchanged += 1; continue }
                changeIntervals.append(s.perf - lastChange.perf)
                steps.append((s.t - lastChange.t) * 1000)
                lastChange = s
            }
            let (a, b) = fit(seg)
            slopes.append(b / max(seg[0].rate, 0.01))
            for s in seg { residuals.append((s.t - (a + b * s.perf / 1000)) * 1000) }

            // Simulated transport strategies: poll every P ms, evaluate cursor at every rAF frame
            // against the fitted "true" video time.
            for pollMs in [100.0, 150.0, 250.0] {
                for strategy in ["hold", "extrapolate", "extrapolate+40ms-deadband"] {
                    var anchorT = seg[0].t, anchorPerf = seg[0].perf, nextPoll = seg[0].perf + pollMs
                    var errs: [Double] = []
                    for s in seg {
                        if s.perf >= nextPoll {
                            nextPoll += pollMs
                            let predicted = anchorT + (s.perf - anchorPerf) / 1000 * s.rate
                            let drift = s.t - predicted
                            switch strategy {
                            case "hold": anchorT = s.t; anchorPerf = s.perf
                            case "extrapolate":
                                jumps["\(strategy)@\(Int(pollMs))", default: []].append(abs(drift) * 1000)
                                anchorT = s.t; anchorPerf = s.perf
                            default:
                                if abs(drift) > 0.040 {
                                    jumps["\(strategy)@\(Int(pollMs))", default: []].append(abs(drift) * 1000)
                                    anchorT = s.t; anchorPerf = s.perf
                                }
                            }
                        }
                        let cursor = strategy == "hold" ? anchorT : anchorT + (s.perf - anchorPerf) / 1000 * s.rate
                        errs.append((cursor - (a + b * s.perf / 1000)) * 1000)
                    }
                    sim["\(strategy)@\(Int(pollMs))", default: []] += errs
                }
            }
        }
        r["timeChangeInterval_ms"] = summary(changeIntervals)
        r["timeStep_ms"] = summary(steps)
        r["framesWithUnchangedTime"] = total > 0 ? Double(unchanged) / Double(total) : 0
        r["fitResidual_ms"] = summary(residuals)
        r["fitSlopeOverRate"] = summary(slopes, places: 4)
        r["cursorErrorVsFit_ms"] = sim.mapValues { errs in summary(errs.map(abs)) }
        r["reanchorJump_ms"] = jumps.mapValues { summary($0) }
        return r
    }
}
