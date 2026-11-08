import AppKit
import QuartzCore
import WebKit

// Usage: YouTubeSpike --video <id> --mode <html-baseurl|html-nobase|localhost|file>
//          [--seconds 20] [--base https://tabster.local/] [--host youtube|nocookie]
//          [--iframe manual|api] [--ua safari|<string>] [--no-script] [--reporter raf|<ms>]
//          [--verbose-time] [--html <path>] [--server-host localhost|127.0.0.1]

struct Options {
    var video = "dQw4w9WgXcQ"
    var mode = PageMode.htmlBaseURL
    var seconds = 20.0
    var base = "https://tabster.local/"
    var host = "https://www.youtube.com"
    var iframe = "manual"
    var userAgent: String?
    var script = true
    var reporter: Any = "raf"
    var verboseTime = false
    var htmlPath: String?
    var serverHost = "localhost"

    static let safariUA =
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15"

    static func parse(_ args: [String]) -> Options {
        var o = Options()
        var it = args.dropFirst().makeIterator()
        while let a = it.next() {
            switch a {
            case "--video": o.video = it.next() ?? o.video
            case "--mode":
                let m = it.next() ?? ""
                guard let mode = PageMode(rawValue: m) else { fatalError("unknown mode \(m)") }
                o.mode = mode
            case "--seconds": o.seconds = Double(it.next() ?? "") ?? o.seconds
            case "--base": o.base = it.next() ?? o.base
            case "--host":
                let h = it.next() ?? ""
                o.host = h == "nocookie" ? "https://www.youtube-nocookie.com" : h == "youtube" ? o.host : h
            case "--iframe": o.iframe = it.next() ?? o.iframe
            case "--ua":
                let u = it.next() ?? ""
                o.userAgent = u == "safari" ? safariUA : u
            case "--no-script": o.script = false
            case "--reporter":
                let r = it.next() ?? "raf"
                o.reporter = Double(r).map { $0 as Any } ?? r
            case "--verbose-time": o.verboseTime = true
            case "--html": o.htmlPath = it.next()
            case "--server-host": o.serverHost = it.next() ?? o.serverHost
            default: fatalError("unknown argument \(a)")
            }
        }
        return o
    }
}

@MainActor
final class Spike {
    let opts: Options
    let controller: YouTubePlayerController
    let window: NSWindow
    var samples: [TimeSample] = []
    var pings: [Ping] = []
    var polls: [Ping] = []
    var ready = false
    var playingSince: Double?
    var scriptStarted = false
    var errors: [Int] = []
    var events: [String] = []

    init(opts: Options) {
        self.opts = opts
        controller = YouTubePlayerController(userAgent: opts.userAgent)
        window = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 800, height: 450),
                          styleMask: [.titled, .closable, .resizable, .miniaturizable],
                          backing: .buffered, defer: false)
        window.title = "YouTube spike — \(opts.mode.rawValue) \(opts.video)"
        window.contentView = controller.webView
        window.isReleasedWhenClosed = false
        controller.onMessage = { [weak self] in self?.handle($0) }
    }

    func run() async {
        window.makeKeyAndOrderFront(nil)
        NSApp.activate()
        let htmlURL = opts.htmlPath.map { URL(fileURLWithPath: $0) } ?? Self.defaultHTML
        guard let html = try? String(contentsOf: htmlURL, encoding: .utf8) else {
            Log.emit(["type": "fatal", "msg": "cannot read \(htmlURL.path)"]); exit(2)
        }
        Log.emit(["type": "config", "mode": opts.mode.rawValue, "video": opts.video, "base": opts.base,
                  "host": opts.host, "iframe": opts.iframe, "ua": opts.userAgent ?? "default",
                  "seconds": opts.seconds, "serverHost": opts.serverHost])
        do {
            try await controller.loadPage(html: html, htmlFile: htmlURL, mode: opts.mode,
                                          baseURL: URL(string: opts.base), serverHost: opts.serverHost)
            if !(await controller.waitForPage()) {
                Log.emit(["type": "swiftError", "where": "load", "msg": "page never posted apiReady"])
            }
            try await controller.setReporter(opts.reporter)
            try await controller.load(videoID: opts.video, options: ["host": opts.host, "iframe": opts.iframe])
        } catch {
            Log.emit(["type": "swiftError", "where": "load", "msg": "\(error)"])
        }
        Task { await pollLoop() }
        try? await Task.sleep(for: .seconds(opts.seconds))
        finish()
    }

    static var defaultHTML: URL {
        // spikes/youtube/Sources/YouTubeSpike/main.swift -> repo root
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Resources/Bundle/youtube-player.html")
    }

    func handle(_ m: PageMessage) {
        let b = m.body
        switch m.type {
        case "time":
            let s = TimeSample(t: b["t"] as? Double ?? 0, perf: b["perf"] as? Double ?? 0,
                               epoch: b["epoch"] as? Double ?? 0, rate: b["rate"] as? Double ?? 1,
                               state: b["state"] as? Int ?? -1, received: m.received,
                               receivedEpoch: Date().timeIntervalSince1970 * 1000)
            samples.append(s)
            if opts.verboseTime { Log.emit(b.merging(["recv": m.received - Log.start]) { a, _ in a }) }
        case "error":
            errors.append(b["code"] as? Int ?? -999)
            events.append("error(\(b["code"] ?? "?"))")
            Log.emit(b)
        case "stateChange":
            events.append(b["name"] as? String ?? "?")
            Log.emit(b)
            if b["state"] as? Int == 1, !scriptStarted, opts.script {
                scriptStarted = true
                Task { await runScript() }
            }
        case "ready":
            ready = true
            events.append("ready")
            Log.emit(b)
            Task { await calibrate() }
        default:
            events.append(m.type)
            Log.emit(b)
        }
    }

    /// 30 quick round trips to map JS performance.now() onto CACurrentMediaTime().
    func calibrate() async {
        for _ in 0..<30 {
            if let p = await probe() { pings.append(p) }
            try? await Task.sleep(for: .milliseconds(20))
        }
        let best = pings.min(by: { $0.rtt < $1.rtt })
        Log.emit(["type": "calibration", "pings": pings.count,
                  "bestRTT_ms": ((best?.rtt ?? .nan) * 1000).rounded(places: 3),
                  "rtt_ms": summary(pings.map(\.rtt), scale: 1000)])
    }

    func probe() async -> Ping? {
        let sent = CACurrentMediaTime()
        guard let snap = try? await controller.snapshot(), let perf = snap["perf"] as? Double else { return nil }
        return Ping(sent: sent, returned: CACurrentMediaTime(), perf: perf)
    }

    /// The "currentTime every 250 ms" samples the task asks for: a Swift-driven poll.
    func pollLoop() async {
        while true {
            try? await Task.sleep(for: .milliseconds(250))
            guard ready else { continue }
            let sent = CACurrentMediaTime()
            guard let snap = try? await controller.snapshot() else { continue }
            let returned = CACurrentMediaTime()
            if let perf = snap["perf"] as? Double { polls.append(Ping(sent: sent, returned: returned, perf: perf)) }
            Log.emit(["type": "currentTime", "t": snap["t"] ?? NSNull(), "state": snap["state"] ?? NSNull(),
                      "rtt_ms": ((returned - sent) * 1000).rounded(places: 3)])
        }
    }

    /// Exercises the command bridge once playback starts.
    func runScript() async {
        func at(_ s: Double) async { try? await Task.sleep(for: .seconds(s)) }
        do {
            await at(2)
            let rates = try await controller.availableRates()
            Log.emit(["type": "cmd", "cmd": "getAvailablePlaybackRates", "result": rates])
            await at(1)
            let seekSent = CACurrentMediaTime()
            try await controller.seek(to: 60)
            Log.emit(["type": "cmd", "cmd": "seekTo(60)"])
            await at(3)
            if let firstAfter = samples.first(where: { $0.received > seekSent && abs($0.t - 60) < 1 && $0.state == 1 }) {
                Log.emit(["type": "measure", "what": "seekToPlayingAt60_ms",
                          "value": ((firstAfter.received - seekSent) * 1000).rounded(places: 1)])
            }
            let applied = try await controller.setRate(1.5)
            Log.emit(["type": "cmd", "cmd": "setPlaybackRate(1.5)", "applied": applied])
            await at(3)
            let odd = try await controller.setRate(1.3)
            Log.emit(["type": "cmd", "cmd": "setPlaybackRate(1.3)", "applied": odd])
            await at(1)
            _ = try await controller.setRate(1)
            Log.emit(["type": "cmd", "cmd": "setPlaybackRate(1)"])
            await at(1)
            try await controller.pause()
            Log.emit(["type": "cmd", "cmd": "pause"])
            await at(1.5)
            try await controller.play()
            Log.emit(["type": "cmd", "cmd": "play"])
        } catch {
            Log.emit(["type": "swiftError", "where": "script", "msg": "\(error)"])
        }
    }

    func finish() {
        var r = Analysis.report(samples: samples, pings: pings, polls: polls)
        r["mode"] = opts.mode.rawValue
        r["base"] = opts.base
        r["host"] = opts.host
        r["iframe"] = opts.iframe
        r["ua"] = opts.userAgent == nil ? "default" : "custom"
        r["video"] = opts.video
        r["ready"] = ready
        r["errors"] = errors
        r["events"] = events
        r["verdict"] = ready && errors.isEmpty && samples.contains { $0.state == 1 } ? "WORKS" : "FAILS"
        r["page_ua"] = controller.webView.customUserAgent ?? ""
        Log.emit(r)
        exit(0)
    }
}

let opts = Options.parse(CommandLine.arguments)
let app = NSApplication.shared
app.setActivationPolicy(.regular)
MainActor.assumeIsolated {
    let spike = Spike(opts: opts)
    Task { await spike.run() }
}
app.run()
