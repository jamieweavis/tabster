import AppKit
import Observation
import QuartzCore
import TabsterKit
import WebKit
import os

/// Owns the WKWebView that hosts `youtube-player.html` (the IFrame Player API), sends it commands
/// and turns its events into observable state plus a smooth `VideoClock`.
///
/// Loading: `loadHTMLString(html, baseURL: https://tabster.local/)` gives the embed an origin
/// and Referer. If YouTube still answers error 153 (no Referer), it retries once from the Rust
/// core's localhost server (`Core.playerUrl()`). See PLAN.md §0 "YouTube embedding".
///
/// Per frame, read `clock.time(at: CACurrentMediaTime())` (or `time(at:)`); it never touches WebKit.
@MainActor
@Observable
final class YouTubePlayerController {
    typealias Failure = YouTubePlayerFailure

    enum PageMode: Equatable {
        /// `loadHTMLString` with the `https://tabster.local/` base URL.
        case inline
        /// `http://localhost:<port>/player.html` from the core's player server.
        case localhost
    }

    static let baseURL = URL(string: "https://tabster.local/")!
    /// The name of the page's `window.webkit.messageHandlers.<name>`.
    static let messageHandlerName = "player"

    // MARK: Observable state

    private(set) var state: PlaybackState = .unstarted
    /// Seconds; 0 until YouTube knows it.
    private(set) var duration: Double = 0
    private(set) var availableRates: [Double] = []
    /// The rate YouTube confirmed (its `rate` event), not the one last requested.
    private(set) var rate: Double = 1
    private(set) var isMuted = false
    private(set) var failure: Failure?
    /// The page's IFrame API loaded; commands go straight through from now on.
    private(set) var isReady = false
    /// The current video's player exists (YouTube's `onReady` fired for it).
    private(set) var isVideoReady = false
    private(set) var videoID: String?
    private(set) var pageMode: PageMode = .inline

    // MARK: Non-observable

    @ObservationIgnored let webView: WKWebView
    /// Smooth video time for the cursor. Read it per frame; it is updated by page events.
    @ObservationIgnored let clock = VideoClock()
    /// Every raw time sample (video seconds, host seconds), for diagnostics.
    @ObservationIgnored var onSample: ((Double, Double) -> Void)?
    /// Every page event (`type`, body), for diagnostics.
    @ObservationIgnored var onEvent: ((String, [String: Any]) -> Void)?

    @ObservationIgnored private let core: Core?
    @ObservationIgnored private var pending: [(js: String, args: [String: Any])] = []
    @ObservationIgnored private var mapper = HostClockMapper()
    @ObservationIgnored private var html: String?
    @ObservationIgnored private var readyTimeout: Task<Void, Never>?
    @ObservationIgnored private var lastAutoplay = false
    @ObservationIgnored private let delegate = NavigationDelegate()

    private static let log = Logger(subsystem: "com.jamieweavis.tabster", category: "youtube")

    /// `core` provides the localhost fallback for error 153; without it that error is final.
    init(core: Core? = nil) {
        self.core = core
        let config = WKWebViewConfiguration()
        config.mediaTypesRequiringUserActionForPlayback = []
        config.allowsAirPlayForMediaPlayback = true
        config.preferences.isElementFullscreenEnabled = true
        config.websiteDataStore = .default()
        webView = WKWebView(frame: .zero, configuration: config)
        #if DEBUG
            webView.isInspectable = true
        #endif
        let handler = WeakMessageHandler()
        config.userContentController.add(handler, name: Self.messageHandlerName)
        webView.navigationDelegate = delegate
        webView.uiDelegate = delegate
        handler.receive = { [weak self] body in self?.receive(body) }
        delegate.onTerminate = { [weak self] in self?.fail(.other(-2)) }
        #if DEBUG
            // Exercise the error-153 fallback directly.
            if ProcessInfo.processInfo.environment["TABSTER_YT_PAGE"] == "localhost", core != nil {
                pageMode = .localhost
            }
        #endif
        loadPage()
    }

    // MARK: Commands

    /// Load (or switch to) a video. It starts playing only if `autoplay`.
    func load(videoID: String, startSeconds: Double = 0, autoplay: Bool = false) {
        self.videoID = videoID
        failure = nil
        duration = 0
        isVideoReady = false
        state = .unstarted
        lastAutoplay = autoplay
        clock.reset(to: startSeconds, at: CACurrentMediaTime())
        // Only the latest load matters.
        pending.removeAll { $0.js.contains("tabster.load") }
        send("tabster.load(id, start, opts)", [
            "id": videoID, "start": startSeconds,
            "opts": ["autoplay": autoplay, "mute": isMuted],
        ])
    }

    func play() { send("tabster.play()") }
    func pause() { send("tabster.pause()") }

    /// Seeks; the clock jumps to `seconds` straight away.
    func seek(to seconds: Double) {
        let t = max(0, duration > 0 ? min(seconds, duration) : seconds)
        clock.seek(to: t, at: CACurrentMediaTime())
        send("tabster.seek(s)", ["s": t])
    }

    /// Requests a playback rate. `rate` changes when YouTube confirms it.
    func setRate(_ rate: Double) { send("tabster.setRate(r)", ["r": rate]) }

    func setMuted(_ muted: Bool) {
        isMuted = muted
        send("tabster.setMuted(m)", ["m": muted])
    }

    /// Reload the page (and the current video, from where it was) after a failure.
    func retry() {
        let resume = videoID.map { ($0, clock.time(at: CACurrentMediaTime())) }
        failure = nil
        loadPage()
        if let (id, t) = resume { load(videoID: id, startSeconds: t, autoplay: lastAutoplay) }
    }

    /// The smooth video time at host time `hostTime` (`CACurrentMediaTime()` axis).
    func time(at hostTime: CFTimeInterval) -> Double { clock.time(at: hostTime) }

    /// The video on youtube.com at the current position, for "Open in YouTube".
    var watchURL: URL? {
        videoID.map { YouTubeLinks.watchURL(videoID: $0, at: clock.time(at: CACurrentMediaTime())) }
    }

    // MARK: Page

    private func loadPage() {
        isReady = false
        isVideoReady = false
        mapper.reset()
        guard let html = html ?? Self.loadHTML() else {
            Self.log.error("youtube-player.html is missing from the bundle")
            fail(.other(-3))
            return
        }
        self.html = html
        switch pageMode {
        case .inline:
            webView.loadHTMLString(html, baseURL: Self.baseURL)
        case .localhost:
            guard let core else { return }
            core.setPlayerPage(html: html)
            guard let url = URL(string: core.playerUrl()), url.host != nil else {
                fail(.missingReferrer)
                return
            }
            webView.load(URLRequest(url: url))
        }
        readyTimeout?.cancel()
        readyTimeout = Task { [weak self] in
            try? await Task.sleep(for: .seconds(20))
            guard let self, !Task.isCancelled, !self.isReady else { return }
            Self.log.error("the player page never became ready")
            self.fail(.other(-3))
        }
    }

    /// The bundled page, or in DEBUG builds run without a bundle, the repo's copy.
    static func loadHTML() -> String? {
        if let url = Bundle.main.url(forResource: "youtube-player", withExtension: "html"),
           let html = try? String(contentsOf: url, encoding: .utf8) {
            return html
        }
        #if DEBUG
            // shell/Sources/Tabster/Video/<this file> -> repo root
            var root = URL(fileURLWithPath: #filePath)
            for _ in 0..<5 { root.deleteLastPathComponent() }
            return try? String(contentsOf: root.appending(path: "Resources/Bundle/youtube-player.html"),
                               encoding: .utf8)
        #else
            return nil
        #endif
    }

    private func send(_ js: String, _ args: [String: Any] = [:]) {
        guard isReady else {
            pending.append((js, args))
            return
        }
        webView.callAsyncJavaScript(js, arguments: args, in: nil, in: .page) { result in
            if case .failure(let error) = result {
                Self.log.error("\(js, privacy: .public) failed: \(error.localizedDescription, privacy: .public)")
            }
        }
    }

    private func flush() {
        let queued = pending
        pending.removeAll()
        for command in queued { send(command.js, command.args) }
    }

    private func fail(_ f: Failure) {
        failure = f
        Self.log.notice("player failure: \(String(describing: f), privacy: .public)")
    }

    // MARK: Events

    private func receive(_ body: Any) {
        let now = CACurrentMediaTime()
        guard let msg = body as? [String: Any], let type = msg["type"] as? String else { return }
        onEvent?(type, msg)
        switch type {
        case "apiReady":
            isReady = true
            readyTimeout?.cancel()
            flush()
        case "ready":
            guard msg["videoId"] as? String == videoID else { return }
            isVideoReady = true
            if let rates = msg["rates"] as? [Double], !rates.isEmpty { availableRates = rates }
            if let d = msg["duration"] as? Double, d > 0 { duration = d }
        case "stateChange":
            let new = PlaybackState(youTubeCode: msg["state"] as? Int ?? -1)
            clock.setState(new, time: msg["t"] as? Double, at: now)
            state = new
            if let d = msg["duration"] as? Double, d > 0 { duration = d }
            if failure != nil, new == .playing { failure = nil }
        case "rate":
            if let r = msg["rate"] as? Double {
                rate = r
                clock.rateChanged(to: r, at: now)
            }
        case "time":
            guard let t = msg["t"] as? Double, let perf = msg["perf"] as? Double else { return }
            let host = mapper.hostTime(perfMs: perf, receivedAt: now)
            clock.ingest(time: t, at: host)
            onSample?(t, host)
        case "error":
            handleError(code: msg["code"] as? Int ?? -1)
        case "autoplayBlocked":
            Self.log.notice("autoplay blocked")
        case "jsError", "log":
            Self.log.debug("page: \(String(describing: msg["msg"] ?? ""), privacy: .public)")
        default:
            break
        }
    }

    private func handleError(code: Int) {
        let f = Failure(code: code)
        if f == .missingReferrer, pageMode == .inline, core != nil {
            Self.log.notice("error 153 with the inline page; retrying from localhost")
            let resume = videoID.map { ($0, clock.time(at: CACurrentMediaTime())) }
            pageMode = .localhost
            pending.removeAll()
            loadPage()
            if let (id, t) = resume { load(videoID: id, startSeconds: t, autoplay: lastAutoplay) }
            return
        }
        fail(f)
    }
}

/// Breaks the WKUserContentController -> handler retain cycle.
private final class WeakMessageHandler: NSObject, WKScriptMessageHandler {
    var receive: (@MainActor (Any) -> Void)?

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        MainActor.assumeIsolated { receive?(message.body) }
    }
}

/// Sends links out of the player (title, "Watch on YouTube") to the browser.
private final class NavigationDelegate: NSObject, WKNavigationDelegate, WKUIDelegate {
    var onTerminate: (@MainActor () -> Void)?

    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction) async -> WKNavigationActionPolicy {
        if action.navigationType == .linkActivated, let url = action.request.url {
            NSWorkspace.shared.open(url)
            return .cancel
        }
        return .allow
    }

    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                 for action: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        if let url = action.request.url { NSWorkspace.shared.open(url) }
        return nil
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        MainActor.assumeIsolated { onTerminate?() }
    }
}
