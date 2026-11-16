import SwiftUI

/// Offers the video a background search found but wasn't sure enough to attach: use it, see the
/// other results, or stop suggesting.
struct VideoSuggestionBar: View {
    let match: VideoMatch
    let onUse: () -> Void
    let onBrowse: () -> Void
    let onDismiss: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            VideoThumbnail(youtubeID: match.candidate.youtubeId, duration: match.candidate.durationSecs, width: 64)
            VStack(alignment: .leading, spacing: 2) {
                Text("Is this the video for this tab?")
                    .font(.callout.weight(.semibold))
                Text(caption)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 8)
            Button("Use Video", action: onUse)
                .help("Attach \"\(match.candidate.title)\" and play it with the tab")
            Button("Other Videos…", action: onBrowse)
            Button(action: onDismiss) {
                Image(systemName: "xmark")
            }
            .buttonStyle(.borderless)
            .help("Don't suggest videos for this tab")
            .accessibilityLabel("Don't suggest videos for this tab")
        }
        .controlSize(.small)
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(.bar)
    }

    private var caption: String {
        var parts = [match.candidate.title]
        if !match.candidate.channel.isEmpty { parts.append(match.candidate.channel) }
        if let reason = match.reasons.first(where: { $0.contains("tab") }) { parts.append(reason) }
        return parts.joined(separator: " · ")
    }
}
