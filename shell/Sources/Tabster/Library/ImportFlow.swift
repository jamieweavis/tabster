import os
import SwiftUI

/// The guided import for one tab file (`AppModel.beginImport`): import it and show what it is,
/// find its YouTube video, then open it, syncing it by ear straight away if it has a video.
/// Several files, or a folder, skip all this and import in one go.
struct ImportFlowSheet: View {
    let core: Core
    let request: ImportFlowRequest
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @State private var step: Step = .importing
    @State private var attachError: String?

    private enum Step {
        case importing
        case failed(String)
        /// Imported; choosing its video.
        case video(Imported)
        /// Ready to open, with the video it'll play (nil: none).
        case finish(Imported, Video?)

        var index: Int {
            switch self {
            case .importing, .failed: 0
            case .video: 1
            case .finish: 2
            }
        }
    }

    private struct Imported {
        let tab: TabSummary
        /// The file was already in the library.
        let duplicate: Bool
    }

    private var fileName: String { request.file.lastPathComponent }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline) {
                Text("Import Tab").font(.headline)
                Spacer()
                StepIndicator(current: step.index)
            }
            .padding([.horizontal, .top], 20)
            content
        }
        .frame(width: 560)
        .task(id: request.id) { await runImport() }
        .alert("Couldn't Attach the Video", isPresented: Binding(
            get: { attachError != nil }, set: { if !$0 { attachError = nil } }
        ), presenting: attachError) { _ in
            Button("OK") {}
        } message: { Text($0) }
    }

    @ViewBuilder private var content: some View {
        switch step {
        case .importing:
            ProgressView("Importing \(fileName)…")
                .frame(maxWidth: .infinity, minHeight: 200)
        case let .failed(message):
            failed(message)
        case let .video(imported):
            TabCard(tab: imported.tab, duplicate: imported.duplicate)
                .padding([.horizontal, .top], 20)
            VideoFinderSheet(
                core: core,
                tabID: imported.tab.id,
                onAttachMatch: { video, label in
                    attach(imported, youtubeID: video.youtubeId) {
                        try core.attachVideo(tab: imported.tab.id, video: video, label: label)
                    }
                },
                onAttachID: { id, label in
                    attach(imported, youtubeID: id) {
                        try core.addVideo(tab: imported.tab.id, urlOrId: id, label: label)
                    }
                },
                onSkip: { step = .finish(imported, nil) },
                heading: "Find Its Video"
            )
        case let .finish(imported, video):
            TabCard(tab: imported.tab, duplicate: imported.duplicate)
                .padding([.horizontal, .top], 20)
            finish(imported, video)
        }
    }

    // MARK: Steps

    private func failed(_ message: String) -> some View {
        VStack(spacing: 16) {
            ContentUnavailableView {
                Label("Couldn't Import \(fileName)", systemImage: "exclamationmark.triangle")
            } description: {
                Text(message)
            }
            .frame(minHeight: 180)
            HStack {
                Button("Choose Another File…", action: importAnother)
                Spacer()
                Button("Close") { dismiss() }
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
    }

    private func finish(_ imported: Imported, _ video: Video?) -> some View {
        let synced = video.map { $0.tracking != nil || $0.sync != .offset(offsetSecs: 0, stretch: 1) } ?? false
        return VStack(alignment: .leading, spacing: 16) {
            if let video {
                HStack(alignment: .top, spacing: 10) {
                    VideoThumbnail(youtubeID: video.youtubeId, duration: video.durationSecs, width: 96)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(video.title.isEmpty ? "YouTube video \(video.youtubeId)" : video.title)
                            .font(.body.weight(.medium))
                            .lineLimit(2)
                        Text([video.channel, video.label].filter { !$0.isEmpty }.joined(separator: " · "))
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }
                Text(synced
                     ? "This video is already synced to the tab."
                     : "Sync by ear plays the video and listens to your Mac's audio to place every bar. It takes as long as the song; choose Stop & Sync in the sync bar to finish early.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                Label("No video attached. The tab plays with its own sound; you can add a video later.",
                      systemImage: "video.slash")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
            HStack(spacing: 8) {
                Button("Import Another…", action: importAnother)
                Spacer()
                if video == nil {
                    Button("Find a Video") { step = .video(imported) }
                    Button("Open Tab") { open(imported.tab, listen: false) }
                        .keyboardShortcut(.defaultAction)
                } else if synced {
                    Button("Open Tab") { open(imported.tab, listen: false) }
                        .keyboardShortcut(.defaultAction)
                } else {
                    Button("Open Tab") { open(imported.tab, listen: false) }
                    Button("Open & Sync by Ear") { open(imported.tab, listen: true) }
                        .keyboardShortcut(.defaultAction)
                }
            }
        }
        .padding(20)
        // Closes without opening the tab (it's in the library either way).
        .background {
            Button("Close") { dismiss() }
                .keyboardShortcut(.cancelAction)
                .hidden()
        }
    }

    // MARK: Actions

    private func runImport() async {
        step = .importing
        let core = core
        let path = request.file.path(percentEncoded: false)
        Logger.library.info("guided import of \(path, privacy: .public)")
        let result = await Task.detached { () -> Result<(Imported, [Video]), Error> in
            Result {
                guard let outcome = core.importPaths(paths: [path]).first else {
                    throw ImportFlowError.notATab
                }
                if let error = outcome.error { throw ImportFlowError.failed(error) }
                guard let id = outcome.id else { throw ImportFlowError.notATab }
                return (Imported(tab: try core.tab(id: id), duplicate: outcome.duplicate), try core.videos(tab: id))
            }
        }.value
        switch result {
        case let .success((imported, videos)):
            // Already has a video (imported before, with one): straight to opening it.
            if let video = videos.first(where: \.isDefault) ?? videos.first {
                step = .finish(imported, video)
            } else {
                step = .video(imported)
            }
        case let .failure(error):
            Logger.library.error("guided import of \(path, privacy: .public) failed: \(error, privacy: .public)")
            step = .failed((error as? ImportFlowError)?.message ?? error.userMessage)
        }
    }

    /// Attach the chosen video (unless it's attached already) and make it the tab's default: the
    /// background search may have attached its own pick meanwhile, and the one chosen here wins.
    private func attach(_ imported: Imported, youtubeID: String, add: () throws -> Int64) {
        do {
            let existing = try core.videos(tab: imported.tab.id)
            let id: Int64
            if let same = existing.first(where: { $0.youtubeId == youtubeID }) {
                id = same.id
            } else {
                id = try add()
            }
            if existing.contains(where: { $0.isDefault && $0.id != id }) { try core.setDefaultVideo(id: id) }
            step = .finish(imported, try core.videos(tab: imported.tab.id).first { $0.id == id })
        } catch {
            Logger.library.error("guided import: couldn't attach \(youtubeID, privacy: .public): \(error, privacy: .public)")
            attachError = error.userMessage
        }
    }

    private func open(_ tab: TabSummary, listen: Bool) {
        app.reveal(tab.id)
        if listen { app.listenRequest = ListenRequest(tab: tab.id) }
        dismiss()
    }

    private func importAnother() {
        let urls = ImportPanel.run()
        guard !urls.isEmpty else { return }
        app.importFlow = nil
        app.beginImport(urls)
    }
}

private enum ImportFlowError: Error {
    case notATab
    case failed(String)

    var message: String {
        switch self {
        case .notATab: "It isn't a Guitar Pro tab Tabster can read (.gp, .gpx, .gp5, .gp4 or .gp3)."
        case let .failed(reason): reason
        }
    }
}

/// What was imported: title, artist and album, then tuning, tracks, bars and length.
private struct TabCard: View {
    let tab: TabSummary
    let duplicate: Bool

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "music.note.list")
                .font(.title2)
                .foregroundStyle(.secondary)
                .frame(width: 32)
            VStack(alignment: .leading, spacing: 2) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(tab.title.isEmpty ? "Untitled" : tab.title)
                        .font(.title3.weight(.semibold))
                        .lineLimit(1)
                    if duplicate {
                        Text("Already in Library")
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(.quaternary, in: Capsule())
                            .fixedSize()
                    }
                }
                let byline = [tab.artist, tab.album].filter { !$0.isEmpty }.joined(separator: " · ")
                if !byline.isEmpty {
                    Text(byline).foregroundStyle(.secondary).lineLimit(1)
                }
                Text(details)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
        }
        .padding(12)
        .background(.background.secondary, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator))
    }

    private var details: String {
        [
            tab.tuning,
            "\(tab.trackCount) track\(tab.trackCount == 1 ? "" : "s")",
            "\(tab.barCount) bar\(tab.barCount == 1 ? "" : "s")",
            TabTable.formatLength(tab.durationSecs),
        ]
        .filter { !$0.isEmpty }
        .joined(separator: " · ")
    }
}

/// "① Tab › ② Video › ③ Sync", the current step filled, earlier ones ticked.
private struct StepIndicator: View {
    let current: Int
    private static let steps = ["Tab", "Video", "Sync"]

    var body: some View {
        HStack(spacing: 6) {
            ForEach(Self.steps.indices, id: \.self) { i in
                if i > 0 {
                    Image(systemName: "chevron.right")
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                }
                Label(Self.steps[i], systemImage: symbol(i))
                    .foregroundStyle(i == current ? AnyShapeStyle(.tint) : AnyShapeStyle(i < current ? .primary : .secondary))
            }
        }
        .font(.callout)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Step \(current + 1) of \(Self.steps.count): \(Self.steps[current])")
    }

    private func symbol(_ i: Int) -> String {
        i < current ? "checkmark.circle.fill" : i == current ? "\(i + 1).circle.fill" : "\(i + 1).circle"
    }
}
