#if DEBUG
    import AppKit
    import QuartzCore
    import TabsterKit
    import SwiftUI

    /// Live check of the YouTube player and `VideoClock` (DEBUG builds only).
    ///
    /// `TABSTER_YT_HARNESS=<videoID> build/Tabster.app/Contents/MacOS/Tabster` opens a player
    /// window, plays the video muted, seeks to 60 s, sets the rate to 0.75 then 1.0, pauses and
    /// resumes, then prints a summary and exits. Every sample, event and display frame is printed
    /// to stdout as JSON lines (the same schema `TabsterKitTests` replays).
    @MainActor
    enum YouTubeDebugHarness {
        private static var runner: Runner?

        static func runIfRequested() {
            guard let id = ProcessInfo.processInfo.environment["TABSTER_YT_HARNESS"], !id.isEmpty else { return }
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    runner = Runner(videoID: id)
                    runner?.start()
                }
            }
        }
    }

    @MainActor
    private final class Runner: NSObject {
        let videoID: String
        let controller: YouTubePlayerController
        let startHost = CACurrentMediaTime()
        var window: NSWindow?
        var link: CADisplayLink?
        var trace = ClockTrace()
        var samples: [(host: Double, t: Double)] = []
        var boundaries: [Double] = []
        var lastState = PlaybackState.unstarted
        var scriptStarted = false

        init(videoID: String) {
            self.videoID = videoID
            let dir = FileManager.default.temporaryDirectory.appending(path: "tabster-yt-harness")
            controller = YouTubePlayerController(core: try? Core(dataDir: dir.path))
        }

        func start() {
            let w = NSWindow(contentRect: NSRect(x: 120, y: 120, width: 800, height: 450),
                             styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
            w.title = "YouTube harness — \(videoID)"
            w.isReleasedWhenClosed = false
            w.contentView = NSHostingView(rootView: YouTubePlayerView(controller: controller).padding(8))
            w.level = .floating  // stay visible: WebKit throttles the page's timers when occluded
            w.makeKeyAndOrderFront(nil)
            w.orderFrontRegardless()
            NSApp.activate()
            window = w

            controller.onSample = { [weak self] t, host in self?.sample(t, host) }
            controller.onEvent = { [weak self] type, body in self?.event(type, body) }
            controller.setMuted(true)
            controller.load(videoID: videoID, autoplay: true)

            let link = w.contentView!.displayLink(target: self, selector: #selector(frame(_:)))
            link.add(to: .main, forMode: .common)
            self.link = link

            Task {
                try? await Task.sleep(for: .seconds(45))
                emit(["type": "harnessError", "msg": "timed out"])
                finish()
            }
        }

        func ts(_ host: Double) -> Double { host - startHost }

        func emit(_ fields: [String: Any]) {
            guard let data = try? JSONSerialization.data(withJSONObject: fields, options: [.sortedKeys]) else { return }
            FileHandle.standardOutput.write(data + Data("\n".utf8))
        }

        func sample(_ t: Double, _ host: Double) {
            if controller.clock.isRunning { samples.append((host, t)) }
            emit(["type": "sample", "t": t, "ts": ts(host), "recv": ts(CACurrentMediaTime())])
        }

        func event(_ type: String, _ body: [String: Any]) {
            let now = CACurrentMediaTime()
            guard type != "time" else { return }
            var b = body.filter { $0.value is String || $0.value is NSNumber || $0.value is [Double] }
            b["ts"] = ts(now)
            emit(b)
            switch type {
            case "stateChange":
                let s = PlaybackState(youTubeCode: body["state"] as? Int ?? -1)
                if s != lastState {
                    trace.markDiscontinuity()
                    boundaries.append(now)
                    lastState = s
                }
                if s == .playing, !scriptStarted {
                    scriptStarted = true
                    Task { await script() }
                }
            case "rate":
                boundaries.append(now)
            case "error":
                emit(["type": "harnessError", "msg": "player error \(body["code"] ?? "?")"])
                Task { try? await Task.sleep(for: .seconds(3)); finish() }  // leave the overlay up briefly
            default:
                break
            }
        }

        @objc func frame(_ link: CADisplayLink) {
            let host = link.targetTimestamp
            let t = controller.time(at: host)
            trace.record(host: host, time: t, running: controller.clock.isRunning, rate: controller.clock.rate)
            emit(["type": "frame", "ts": ts(host), "v": t, "running": controller.clock.isRunning])
        }

        func script() async {
            func wait(_ s: Double) async { try? await Task.sleep(for: .seconds(s)) }
            await wait(3)
            seek(60)
            await wait(3)
            command("setRate(0.75)") { $0.setRate(0.75) }
            await wait(3)
            command("setRate(1)") { $0.setRate(1) }
            await wait(3)
            command("pause") { $0.pause() }
            await wait(1.5)
            command("play") { $0.play() }
            await wait(3)
            finish()
        }

        func seek(_ to: Double) {
            let now = CACurrentMediaTime()
            trace.markDiscontinuity()
            boundaries.append(now)
            emit(["type": "seek", "to": to, "ts": ts(now)])
            controller.seek(to: to)
        }

        func command(_ name: String, _ body: (YouTubePlayerController) -> Void) {
            emit(["type": "cmd", "cmd": name, "ts": ts(CACurrentMediaTime())])
            body(controller)
        }

        func finish() {
            link?.invalidate()
            let edges = [-Double.infinity] + boundaries + [Double.infinity]
            var steadyError = 0.0
            for (lo, hi) in zip(edges, edges.dropFirst()) {
                let points = samples.filter { $0.host >= lo + 1 && $0.host < hi }.map { (x: $0.host, y: $0.t) }
                guard points.count >= 4, let fit = LinearFit(points) else { continue }
                for f in trace.frames where f.running && f.host >= max(lo + 1.5, points[0].x) && f.host < min(hi, points.last!.x) {
                    steadyError = max(steadyError, abs(f.time - fit(f.host)))
                }
            }
            let nearEvent: (ClockTrace.Frame) -> Bool = { f in
                self.boundaries.contains { f.host >= $0 && f.host < $0 + 1.5 }
            }
            emit([
                "type": "summary",
                "frames": trace.frames.count,
                "samples": samples.count,
                "pageMode": controller.pageMode == .inline ? "inline" : "localhost",
                "maxBackwardStep_ms": trace.maxBackwardStep() * 1000,
                "maxFrameJitter_ms": trace.maxFrameJitter() * 1000,
                "maxFrameJitterSteady_ms": trace.maxFrameJitter { !nearEvent($0) } * 1000,
                "steadyStateErrorVsFit_ms": steadyError * 1000,
                "failure": controller.failure.map { "\($0)" } ?? "none",
            ])
            exit(0)
        }
    }
#endif
