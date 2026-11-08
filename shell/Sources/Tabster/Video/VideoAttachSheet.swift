import SwiftUI

/// Attach a YouTube video to a tab: paste a URL or id, pick a label. Validation uses the core's
/// `youtubeId(input:)`; the caller stores the result (e.g. `core.addVideo`).
struct VideoAttachSheet: View {
    /// Called with the 11-character video id and the label.
    let onAttach: (_ youtubeID: String, _ label: String) -> Void
    var onCancel: () -> Void = {}
    /// Parses a URL or id; injectable for previews.
    var parse: (String) -> String? = { youtubeId(input: $0) }

    static let suggestedLabels = ["Original", "Live", "Cover", "Lesson"]

    @Environment(\.dismiss) private var dismiss
    @State private var input = ""
    @State private var label = "Original"

    private var parsedID: String? {
        let trimmed = input.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : parse(trimmed)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Attach YouTube Video").font(.headline)
            Form {
                TextField("URL or ID", text: $input, prompt: Text("https://youtu.be/…"))
                    .onSubmit(attach)
                HStack {
                    TextField("Label", text: $label)
                    Menu {
                        ForEach(Self.suggestedLabels, id: \.self) { s in
                            Button(s) { label = s }
                        }
                    } label: {
                        Image(systemName: "chevron.down")
                    }
                    .menuStyle(.borderlessButton)
                    .menuIndicator(.hidden)
                    .fixedSize()
                    .accessibilityLabel("Suggested labels")
                }
            }
            .formStyle(.columns)
            status
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) {
                    onCancel()
                    dismiss()
                }
                .keyboardShortcut(.cancelAction)
                Button("Attach", action: attach)
                    .keyboardShortcut(.defaultAction)
                    .disabled(parsedID == nil)
            }
        }
        .padding(20)
        .frame(width: 420)
        .onAppear(perform: prefillFromPasteboard)
    }

    @ViewBuilder private var status: some View {
        if let id = parsedID {
            Label("Video \(id)", systemImage: "checkmark.circle.fill")
                .foregroundStyle(.green)
        } else if !input.trimmingCharacters(in: .whitespaces).isEmpty {
            Label("Not a YouTube URL or video ID", systemImage: "xmark.circle.fill")
                .foregroundStyle(.red)
        } else {
            Label("Paste a youtube.com or youtu.be link, or an 11-character ID", systemImage: "link")
                .foregroundStyle(.secondary)
        }
    }

    private func attach() {
        guard let id = parsedID else { return }
        let trimmedLabel = label.trimmingCharacters(in: .whitespacesAndNewlines)
        onAttach(id, trimmedLabel.isEmpty ? "Original" : trimmedLabel)
        dismiss()
    }

    /// Start with the clipboard's contents if they're a YouTube link.
    private func prefillFromPasteboard() {
        guard input.isEmpty, let s = NSPasteboard.general.string(forType: .string),
              s.count < 300, parse(s.trimmingCharacters(in: .whitespacesAndNewlines)) != nil
        else { return }
        input = s.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

/// Menu content listing a tab's videos. Use inside `Menu { VideoPicker(...) } label: { ... }`.
struct VideoPicker: View {
    let videos: [Video]
    /// The video currently shown, if any.
    var selectedID: Int64?
    let onSelect: (Video) -> Void
    var onRemove: ((Video) -> Void)?
    var onSetDefault: ((Video) -> Void)?
    var onAdd: (() -> Void)?

    var body: some View {
        if videos.isEmpty {
            Text("No videos")
        }
        ForEach(videos, id: \.id) { video in
            Toggle(isOn: Binding(get: { video.id == selectedID }, set: { _ in onSelect(video) })) {
                Text(title(video))
                if video.isDefault { Text("Default") }
            }
        }
        if let onAdd {
            Divider()
            Button("Attach Video…", systemImage: "plus", action: onAdd)
        }
        if !videos.isEmpty, onSetDefault != nil || onRemove != nil {
            Divider()
        }
        if let onSetDefault, videos.count > 1 {
            Menu("Make Default") {
                ForEach(videos, id: \.id) { video in
                    Button(title(video)) { onSetDefault(video) }.disabled(video.isDefault)
                }
            }
        }
        if let onRemove, !videos.isEmpty {
            Menu("Remove") {
                ForEach(videos, id: \.id) { video in
                    Button(title(video), role: .destructive) { onRemove(video) }
                }
            }
        }
    }

    private func title(_ v: Video) -> String {
        v.label.isEmpty ? v.youtubeId : "\(v.label) (\(v.youtubeId))"
    }
}
