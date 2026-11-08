import AppKit
import TabsterKit
import SwiftUI
import WebKit

/// The YouTube player, 16:9 with rounded corners, with overlays for loading and failures.
struct YouTubePlayerView: View {
    let controller: YouTubePlayerController
    var cornerRadius: CGFloat = 10

    var body: some View {
        ZStack {
            Color.black
            WebViewHost(webView: controller.webView)
                .opacity(controller.failure == nil ? 1 : 0)
            overlay
        }
        .aspectRatio(16 / 9, contentMode: .fit)
        .clipShape(.rect(cornerRadius: cornerRadius))
    }

    @ViewBuilder private var overlay: some View {
        if let failure = controller.failure {
            FailureOverlay(failure: failure, watchURL: controller.watchURL, retry: controller.retry)
        } else if controller.videoID != nil, !controller.isVideoReady {
            ProgressView()
                .controlSize(.large)
                .tint(.white)
                .accessibilityLabel("Loading video")
        }
    }
}

private struct FailureOverlay: View {
    let failure: YouTubePlayerFailure
    let watchURL: URL?
    let retry: () -> Void

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: failure.isPermanent ? "play.slash" : "exclamationmark.triangle")
                .font(.largeTitle)
            Text(failure.title).font(.headline)
            Text(failure.detail)
                .font(.callout)
                .foregroundStyle(.white.opacity(0.7))
                .multilineTextAlignment(.center)
            HStack {
                if !failure.isPermanent {
                    Button("Retry", systemImage: "arrow.clockwise", action: retry)
                }
                if let watchURL {
                    Button("Open in YouTube", systemImage: "arrow.up.forward.app") {
                        NSWorkspace.shared.open(watchURL)
                    }
                }
            }
            .padding(.top, 4)
        }
        .foregroundStyle(.white)
        .padding()
        .environment(\.colorScheme, .dark)
    }
}

/// Hosts the controller's single WKWebView. A WKWebView can only live in one superview, so it is
/// re-parented into whichever container SwiftUI currently shows.
private struct WebViewHost: NSViewRepresentable {
    let webView: WKWebView

    func makeNSView(context: Context) -> NSView {
        let container = NSView()
        attach(to: container)
        return container
    }

    func updateNSView(_ container: NSView, context: Context) {
        if webView.superview !== container { attach(to: container) }
    }

    private func attach(to container: NSView) {
        webView.removeFromSuperview()
        webView.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(webView)
        NSLayoutConstraint.activate([
            webView.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            webView.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            webView.topAnchor.constraint(equalTo: container.topAnchor),
            webView.bottomAnchor.constraint(equalTo: container.bottomAnchor),
        ])
    }
}
