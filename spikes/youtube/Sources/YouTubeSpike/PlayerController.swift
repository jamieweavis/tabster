import AppKit
import QuartzCore
import WebKit

/// How the player page gets its origin. See FINDINGS.md.
enum PageMode: String {
    case htmlBaseURL = "html-baseurl"   // loadHTMLString(html, baseURL: <base>)
    case htmlNoBase = "html-nobase"     // loadHTMLString(html, baseURL: nil) — the 153 baseline
    case localhost                      // http://127.0.0.1:<port>/player.html via LocalServer
    case file                           // loadFileURL(file://…/youtube-player.html)
}

/// One message from the page, stamped with the Swift receive time.
struct PageMessage {
    let body: [String: Any]
    let received: CFTimeInterval   // CACurrentMediaTime()
    var type: String { body["type"] as? String ?? "?" }
}

/// Prototype of the Phase 3 `YouTubePlayerController`: owns the WKWebView, loads the player page,
/// sends commands (Swift → JS) and surfaces page events (JS → Swift).
@MainActor
final class YouTubePlayerController: NSObject {
    let webView: WKWebView
    var onMessage: ((PageMessage) -> Void)?
    private var server: LocalServer?

    init(userAgent: String?) {
        let config = WKWebViewConfiguration()
        config.mediaTypesRequiringUserActionForPlayback = []
        config.allowsAirPlayForMediaPlayback = true
        config.preferences.isElementFullscreenEnabled = true
        config.websiteDataStore = .default()
        webView = WKWebView(frame: .zero, configuration: config)
        super.init()
        config.userContentController.add(WeakMessageHandler(self), name: "player")
        webView.navigationDelegate = self
        webView.uiDelegate = self
        webView.isInspectable = true
        if let userAgent { webView.customUserAgent = userAgent }
    }

    func loadPage(html: String, htmlFile: URL, mode: PageMode, baseURL: URL?,
                  serverHost: String = "localhost") async throws {
        switch mode {
        case .htmlBaseURL:
            webView.loadHTMLString(html, baseURL: baseURL)
        case .htmlNoBase:
            webView.loadHTMLString(html, baseURL: nil)
        case .localhost:
            let server = try LocalServer(html: html)
            let port = try await server.start()
            self.server = server
            let url = URL(string: "http://\(serverHost):\(port)/player.html")!
            Log.emit(["type": "server", "url": url.absoluteString])
            webView.load(URLRequest(url: url))
        case .file:
            webView.loadFileURL(htmlFile, allowingReadAccessTo: htmlFile.deletingLastPathComponent())
        }
    }

    /// True once the page posted `apiReady` (its `window.tabster` exists and the IFrame API loaded).
    private(set) var pageReady = false

    /// Commands sent before the page's script ran fail with "Can't find variable: tabster", so
    /// wait for `apiReady` first. Returns false on timeout (e.g. iframe_api blocked).
    func waitForPage(timeout: Duration = .seconds(10)) async -> Bool {
        let deadline = ContinuousClock.now + timeout
        while !pageReady, ContinuousClock.now < deadline { try? await Task.sleep(for: .milliseconds(20)) }
        return pageReady
    }

    // MARK: Commands (Swift → JS)

    @discardableResult
    func call(_ js: String, _ args: [String: Any] = [:]) async throws -> Any? {
        try await webView.callAsyncJavaScript(js, arguments: args, contentWorld: .page)
    }

    func load(videoID: String, start: Double = 0, options: [String: Any] = [:]) async throws {
        try await call("return tabster.load(id, start, opts)", ["id": videoID, "start": start, "opts": options])
    }
    func play() async throws { try await call("tabster.play()") }
    func pause() async throws { try await call("tabster.pause()") }
    func seek(to seconds: Double) async throws { try await call("tabster.seek(s)", ["s": seconds]) }
    /// Returns the rate YouTube actually applied.
    func setRate(_ rate: Double) async throws -> Double {
        (try await call("return tabster.setRate(r)", ["r": rate]) as? Double) ?? 1
    }
    func availableRates() async throws -> [Double] {
        (try await call("return tabster.rates()") as? [Double]) ?? []
    }
    func snapshot() async throws -> [String: Any] {
        (try await call("return tabster.snapshot()") as? [String: Any]) ?? [:]
    }
    func setReporter(_ mode: Any) async throws { try await call("tabster.setReporter(m)", ["m": mode]) }

    fileprivate func receive(_ body: Any) {
        let now = CACurrentMediaTime()
        guard let dict = body as? [String: Any] else { return }
        if dict["type"] as? String == "apiReady" { pageReady = true }
        onMessage?(PageMessage(body: dict, received: now))
    }
}

extension YouTubePlayerController: WKNavigationDelegate, WKUIDelegate {
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        Log.emit(["type": "nav", "event": "didFinish", "url": webView.url?.absoluteString ?? "nil"])
    }
    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        Log.emit(["type": "nav", "event": "didFail", "error": error.localizedDescription])
    }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        Log.emit(["type": "nav", "event": "didFailProvisional", "error": error.localizedDescription])
    }
    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        Log.emit(["type": "nav", "event": "contentProcessTerminated"])
    }
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction) async -> WKNavigationActionPolicy {
        // Links out of the player (title, "Watch on YouTube", errors' "more info") go to the browser.
        if action.navigationType == .linkActivated, let url = action.request.url {
            Log.emit(["type": "nav", "event": "external", "url": url.absoluteString])
            NSWorkspace.shared.open(url)
            return .cancel
        }
        return .allow
    }
    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                 for action: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        if let url = action.request.url {
            Log.emit(["type": "nav", "event": "popup", "url": url.absoluteString])
            NSWorkspace.shared.open(url)
        }
        return nil
    }
}

/// Breaks the WKUserContentController → handler retain cycle.
final class WeakMessageHandler: NSObject, WKScriptMessageHandler {
    weak var target: YouTubePlayerController?
    init(_ target: YouTubePlayerController) { self.target = target }
    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        MainActor.assumeIsolated { target?.receive(message.body) }
    }
}
