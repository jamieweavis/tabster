import SwiftUI

/// Edits one tab's tags as comma-separated text.
struct TagsSheet: View {
    let tab: TabSummary
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var text: String

    init(tab: TabSummary) {
        self.tab = tab
        _text = State(initialValue: tab.tags.joined(separator: ", "))
    }

    private var tags: [String] {
        text.split(separator: ",")
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
    }

    /// Existing tags the tab doesn't have yet, offered as one-click additions.
    private var suggestions: [String] {
        let current = Set(tags.map { $0.lowercased() })
        return model.tags.filter { !current.contains($0.lowercased()) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Tags for “\(tab.title.isEmpty ? "Untitled" : tab.title)”")
                .font(.headline)
            TextField("Tags", text: $text, prompt: Text("rock, practice, drop d"))
                .textFieldStyle(.roundedBorder)
            Text("Separate tags with commas.")
                .font(.caption)
                .foregroundStyle(.secondary)
            if !suggestions.isEmpty {
                ScrollView(.horizontal) {
                    HStack(spacing: 6) {
                        ForEach(suggestions, id: \.self) { tag in
                            Button(tag, systemImage: "plus") {
                                text = (tags + [tag]).joined(separator: ", ")
                            }
                            .buttonStyle(.bordered)
                            .controlSize(.small)
                        }
                    }
                }
                .scrollIndicators(.never)
            }
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Save") {
                    model.setTags(tab.id, tags)
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 420)
    }
}
