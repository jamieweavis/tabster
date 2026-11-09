import TabsterKit
import SwiftUI

/// A name prompt for saving or renaming a loop.
enum LoopPrompt: Identifiable {
    case save
    case rename(SavedLoop)

    var id: Int64 {
        switch self {
        case .save: -1
        case let .rename(loop): loop.id
        }
    }
}

/// The player bar's loop control: the icon toggles looping (on loops the current section), the
/// chevron opens the loop options — saved loops (choose one to loop it), save, rename, delete.
struct LoopButton: View {
    let model: TabViewerModel
    @Binding var prompt: LoopPrompt?

    var body: some View {
        let looping = model.loop != nil
        SplitToggleButton(
            title: "Loop",
            systemImage: "repeat",
            isOn: Binding(get: { model.loop != nil }, set: { model.setLooping($0) }),
            showOptions: Binding(get: { model.showLoopOptions }, set: { model.showLoopOptions = $0 }),
            help: looping ? "Looping — click (or press \\) to clear" : "Loop the current section",
            optionsHelp: "Saved loops"
        ) {
            LoopPanel(model: model, prompt: $prompt)
        }
    }
}

struct LoopPanel: View {
    let model: TabViewerModel
    @Binding var prompt: LoopPrompt?

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            SoundPanel.header("Loop", isOn: Binding(get: { model.loop != nil }, set: { model.setLooping($0) }))

            if let loop = model.loop {
                Text(TabViewer.loopLabel(loop))
                    .font(.subheadline)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }

            VStack(alignment: .leading, spacing: 4) {
                Text("Saved Loops").font(.subheadline)
                if model.savedLoops.isEmpty {
                    Text("No saved loops for this tab.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                } else {
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(model.savedLoops, id: \.id) { saved in
                            row(saved)
                        }
                    }
                }
            }

            Button("Save Current Loop…", systemImage: "plus") { ask(.save) }
                .disabled(model.loop == nil)
            Button("Loop Section", systemImage: "repeat") { model.loopCurrentSection() }
                .disabled(model.info?.sections.isEmpty ?? true)
        }
    }

    private func row(_ saved: SavedLoop) -> some View {
        let selected = model.currentSavedLoop?.id == saved.id
        return HStack(spacing: 6) {
            Button {
                model.select(saved)
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "checkmark")
                        .font(.system(size: 11, weight: .semibold))
                        .opacity(selected ? 1 : 0)
                    Text(saved.name)
                        .lineLimit(1)
                        .truncationMode(.tail)
                    Spacer(minLength: 0)
                    Text(TabViewer.loopLabel(saved.startBar...saved.endBar))
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                }
                .contentShape(Rectangle())
            }
            .help("Loop this section")

            Button {
                ask(.rename(saved))
            } label: {
                Image(systemName: "pencil").foregroundStyle(.secondary)
            }
            .help("Rename this loop")

            Button {
                model.delete(saved)
            } label: {
                Image(systemName: "trash").foregroundStyle(.secondary)
            }
            .help("Delete this loop")
        }
        .buttonStyle(.plain)
        .padding(.vertical, 3)
    }

    /// The name prompt is an alert on the window, so close the popover first.
    private func ask(_ p: LoopPrompt) {
        model.showLoopOptions = false
        prompt = p
    }
}

/// Alert for naming a loop.
struct LoopPromptModifier: ViewModifier {
    let model: TabViewerModel
    @Binding var prompt: LoopPrompt?
    @State private var name = ""

    func body(content: Content) -> some View {
        content
            .alert(title, isPresented: Binding(get: { prompt != nil }, set: { if !$0 { prompt = nil } })) {
                TextField("Name", text: $name)
                Button(isRename ? "Rename" : "Save") { commit() }
                    .keyboardShortcut(.defaultAction)
                Button("Cancel", role: .cancel) { prompt = nil }
            } message: {
                if let loop = model.loop, !isRename {
                    Text(TabViewer.loopLabel(loop))
                }
            }
            .onChange(of: prompt?.id) {
                switch prompt {
                case .save: name = model.suggestedLoopName
                case let .rename(saved): name = saved.name
                case nil: break
                }
            }
    }

    private var isRename: Bool {
        if case .rename = prompt { return true }
        return false
    }

    private var title: String { isRename ? "Rename Loop" : "Save Loop" }

    private func commit() {
        switch prompt {
        case .save: model.saveCurrentLoop(name: name)
        case let .rename(saved): model.rename(saved, to: name)
        case nil: break
        }
        prompt = nil
    }
}
