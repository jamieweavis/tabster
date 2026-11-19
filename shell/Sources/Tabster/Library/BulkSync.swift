import os
import SwiftUI

/// A bulk listen-and-sync run: the untracked tabs still to go, and how the ones so far went.
/// `AppModel` opens each in turn; its viewer listens to the video and reports back
/// (`TabViewerModel.syncForBulk`).
struct BulkSyncRun: Equatable {
    let queue: [Int64]
    /// How long each queued tab's video runs (the tab's own length when the video's isn't known).
    let lengths: [Double]
    var index = 0
    /// When the current tab's turn began.
    var turnStarted = Date()
    var synced = 0
    /// Tabs that were listened to but couldn't be lined up.
    var failed = 0
    /// Tabs whose video wouldn't play.
    var skipped = 0
    /// Untracked tabs left out because their sync was made by hand.
    let handSynced: Int

    /// The tab being listened to.
    var current: Int64? { queue.indices.contains(index) ? queue[index] : nil }

    /// Loading the video and lining the tab up, on top of each video's length.
    private static let overheadSecs = 8.0

    /// Roughly how long the rest of the run takes from `now`: what's left of the current video
    /// and every one after it, each heard once through at normal speed.
    func remainingSecs(at now: Date) -> Double {
        guard queue.indices.contains(index) else { return 0 }
        let current = max(0, lengths[index] + Self.overheadSecs - now.timeIntervalSince(turnStarted))
        return lengths[(index + 1)...].reduce(current) { $0 + $1 + Self.overheadSecs }
    }

    /// "12:34" or "1:02:03".
    static func countdown(_ secs: Double) -> String {
        let s = Int(secs.rounded(.up))
        return s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60)
                         : String(format: "%d:%02d", s / 60, s % 60)
    }
}

/// How one tab of a bulk sync went.
enum BulkSyncOutcome {
    case synced
    case failed(String)
    case skipped(String)
    /// Something that would fail every tab alike (no audio access): end the run.
    case abort(String)
}

extension AppModel {
    func toggleBulkSync() {
        if bulkSync == nil { startBulkSync() } else { stopBulkSync() }
    }

    /// Queue every tab with a video that has never been synced, and open the first.
    func startBulkSync() {
        guard let core, bulkSync == nil, !preparingBulkSync else { return }
        preparingBulkSync = true
        Task {
            let found = await Task.detached { Result { try Self.untrackedTabs(core) } }.value
            preparingBulkSync = false
            switch found {
            case let .success((queue, lengths, handSynced)):
                Logger.library.info("bulk sync: \(queue.count) untracked tab(s), \(handSynced) synced by hand")
                guard !queue.isEmpty else {
                    bulkSyncSummary = "No tabs to sync: every tab with a video is already tracked"
                        + (handSynced > 0 ? " or synced by hand." : ".")
                    return
                }
                // The run first: changing the selection to anything but its tab stops it.
                bulkSync = BulkSyncRun(queue: queue, lengths: lengths, handSynced: handSynced)
                selection = queue[0]
            case let .failure(error):
                errorMessage = error.userMessage
            }
        }
    }

    /// Tabs whose default video has no tracking and no sync at all, in title order; and how many
    /// more are untracked but carry a sync someone made (taps, a file's sync), which a listen
    /// would overwrite.
    private nonisolated static func untrackedTabs(_ core: Core) throws -> (queue: [Int64], lengths: [Double], handSynced: Int) {
        let all = TabFilter(query: "", favouritesOnly: false, tag: nil, setlist: nil, sort: .title)
        var queue: [Int64] = []
        var lengths: [Double] = []
        var handSynced = 0
        for tab in try core.listTabs(filter: all) where tab.hasVideo && tab.tracking == nil {
            let videos = try core.videos(tab: tab.id)
            guard let video = videos.first(where: \.isDefault) ?? videos.first, video.tracking == nil else { continue }
            if video.sync == .offset(offsetSecs: 0, stretch: 1) {
                queue.append(tab.id)
                lengths.append(video.durationSecs.flatMap { $0 > 0 ? $0 : nil } ?? tab.durationSecs)
            } else {
                handSynced += 1
            }
        }
        return (queue, lengths, handSynced)
    }

    /// The viewer of the run's current tab is done with it: on to the next.
    func bulkSyncFinished(tab: Int64, outcome: BulkSyncOutcome) {
        guard var run = bulkSync, run.current == tab else { return }
        switch outcome {
        case .synced:
            run.synced += 1
        case let .failed(message):
            run.failed += 1
            Logger.library.notice("bulk sync: tab \(tab) failed: \(message, privacy: .public)")
        case let .skipped(message):
            run.skipped += 1
            Logger.library.notice("bulk sync: tab \(tab) skipped: \(message, privacy: .public)")
        case let .abort(message):
            Logger.library.error("bulk sync stopped at tab \(tab): \(message, privacy: .public)")
            bulkSync = nil
            bulkSyncSummary = Self.summary(of: run, stopped: message)
            return
        }
        run.index += 1
        run.turnStarted = Date()
        if let next = run.current {
            bulkSync = run
            selection = next
        } else {
            bulkSync = nil
            bulkSyncSummary = Self.summary(of: run, stopped: nil)
        }
    }

    /// Stop the run (the button, or another tab was chosen). The open viewer stops listening.
    func stopBulkSync() {
        guard let run = bulkSync else { return }
        bulkSync = nil
        Logger.library.info("bulk sync stopped at \(run.index + 1) of \(run.queue.count)")
        if run.synced + run.failed + run.skipped > 0 {
            bulkSyncSummary = Self.summary(of: run, stopped: "Stopped before the rest were listened to.")
        }
    }

    private static func summary(of run: BulkSyncRun, stopped: String?) -> String {
        var lines = ["Synced \(run.synced) of \(run.queue.count) tab\(run.queue.count == 1 ? "" : "s") by ear."]
        if run.failed > 0 { lines.append("\(run.failed) couldn't be lined up with what was heard.") }
        if run.skipped > 0 { lines.append("\(run.skipped) skipped: the video wouldn't play.") }
        if run.handSynced > 0 { lines.append("\(run.handSynced) left alone: already synced by hand.") }
        if let stopped { lines.append(stopped) }
        return lines.joined(separator: "\n")
    }
}

/// The toolbar's bulk sync button: starts a run, or shows how far it is and stops it.
struct BulkSyncButton: View {
    /// Nil in the debug harness (no library to go through).
    let app: AppModel?

    var body: some View {
        Button {
            app?.toggleBulkSync()
        } label: {
            if let run = app?.bulkSync {
                // Ticks down by the second; re-estimated as each tab finishes.
                TimelineView(.periodic(from: .now, by: 1)) { context in
                    let left = BulkSyncRun.countdown(run.remainingSecs(at: context.date))
                    Label("\(run.index + 1) of \(run.queue.count) · ~\(left) left", systemImage: "stop.circle")
                        .labelStyle(.titleAndIcon)
                        .monospacedDigit()
                }
            } else {
                Label("Sync Untracked Tabs", systemImage: "ear.badge.waveform")
            }
        }
        .help(app?.bulkSync == nil
              ? "Listen to every untracked tab's video, one after another, and sync each by ear"
              : "Listening through the untracked tabs — click to stop")
        .disabled(app?.core == nil || app?.preparingBulkSync == true)
    }
}
