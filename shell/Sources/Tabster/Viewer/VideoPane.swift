import SwiftUI

/// Slot for the YouTube player (Phase 3). The viewer shows it above the tab when
/// `TabViewerModel.showVideo` is on; until then it's an empty placeholder.
struct VideoPane: View {
    var body: some View {
        ZStack {
            Rectangle().fill(.black.opacity(0.85))
            Label("Video arrives in Phase 3", systemImage: "play.rectangle")
                .foregroundStyle(.secondary)
        }
        .aspectRatio(16 / 9, contentMode: .fit)
        .frame(maxHeight: 360)
    }
}
