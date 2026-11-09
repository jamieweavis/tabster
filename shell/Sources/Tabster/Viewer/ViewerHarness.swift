import AppKit
import SwiftUI

/// DEBUG-only harness for developing the viewer without the library UI.
///
/// `TABSTER_VIEW=<tab file>` imports the file into a temporary data dir and shows it in a
/// `TabViewer`. Optional: `TABSTER_TRACK=<n>`, `TABSTER_PLAY=1`, `TABSTER_SEEK=<secs>`,
/// `TABSTER_LOOP=<first>-<last>` (1-based bars),
/// `TABSTER_APPEARANCE=dark|light`, `TABSTER_SCALE=<zoom>`, `TABSTER_WINDOW=<w>x<h>`, `TABSTER_SELFTEST=1` (exercise the transport and log),
/// `TABSTER_SCROLLTEST=1` (scroll
/// through the whole tab and log draw times), and `TABSTER_VIEW_SNAPSHOT=<png>` (write the window to
/// a PNG after ~2 s, or `TABSTER_SNAPSHOT_AFTER=<secs>`, log perf stats and quit).
///
/// Video: `TABSTER_VIDEO=<YouTube id>` attaches the video (muted unless `TABSTER_VIDEO_MUTE=0`)
/// with `TABSTER_SYNC_OFFSET=<secs>` / `TABSTER_SYNC_STRETCH=<x>`; `TABSTER_SYNC_PANEL=1` opens
/// the sync editor (`TABSTER_SYNC_PERBAR=1` in per-bar mode), and `TABSTER_VIDEO_LOG=<file>` writes every display frame (host, video and score time, played bar)
/// and raw player sample as JSON lines.
///
/// Practice: `TABSTER_COUNTIN=1`,
/// `TABSTER_TAB_AUDIO=0|1` (synth or silent internal clock), `TABSTER_PRACTICED=<secs>` seeds practice
/// history and `TABSTER_SAVED_LOOPS=Name:13-16,…` seeds saved loops. `TABSTER_METRONOME=1` turns
/// the metronome on; `TABSTER_METRONOME_POPOVER=1` / `TABSTER_TAB_AUDIO_POPOVER=1` open the
/// metronome / tab audio options popover.
///
/// Hook (in `TabsterApp`): `ViewerHarness.debugHarnessIfRequested { ContentView() }`.
enum ViewerHarness {
    @MainActor @ViewBuilder
    static func debugHarnessIfRequested<Content: View>(else content: () -> Content) -> some View {
        #if DEBUG
        if let path = ProcessInfo.processInfo.environment["TABSTER_VIEW"] {
            HarnessView(path: path)
        } else {
            content()
        }
        #else
        content()
        #endif
    }
}

#if DEBUG
private struct HarnessView: View {
    let path: String
    @State private var model: TabViewerModel?
    @State private var error: String?

    private var env: [String: String] { ProcessInfo.processInfo.environment }

    var body: some View {
        Group {
            if let model {
                NavigationStack { TabViewer(model: model) }
            } else if let error {
                Text(error).foregroundStyle(.red).frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .frame(minWidth: 600, minHeight: 400)
        .task { await start() }
    }

    private func start() async {
        NSApp.setActivationPolicy(.regular)
        NSApp.activate()
        switch env["TABSTER_APPEARANCE"] {
        case "dark": NSApp.appearance = NSAppearance(named: .darkAqua)
        case "light": NSApp.appearance = NSAppearance(named: .aqua)
        default: break
        }
        if let size = env["TABSTER_WINDOW"]?.split(separator: "x").compactMap({ Double($0) }), size.count == 2,
           let window = NSApp.windows.first {
            window.setContentSize(NSSize(width: size[0], height: size[1]))
            window.center()
        }
        let path = path
        let track = env["TABSTER_TRACK"].flatMap(UInt32.init)
        let seek = env["TABSTER_SEEK"].flatMap(Double.init)
        // 1-based inclusive bar range, e.g. "23-24".
        let loop = env["TABSTER_LOOP"]?.split(separator: "-").compactMap { UInt32($0) }.map { $0 - 1 }
        let videoID = env["TABSTER_VIDEO"]
        let sync = SyncSpec.offset(offsetSecs: env["TABSTER_SYNC_OFFSET"].flatMap(Double.init) ?? 0,
                                   stretch: env["TABSTER_SYNC_STRETCH"].flatMap(Double.init) ?? 1)
        // Volatile: don't touch the user's saved preferences.
        var volatile = UserDefaults.standard.volatileDomain(forName: UserDefaults.argumentDomain)
        if let audio = env["TABSTER_TAB_AUDIO"] { volatile["tabAudio"] = audio == "1" }
        if let audio = env["TABSTER_VIDEO_AUDIO"] { volatile["videoAudio"] = audio == "1" }
        if let countIn = env["TABSTER_COUNTIN"] { volatile["viewer.countIn"] = countIn == "1" }
        volatile[TabViewerModel.scaleKey] = env["TABSTER_SCALE"].flatMap(Float.init) ?? 1
        UserDefaults.standard.setVolatileDomain(volatile, forName: UserDefaults.argumentDomain)
        let practiced = env["TABSTER_PRACTICED"].flatMap(Double.init)
        let savedLoops: [(String, UInt32, UInt32)] = (env["TABSTER_SAVED_LOOPS"] ?? "").split(separator: ",").compactMap {
            let parts = $0.split(separator: ":")
            let bars = parts.last?.split(separator: "-").compactMap { UInt32($0) } ?? []
            guard parts.count == 2, bars.count == 2, bars[0] >= 1, bars[1] >= 1 else { return nil }
            return (String(parts[0]), bars[0] - 1, bars[1] - 1)
        }
        if videoID != nil {
            // WebKit throttles an occluded page's timers (and so the player's samples).
            NSApp.windows.first?.level = .floating
        }
        let result = await Task.detached { () -> Result<(Core, Int64), Error> in
            do {
                let dir = FileManager.default.temporaryDirectory
                    .appendingPathComponent("tabster-harness-\(ProcessInfo.processInfo.processIdentifier)")
                try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
                let core = try Core(dataDir: dir.path)
                let outcome = core.importPaths(paths: [path]).first
                guard let id = outcome?.id else {
                    throw HarnessError(message: outcome?.error ?? "import failed")
                }
                var state = try core.practiceState(tab: id) ?? PracticeState(
                    track: 0, speed: 1, loopStartBar: nil, loopEndBar: nil, lastPositionSecs: 0)
                if let track { state.track = track }
                if let seek { state.lastPositionSecs = seek }
                if let loop, loop.count == 2 {
                    state.loopStartBar = loop[0]
                    state.loopEndBar = loop[1]
                }
                if state.speed <= 0 { state.speed = 1 }
                try core.setPracticeState(tab: id, state: state)
                if let practiced { try core.recordPractice(tab: id, seconds: practiced) }
                for (name, start, end) in savedLoops {
                    _ = try core.saveLoop(tab: id, name: name, startBar: start, endBar: end)
                }
                if let videoID {
                    let video = try core.addVideo(tab: id, urlOrId: videoID, label: "Original")
                    try core.setDefaultVideo(id: video)
                    try core.setVideoSync(id: video, sync: sync)
                }
                return .success((core, id))
            } catch {
                return .failure(error)
            }
        }.value
        switch result {
        case let .success((core, id)):
            let model = TabViewerModel(core: core, tabID: id)
            model.autoplay = env["TABSTER_PLAY"] == "1"
            model.startMuted = env["TABSTER_VIDEO_MUTE"] != "0"
            if env["TABSTER_SYNC_PANEL"] == "1" { model.syncEditing = true }
            if let path = env["TABSTER_VIDEO_LOG"] { VideoLog.start(model, path: path) }
            if env["TABSTER_TAPTEST"] == "1" { scheduleTapTest(model) }
            if env["TABSTER_SYNC_PERBAR"] == "1" {
                Task {
                    while model.session == nil { try? await Task.sleep(for: .milliseconds(50)) }
                    model.setPerBarSync(true)
                }
            }
            self.model = model
            if env["TABSTER_SCROLLTEST"] == "1" { scheduleScrollTest() }
            if env["TABSTER_SELFTEST"] == "1" { scheduleSelfTest(model) }
            if env["TABSTER_METRONOME"] == "1" { model.metronome = true }
            if env["TABSTER_METRONOME_POPOVER"] == "1" {
                Task {
                    while model.info == nil { try? await Task.sleep(for: .milliseconds(50)) }
                    try? await Task.sleep(for: .milliseconds(600))
                    model.showMetronomeOptions = true
                }
            }
            if env["TABSTER_TAB_AUDIO_POPOVER"] == "1" {
                Task {
                    while model.info == nil { try? await Task.sleep(for: .milliseconds(50)) }
                    try? await Task.sleep(for: .milliseconds(600))
                    model.showTabAudioOptions = true
                }
            }
            if env["TABSTER_VIDEO_POPOVER"] == "1" {
                Task {
                    while model.info == nil { try? await Task.sleep(for: .milliseconds(50)) }
                    try? await Task.sleep(for: .milliseconds(600))
                    model.showVideoOptions = true
                }
            }
            if env["TABSTER_LOOP_POPOVER"] == "1" {
                Task {
                    while model.info == nil { try? await Task.sleep(for: .milliseconds(50)) }
                    try? await Task.sleep(for: .milliseconds(600))
                    model.loopCurrentSection()
                    model.showLoopOptions = true
                }
            }
            if let snapshot = env["TABSTER_VIEW_SNAPSHOT"] { scheduleSnapshot(to: snapshot) }
            // Measure steady-state playback, not the open/first-layout frames.
            Task {
                try? await Task.sleep(for: .seconds(1))
                ViewerMetrics.tick = TimingStats()
                ViewerMetrics.frameInterval = TimingStats()
            }
        case let .failure(error):
            self.error = "\(error)"
        }
    }

    /// Drives the transport API the keyboard/mouse handlers call and logs the results.
    private func scheduleSelfTest(_ model: TabViewerModel) {
        Task {
            try? await Task.sleep(for: .milliseconds(800))
            @MainActor func state(_ label: String) {
                let bar = model.cursorBar.map { "\($0 + 1)" } ?? "-"
                let loop = model.loop.map { "\($0.lowerBound + 1)-\($0.upperBound + 1)" } ?? "none"
                report(String(format: "selftest %@: t=%.3f bar=%@ loop=%@ playing=%d", label, model.currentTime, bar, loop,
                              model.isPlaying ? 1 : 0))
            }
            state("start")
            model.seek(to: 0)
            for _ in 0..<3 { model.nextBar() }
            state("after 3x next")
            model.setLoopStart()
            model.nextBar()
            model.setLoopEnd()
            state("loop [ ] around bars 4-5")
            model.previousBar()
            state("previous")
            model.play()
            try? await Task.sleep(for: .seconds(4))
            state("played 4s inside loop")
            model.pause()
            model.setLoop(nil)
            model.setSpeed(0.5)
            if let section = model.info?.sections.dropFirst().first {
                model.seek(to: section.scoreSecs)
                state("section \(section.label) (bar \(section.masterBar + 1))")
            }
            model.seek(atX: 300, y: 260)
            state("click at (300,260)")
            model.play()
            try? await Task.sleep(for: .seconds(1))
            state("played 1s at 50%")
            model.pause()
            model.setSpeed(1)
        }
    }

    /// Taps T three times, one bar apart, and logs where each tap put its bar.
    private func scheduleTapTest(_ model: TabViewerModel) {
        Task {
            try? await Task.sleep(for: .seconds(5))
            for _ in 0..<3 {
                model.tapHere()
                if let tap = model.lastTap {
                    let bar = model.playedBars[tap.bar]
                    report(String(format: "taptest: played bar %d (bar %d) -> video %.3f; map gives %.3f; keyframes %d",
                                  tap.bar, bar.masterBar + 1, tap.video, model.barVideoTimes[tap.bar],
                                  model.syncKeyframes.count))
                }
                try? await Task.sleep(for: .seconds(60.0 / 166 * 4))
            }
        }
    }

    private func scheduleScrollTest() {
        Task {
            try? await Task.sleep(for: .seconds(1))
            guard let scroll = findScrollView(in: NSApp.windows.first?.contentView) else { return }
            ViewerMetrics.draw = TimingStats()
            let clip = scroll.contentView
            let maxY = (scroll.documentView?.frame.height ?? 0) - clip.bounds.height
            var y: CGFloat = 0
            let start = CACurrentMediaTime()
            var frames = 0
            while y < maxY {
                y += 24
                clip.setBoundsOrigin(NSPoint(x: 0, y: min(y, maxY)))
                scroll.reflectScrolledClipView(clip)
                frames += 1
                try? await Task.sleep(for: .milliseconds(8))
            }
            let line = String(format: "scrolltest: %d steps over %.0fpt in %.2fs; draw %@",
                              frames, maxY, CACurrentMediaTime() - start, ViewerMetrics.draw.summary)
            report(line)
        }
    }

    private func scheduleSnapshot(to path: String) {
        Task {
            let wait: Double = env["TABSTER_SNAPSHOT_AFTER"].flatMap(Double.init)
                ?? (env["TABSTER_SELFTEST"] == "1" ? 8 : env["TABSTER_SCROLLTEST"] == "1" ? 6 : 2.2)
            try? await Task.sleep(for: .seconds(wait))
            guard let window = NSApp.windows.first(where: { $0.isVisible }),
                  let view = window.contentView?.superview ?? window.contentView
            else { return }
            view.layoutSubtreeIfNeeded()
            if let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) {
                view.cacheDisplay(in: view.bounds, to: rep)
                try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
            }
            for (i, other) in NSApp.windows.enumerated() where other !== window && other.isVisible {
                guard let content = other.contentView?.superview ?? other.contentView,
                      content.bounds.width > 40,
                      let rep = content.bitmapImageRepForCachingDisplay(in: content.bounds) else { continue }
                content.cacheDisplay(in: content.bounds, to: rep)
                let extra = path.replacingOccurrences(of: ".png", with: "-window\(i).png")
                try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: extra))
            }
            report(String(format: "open %.1f ms, first layout applied %.1f ms, first render %.1f ms (from model init)",
                          ViewerMetrics.openMs ?? -1, ViewerMetrics.firstLayoutMs ?? -1, ViewerMetrics.firstRenderMs ?? -1))
            report(String(format: "background open work: openTab %.1f ms, total incl. played-bar table + first layout %.1f ms",
                          ViewerMetrics.openWork?.openTab ?? -1, ViewerMetrics.openWork?.total ?? -1))
            report("layout \(ViewerMetrics.layout.summary)")
            report("draw \(ViewerMetrics.draw.summary)")
            report("tick \(ViewerMetrics.tick.summary)")
            report("frame interval \(ViewerMetrics.frameInterval.summary)")
            if let doc = findScrollView(in: window.contentView)?.documentView {
                let layer = doc.layer
                report("document \(Int(doc.frame.width))x\(Int(doc.frame.height))pt layer=\(layer.map { String(describing: type(of: $0)) } ?? "none") sublayers=\(layer?.sublayers?.count ?? 0) rss=\(Self.residentMB())MB")
            }
            if let model {
                report(String(format: "position %.2fs playing=%d", model.currentTime, model.isPlaying ? 1 : 0))
                if let player = model.player {
                    report(String(format: "video %@ state=%@ t=%.3f rate=%.2f failure=%@ videoClock=%d",
                                  player.videoID ?? "-", player.state.rawValue, player.time(at: CACurrentMediaTime()),
                                  player.rate, player.failure.map { "\($0)" } ?? "none", model.usingVideoClock ? 1 : 0))
                }
                VideoLog.finish()
            }
            // Also a real on-screen capture (includes toolbar controls cacheDisplay can't draw);
            // needs the Screen Recording permission, so it's best-effort.
            let screen = Process()
            screen.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
            screen.arguments = ["-x", "-o", "-l\(window.windowNumber)", path.replacingOccurrences(of: ".png", with: "-screen.png")]
            try? screen.run()
            screen.waitUntilExit()
            model?.close()
            try? await Task.sleep(for: .milliseconds(200))
            NSApp.terminate(nil)
        }
    }

    private static func residentMB() -> Int {
        var info = mach_task_basic_info()
        var count = mach_msg_type_number_t(MemoryLayout<mach_task_basic_info>.size / MemoryLayout<natural_t>.size)
        let result = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), $0, &count)
            }
        }
        return result == KERN_SUCCESS ? Int(info.resident_size / 1_048_576) : -1
    }

    private func report(_ line: String) {
        ViewerLog.log.info("\(line, privacy: .public)")
        FileHandle.standardError.write(Data("[viewer] \(line)\n".utf8))
    }

    private func findScrollView(in view: NSView?) -> NSScrollView? {
        guard let view else { return nil }
        if let scroll = view as? NSScrollView, scroll.documentView is TabDocumentView { return scroll }
        for sub in view.subviews {
            if let found = findScrollView(in: sub) { return found }
        }
        return nil
    }
}

/// Per-frame JSON lines for checking the cursor against the video (`TABSTER_VIDEO_LOG`).
@MainActor
private enum VideoLog {
    static var handle: FileHandle?
    static let start0 = CACurrentMediaTime()

    static func start(_ model: TabViewerModel, path: String) {
        FileManager.default.createFile(atPath: path, contents: nil)
        handle = FileHandle(forWritingAtPath: path)
        model.frameObserver = { host, video, score, bar in
            write(["type": "frame", "host": host - start0, "v": video, "s": score, "bar": bar.map { Int($0) } ?? -1,
                   "running": model.player?.clock.isRunning ?? false])
        }
        Task {
            while model.player == nil { try? await Task.sleep(for: .milliseconds(50)) }
            model.player?.onSample = { t, host in write(["type": "sample", "t": t, "host": host - start0]) }
            model.player?.onEvent = { type, body in
                guard type != "time" else { return }
                var fields = body.filter { $0.value is String || $0.value is NSNumber }
                fields["host"] = CACurrentMediaTime() - start0
                write(fields)
            }
        }
    }

    static func write(_ fields: [String: Any]) {
        guard let handle, let data = try? JSONSerialization.data(withJSONObject: fields, options: [.sortedKeys]) else { return }
        handle.write(data + Data("\n".utf8))
    }

    static func finish() {
        try? handle?.close()
        handle = nil
    }
}

private struct HarnessError: Error, CustomStringConvertible {
    let message: String
    var description: String { message }
}
#endif
