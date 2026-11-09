import AppKit
import SwiftUI

/// Showing or hiding the split view's sidebar grows or shrinks the window by the sidebar's width,
/// so the detail pane keeps its size (the window's left edge stays; its right edge moves).
/// SwiftUI's `NavigationSplitView` only redistributes the width it has, so this attaches to the
/// `NSSplitView` behind it. Place as a background of the `NavigationSplitView`.
struct SidebarWindowResizer: NSViewRepresentable {
    var sidebarHidden: Bool
    /// Told the sidebar's width (plus the divider) whenever it changes while open, so the window's
    /// minimum can keep room for it: the split view won't narrow the sidebar to fit the window.
    var onSidebarWidth: (CGFloat) -> Void = { _ in }

    func makeNSView(context: Context) -> Probe { Probe() }
    func updateNSView(_ view: Probe, context: Context) {
        view.onSidebarWidth = onSidebarWidth
        view.update(sidebarHidden: sidebarHidden)
    }

    final class Probe: NSView {
        private weak var splitView: NSSplitView?
        private var observer: Any?
        private var sidebarHidden: Bool?
        /// The sidebar's width (plus the divider) when last seen open: what hiding it gives back
        /// and what showing it takes again.
        private var sidebarWidth: CGFloat = 0
        var onSidebarWidth: (CGFloat) -> Void = { _ in }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            if window == nil { detach() } else { DispatchQueue.main.async { [weak self] in self?.attach() } }
        }

        func update(sidebarHidden hidden: Bool) {
            defer { sidebarHidden = hidden }
            guard let was = sidebarHidden, was != hidden, let window else { return }
            if hidden { recordWidth() }
            let delta = sidebarWidth
            guard delta > 0, !window.styleMask.contains(.fullScreen) else { return }
            // After this update, so the split view has started collapsing (or expanding) and the
            // window's minimum size no longer counts the sidebar when it shrinks.
            DispatchQueue.main.async { [weak window] in
                guard let window else { return }
                var frame = window.frame
                if hidden {
                    frame.size.width -= delta
                } else {
                    frame.size.width += delta
                    if let screen = window.screen?.visibleFrame {
                        frame.size.width = min(frame.width, screen.width)
                        frame.origin.x = max(frame.minX, screen.minX)
                        frame.origin.x = min(frame.minX, screen.maxX - frame.width)
                    }
                }
                NSAnimationContext.runAnimationGroup { _ in
                    window.animator().setFrame(frame, display: true)
                }
            }
        }

        private func attach() {
            guard splitView == nil, let content = window?.contentView, let split = outermostSplitView(in: content)
            else { return }
            splitView = split
            recordWidth()
            observer = NotificationCenter.default.addObserver(
                forName: NSSplitView.didResizeSubviewsNotification, object: split, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.recordWidth() }
            }
        }

        private func detach() {
            if let observer { NotificationCenter.default.removeObserver(observer) }
            observer = nil
            splitView = nil
        }

        /// Notes the sidebar's width while it's open — not mid-animation as it closes.
        private func recordWidth() {
            guard sidebarHidden != true, let split = splitView, let sidebar = split.arrangedSubviews.first,
                  !split.isSubviewCollapsed(sidebar), sidebar.frame.width > 0
            else { return }
            let width = sidebar.frame.width + split.dividerThickness
            guard width != sidebarWidth else { return }
            sidebarWidth = width
            // Not mid-layout: this changes SwiftUI state.
            DispatchQueue.main.async { [weak self] in self?.onSidebarWidth(width) }
        }

        /// The window's first `NSSplitView`, searched breadth-first: the one holding the sidebar.
        private func outermostSplitView(in root: NSView) -> NSSplitView? {
            var queue = [root]
            while !queue.isEmpty {
                let view = queue.removeFirst()
                if let split = view as? NSSplitView { return split }
                queue.append(contentsOf: view.subviews)
            }
            return nil
        }
    }
}
