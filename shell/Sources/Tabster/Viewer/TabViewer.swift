import SwiftUI

/// The tab viewer for one open tab: renders the tab, transport, track picker, (later) video.
/// Contract between the library UI and the viewer: `TabViewer(core:tabID:)` is the detail view.
/// Phase 0 placeholder; the viewer agent replaces this file.
struct TabViewer: View {
    let core: Core
    let tabID: Int64

    var body: some View {
        Text("Tab \(tabID)").frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
