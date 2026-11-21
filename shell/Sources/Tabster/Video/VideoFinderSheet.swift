import AppKit
import SwiftUI

/// Find a YouTube video for a tab: searches for the tab's artist and title as it opens, and lists
/// the results the way the core ranked them (best match first, with why). The field also takes a
/// pasted YouTube link. The caller stores the choice.
struct VideoFinderSheet: View {
    let core: Core
    let tabID: Int64
    /// Called with a search result and the label to attach it under.
    let onAttachMatch: (_ video: VideoCandidate, _ label: String) -> Void
    /// Called with a pasted link's video id and the label.
    let onAttachID: (_ youtubeID: String, _ label: String) -> Void
    /// Set when the finder is a step of another sheet (the guided import): Cancel becomes Skip,
    /// which calls this, and attaching leaves closing to the caller.
    var onSkip: (() -> Void)?
    var heading = "Find a YouTube Video"

    static let suggestedLabels = ["Original", "Live", "Acoustic", "Cover", "Lesson", "Backing Track"]

    @Environment(\.dismiss) private var dismiss
    @State private var input = ""
    /// What the last search was for; the field differs from it once edited.
    @State private var searchedFor: String?
    @State private var phase: Phase = .idle
    @State private var selection: String?
    @State private var label = "Original"
    @State private var labelEdited = false
    @State private var searchTask: Task<Void, Never>?
    @FocusState private var fieldFocused: Bool

    private enum Phase: Equatable {
        case idle
        case searching
        case results([VideoMatch])
        case failed(String)
    }

    private var trimmed: String { input.trimmingCharacters(in: .whitespacesAndNewlines) }
    /// The field holds a YouTube link (or an 11-character id) rather than a search.
    private var linkID: String? { trimmed.isEmpty ? nil : youtubeId(input: trimmed) }
    private var results: [VideoMatch] {
        if case let .results(r) = phase { return r }
        return []
    }
    private var selectedMatch: VideoMatch? { results.first { $0.candidate.youtubeId == selection } }
    private var canAttach: Bool { linkID != nil || selectedMatch != nil }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(heading).font(.headline)
            searchField
            content
                .frame(height: 340)
                .background(.background.secondary, in: RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator))
            footer
        }
        .padding(20)
        .frame(width: 560)
        .onAppear(perform: start)
        .onDisappear { searchTask?.cancel() }
    }

    // MARK: Search field

    private var searchField: some View {
        HStack(spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: linkID == nil ? "magnifyingglass" : "link")
                    .foregroundStyle(.secondary)
                    .frame(width: 16)
                TextField("Search YouTube or paste a link", text: $input)
                    .textFieldStyle(.plain)
                    .focused($fieldFocused)
                    .onSubmit(submit)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .background(.background, in: RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(.separator))
            Button("Search", action: search)
                .disabled(trimmed.isEmpty || linkID != nil || phase == .searching)
        }
    }

    // MARK: Results

    @ViewBuilder private var content: some View {
        if let id = linkID {
            ContentUnavailableView {
                Label("YouTube Video \(id)", systemImage: "checkmark.circle.fill")
                    .foregroundStyle(.green)
            } description: {
                Text("Attach this video, or clear the field to search.")
            }
        } else {
            switch phase {
            case .idle:
                ContentUnavailableView("Search for the Song", systemImage: "magnifyingglass",
                                       description: Text("Type an artist and title, or paste a youtube.com or youtu.be link."))
            case .searching:
                ProgressView("Searching YouTube…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            case let .failed(message):
                ContentUnavailableView {
                    Label("Couldn't Search YouTube", systemImage: "wifi.exclamationmark")
                } description: {
                    Text(message)
                } actions: {
                    Button("Try Again", action: search)
                }
            case let .results(matches) where matches.isEmpty:
                ContentUnavailableView("No Videos Found", systemImage: "questionmark.video",
                                       description: Text("Try fewer words, or paste a link."))
            case let .results(matches):
                List(matches, id: \.candidate.youtubeId, selection: $selection) { match in
                    VideoMatchRow(match: match, isBest: match == matches.first)
                        .tag(match.candidate.youtubeId)
                }
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
                .contextMenu(forSelectionType: String.self) { ids in
                    if let match = matches.first(where: { ids.contains($0.candidate.youtubeId) }) {
                        Button("Open on YouTube", systemImage: "safari") { open(match) }
                    }
                } primaryAction: { ids in
                    // Double-click attaches.
                    if let id = ids.first {
                        selection = id
                        attach()
                    }
                }
                .clipShape(RoundedRectangle(cornerRadius: 8))
            }
        }
    }

    // MARK: Footer

    private var footer: some View {
        HStack(spacing: 8) {
            Text("Label")
            TextField("Label", text: Binding(get: { label }, set: { label = $0; labelEdited = true }))
                .frame(width: 120)
                .labelsHidden()
            Menu {
                ForEach(Self.suggestedLabels, id: \.self) { s in
                    Button(s) { label = s; labelEdited = true }
                }
            } label: {
                Image(systemName: "chevron.down")
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            .accessibilityLabel("Suggested labels")
            if let match = selectedMatch {
                Button("Preview", systemImage: "safari") { open(match) }
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
                    .help("Open this video on YouTube")
            }
            Spacer()
            if let onSkip {
                Button("Skip", action: onSkip)
                    .keyboardShortcut(.cancelAction)
                    .help("Carry on without a video")
            } else {
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
            }
            Button(onSkip == nil ? "Attach" : "Attach & Continue", action: attach)
                .keyboardShortcut(.defaultAction)
                .disabled(!canAttach)
        }
        .onChange(of: selection) { _, _ in
            // Follow the chosen video's version ("Live", "Cover" …) until the label is edited.
            if !labelEdited, let match = selectedMatch { label = match.label }
        }
    }

    // MARK: Actions

    private func start() {
        // A YouTube link on the clipboard: offer it.
        if let s = NSPasteboard.general.string(forType: .string)?.trimmingCharacters(in: .whitespacesAndNewlines),
           s.count < 300, youtubeId(input: s) != nil {
            input = s
            return
        }
        let query = (try? core.videoQuery(tab: tabID)) ?? ""
        input = query
        fieldFocused = true
        // The background search's results, if it already ran for the same query.
        if let stored = try? core.videoSearch(tab: tabID), stored.state == .done,
           stored.query == query, !stored.results.isEmpty {
            searchedFor = query
            show(stored.results)
        } else if !query.isEmpty {
            search()
        }
    }

    /// Return: search for the text, or attach when it's a link or the results are for it already.
    private func submit() {
        if linkID != nil || (searchedFor == trimmed && selectedMatch != nil) {
            attach()
        } else {
            search()
        }
    }

    private func search() {
        guard linkID == nil, !trimmed.isEmpty else { return }
        let query = trimmed
        let core = core
        let tab = tabID
        searchTask?.cancel()
        phase = .searching
        searchTask = Task {
            let result = await Task.detached { Result { try core.searchVideos(tab: tab, query: query) } }.value
            guard !Task.isCancelled else { return }
            searchedFor = query
            switch result {
            case let .success(matches):
                show(matches)
            case let .failure(error):
                phase = .failed(error.userMessage)
            }
        }
    }

    private func show(_ matches: [VideoMatch]) {
        phase = .results(matches)
        selection = matches.first?.candidate.youtubeId
        if !labelEdited, let best = matches.first { label = best.label }
    }

    private func attach() {
        let chosenLabel = label.trimmingCharacters(in: .whitespacesAndNewlines)
        let finalLabel = chosenLabel.isEmpty ? "Original" : chosenLabel
        if let id = linkID {
            onAttachID(id, finalLabel)
        } else if let match = selectedMatch {
            onAttachMatch(match.candidate, finalLabel)
        } else {
            return
        }
        if onSkip == nil { dismiss() }
    }

    private func open(_ match: VideoMatch) {
        if let url = URL(string: "https://www.youtube.com/watch?v=\(match.candidate.youtubeId)") {
            NSWorkspace.shared.open(url)
        }
    }
}

/// One search result: thumbnail, title, channel, length against the tab's, and why it ranked
/// where it did.
struct VideoMatchRow: View {
    let match: VideoMatch
    let isBest: Bool

    private var video: VideoCandidate { match.candidate }

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            VideoThumbnail(youtubeID: video.youtubeId, duration: video.durationSecs)
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(video.title)
                        .font(.body.weight(.medium))
                        .lineLimit(2)
                    Spacer(minLength: 4)
                    ConfidenceBadge(confidence: match.confidence, isBest: isBest)
                }
                HStack(spacing: 4) {
                    Text(video.channel.isEmpty ? "Unknown channel" : video.channel)
                    if match.official {
                        Image(systemName: "checkmark.seal.fill")
                            .foregroundStyle(.secondary)
                            .help("The artist's own channel")
                            .accessibilityLabel("The artist's own channel")
                    }
                    if let views = video.views {
                        Text("· \(Self.formatViews(views))")
                    }
                }
                .font(.callout)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                if !match.reasons.isEmpty {
                    Text(match.reasons.joined(separator: " · "))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
            }
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
    }

    static func formatViews(_ views: UInt64) -> String {
        let n = Double(views)
        switch n {
        case 1_000_000_000...: return String(format: "%.1fB views", n / 1_000_000_000)
        case 1_000_000...: return String(format: "%.1fM views", n / 1_000_000)
        case 1_000...: return String(format: "%.0fK views", n / 1_000)
        default: return "\(views) view\(views == 1 ? "" : "s")"
        }
    }
}

/// A video's thumbnail with its length in the corner.
struct VideoThumbnail: View {
    let youtubeID: String
    let duration: Double?
    var width: CGFloat = 112

    var body: some View {
        AsyncImage(url: URL(string: "https://i.ytimg.com/vi/\(youtubeID)/mqdefault.jpg")) { phase in
            if let image = phase.image {
                image.resizable().aspectRatio(contentMode: .fill)
            } else {
                Rectangle().fill(.quaternary)
                    .overlay { Image(systemName: "play.rectangle").foregroundStyle(.tertiary) }
            }
        }
        .frame(width: width, height: width * 9 / 16)
        .clipShape(RoundedRectangle(cornerRadius: 5))
        .overlay(alignment: .bottomTrailing) {
            if let duration {
                Text(TabTable.formatLength(duration))
                    .font(.caption2.monospacedDigit().weight(.medium))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 4)
                    .padding(.vertical, 1)
                    .background(.black.opacity(0.75), in: RoundedRectangle(cornerRadius: 3))
                    .padding(4)
            }
        }
        .accessibilityHidden(true)
    }
}

/// "Best Match" / "Likely" capsule; nothing for unlikely results.
struct ConfidenceBadge: View {
    let confidence: MatchConfidence
    let isBest: Bool

    var body: some View {
        switch confidence {
        case .high:
            badge(isBest ? "Best Match" : "Good Match", color: .green)
        case .medium:
            badge("Likely", color: .orange)
        case .low:
            EmptyView()
        }
    }

    private func badge(_ text: String, color: Color) -> some View {
        Text(text)
            .font(.caption2.weight(.semibold))
            .foregroundStyle(color)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(color.opacity(0.15), in: Capsule())
            .fixedSize()
    }
}
