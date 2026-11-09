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
