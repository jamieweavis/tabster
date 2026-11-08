import TabsterKit
import SwiftUI

/// Paste per-bar sync data: a JSON array of video times (one per played bar), the
/// `video-points` object containing `points`, or the whole list (the entry for `videoID` wins).
struct BarPointsSheet: View {
    var videoID: String?
    /// How many played bars the tab has, to warn about a mismatch.
    var playedBarCount: Int
    let onImport: ([Double]) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var text = ""

    private var points: [Double]? {
        text.isEmpty ? nil : SyncEditing.parseBarPoints(text, preferVideoID: videoID)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Paste Per-Bar Sync Points").font(.headline)
            Text("A JSON array of video times in seconds, one per played bar, or a video-points JSON object.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            TextEditor(text: $text)
                .font(.callout.monospaced())
                .frame(minHeight: 140)
                .scrollContentBackground(.hidden)
                .padding(4)
                .background(.background.secondary, in: .rect(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(.separator))
            status
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Import") {
                    if let points { onImport(points) }
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(points == nil)
            }
        }
        .padding(20)
        .frame(width: 460)
        .onAppear {
            if let s = NSPasteboard.general.string(forType: .string), s.count < 200_000,
               SyncEditing.parseBarPoints(s, preferVideoID: videoID) != nil {
                text = s
            }
        }
    }

    @ViewBuilder private var status: some View {
        if let points {
            let extra = points.count - playedBarCount
            if extra == 0 || extra == 1 {
                Label("\(points.count) points for \(playedBarCount) bars", systemImage: "checkmark.circle.fill")
                    .foregroundStyle(.green)
            } else {
                Label("\(points.count) points, but the tab plays \(playedBarCount) bars — they may not match",
                      systemImage: "exclamationmark.triangle.fill")
                    .foregroundStyle(.orange)
            }
        } else if !text.isEmpty {
            Label("Not a list of numbers or a points object", systemImage: "xmark.circle.fill")
                .foregroundStyle(.red)
        } else {
            Label("Paste the JSON here", systemImage: "doc.on.clipboard")
                .foregroundStyle(.secondary)
        }
    }
}
