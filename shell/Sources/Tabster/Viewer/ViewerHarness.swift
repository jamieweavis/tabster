import AppKit
import SwiftUI

/// DEBUG-only harness for developing the viewer without the library UI.
///
/// `TABSTER_VIEW=<tab file>` imports the file into a temporary data dir and shows it in a
/// `TabViewer`. Optional: `TABSTER_TRACK=<n>`, `TABSTER_PLAY=1`, `TABSTER_SEEK=<secs>`,
/// `TABSTER_LOOP=<first>-<last>` (1-based bars),
/// `TABSTER_APPEARANCE=dark|light`, `TABSTER_SCALE=<zoom>`, `TABSTER_WINDOW=<w>x<h>`, `TABSTER_SELFTEST=1` (exercise the transport and log),
/// `TABSTER_SCROLLTEST=1` (scroll
/// through the whole tab and log draw times), and `TABSTER_SNAPSHOT=<png>` (write the window to
/// a PNG after ~2 s, log perf stats and quit).
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
                var state = try core.practiceState(tab: id)
                if let track { state.track = track }
                if let seek { state.lastPositionSecs = seek }
                if let loop, loop.count == 2 {
                    state.loopStartBar = loop[0]
                    state.loopEndBar = loop[1]
                }
                if state.speed <= 0 { state.speed = 1 }
                try core.setPracticeState(tab: id, state: state)
                return .success((core, id))
            } catch {
                return .failure(error)
            }
        }.value
        switch result {
        case let .success((core, id)):
            let model = TabViewerModel(core: core, tabID: id)
            model.autoplay = env["TABSTER_PLAY"] == "1"
            if let scale = env["TABSTER_SCALE"].flatMap(Float.init) { model.zoom(by: scale - 1) }
            self.model = model
            if env["TABSTER_SCROLLTEST"] == "1" { scheduleScrollTest() }
            if env["TABSTER_SELFTEST"] == "1" { scheduleSelfTest(model) }
            if let snapshot = env["TABSTER_SNAPSHOT"] { scheduleSnapshot(to: snapshot) }
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
            let wait: Double = env["TABSTER_SELFTEST"] == "1" ? 8 : env["TABSTER_SCROLLTEST"] == "1" ? 6 : 2.2
            try? await Task.sleep(for: .seconds(wait))
            guard let window = NSApp.windows.first(where: { $0.isVisible }),
                  let view = window.contentView?.superview ?? window.contentView
            else { return }
            view.layoutSubtreeIfNeeded()
            if let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) {
                view.cacheDisplay(in: view.bounds, to: rep)
                try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
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

private struct HarnessError: Error, CustomStringConvertible {
    let message: String
    var description: String { message }
}
#endif
