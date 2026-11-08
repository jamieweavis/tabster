import SwiftUI

/// The video beside or above the tab: the YouTube player and, while syncing, the sync bar and
/// the bar ruler.
struct VideoPane: View {
    let model: TabViewerModel
    /// Beside the tab (a column): the player keeps its 16:9 height and the rest stacks under it.
    /// Above the tab, the player shrinks to whatever height the split leaves.
    var beside = false

    var body: some View {
        VStack(spacing: 10) {
            if let player = model.player {
                if beside {
                    YouTubePlayerView(controller: player)
                        .frame(maxWidth: .infinity)
                } else {
                    YouTubePlayerView(controller: player)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .layoutPriority(-1)
                }
            }
            if model.syncEditing {
                SyncPanel(model: model)
            }
            if beside || model.player == nil { Spacer(minLength: 0) }
        }
        .padding(10)
        .frame(minWidth: 240, minHeight: model.syncEditing ? 220 : 120)
        .background(Color(nsColor: .windowBackgroundColor))
    }
}

/// The sync editor wired to the viewer model.
private struct SyncPanel: View {
    let model: TabViewerModel

    var body: some View {
        VStack(spacing: 8) {
            SyncNudgeBar(
                sync: model.sync,
                canTap: model.usingVideoClock,
                tapFeedback: tapFeedback,
                actions: .init(
                    nudge: { model.nudgeSync(by: $0) },
                    tapBarOne: { model.tapBarOne() },
                    tap: { model.tapHere() },
                    stretch: { model.setStretch($0) },
                    setPerBar: { model.setPerBarSync($0) },
                    importFromFile: model.hasFileSync ? { model.importFileSync() } : nil,
                    pasteBarPoints: { model.showPointsSheet = true },
                    reset: { model.resetSync() }
                )
            )
            SyncRuler(
                videoTime: { model.videoTime(at: $0) },
                marks: { model.rulerMarks(in: $0) },
                isPlaying: model.isPlaying,
                revision: model.syncRevision,
                canDragMarks: model.isPerBarSync,
                moveMark: { model.moveBar($0, toVideo: $1) },
                shift: { model.nudgeSync(by: $0) }
            )
        }
        .padding(10)
        .background(.background.secondary, in: .rect(cornerRadius: 10))
    }

    private var tapFeedback: String? {
        guard let tap = model.lastTap, model.playedBars.indices.contains(tap.bar) else {
            return model.isPerBarSync ? "Play, then press T on each downbeat" : nil
        }
        let bar = model.playedBars[tap.bar].masterBar + 1
        return String(format: "Bar %d → %d:%06.3f", bar, Int(tap.video) / 60, tap.video.truncatingRemainder(dividingBy: 60))
    }
}
