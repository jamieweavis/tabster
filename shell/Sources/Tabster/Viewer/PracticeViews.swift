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

/// The toolbar's Loops menu: saved loops (choose one to loop it), save, rename, delete.
struct LoopsMenu: View {
    let model: TabViewerModel
    @Binding var prompt: LoopPrompt?

    var body: some View {
        Menu {
            if model.savedLoops.isEmpty {
                Text("No Saved Loops")
            } else {
                Section("Saved Loops") {
                    ForEach(model.savedLoops, id: \.id) { saved in
                        Toggle(isOn: Binding(get: { model.currentSavedLoop?.id == saved.id },
                                             set: { _ in model.select(saved) })) {
                            Text(saved.name)
                            Text(TabViewer.loopLabel(saved.startBar...saved.endBar))
                        }
                    }
                }
            }
            Divider()
            Button("Save Current Loop…", systemImage: "plus") { prompt = .save }
                .disabled(model.loop == nil)
            Button("Loop Section", systemImage: "repeat") { model.loopCurrentSection() }
                .disabled(model.info?.sections.isEmpty ?? true)
            if !model.savedLoops.isEmpty {
                Menu("Rename", systemImage: "pencil") {
                    ForEach(model.savedLoops, id: \.id) { saved in
                        Button(saved.name) { prompt = .rename(saved) }
                    }
                }
                Menu("Delete", systemImage: "trash") {
                    ForEach(model.savedLoops, id: \.id) { saved in
                        Button(saved.name, role: .destructive) { model.delete(saved) }
                    }
                }
            }
        } label: {
            Label("Loops", systemImage: "bookmark")
        }
        .help("Saved loops")
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

/// The toolbar button for the practice popover: the trainer's speed and pass while it runs.
struct PracticeButton: View {
    let model: TabViewerModel

    var body: some View {
        Button {
            model.showPractice.toggle()
        } label: {
            if let trainer = model.trainer {
                Label {
                    Text(trainer.atTarget ? "\(TabViewer.percent(trainer.speed)) ✓"
                         : "\(TabViewer.percent(trainer.speed)) · \(trainer.passes + 1)/\(trainer.settings.passesPerStep)")
                } icon: {
                    Image(systemName: "gauge.with.dots.needle.67percent")
                }
                .labelStyle(.titleAndIcon)
                .monospacedDigit()
            } else {
                Label("Practice", systemImage: "gauge.with.dots.needle.33percent")
            }
        }
        .tint(model.trainer != nil ? .orange : nil)
        .help("Speed trainer and count-in")
        .popover(isPresented: Binding(get: { model.showPractice }, set: { model.showPractice = $0 }),
                 arrowEdge: .bottom) {
            PracticePanel(model: model)
        }
    }
}

/// Speed trainer settings and progress, count-in, and practice time.
struct PracticePanel: View {
    let model: TabViewerModel

    private var synthIsClock: Bool { model.synthAvailable && model.tabAudio && !model.usingVideoClock }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .firstTextBaseline) {
                Text("Speed Trainer").font(.headline)
                Spacer()
                if let loop = model.loop {
                    Text(model.currentSavedLoop?.name ?? TabViewer.loopLabel(loop))
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }

            if let trainer = model.trainer {
                running(trainer)
            } else {
                settings
            }

            if model.usingVideoClock {
                Label("With a video, the trainer steps through YouTube's speeds (50, 75, 100, 125 %).",
                      systemImage: "info.circle")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Divider()

            Toggle(isOn: Binding(get: { model.countIn }, set: { model.countIn = $0 })) {
                Text("Count in one bar")
                Text(synthIsClock ? "Before playing, and before each faster pass"
                                  : "Needs tab audio (no video)")
            }
            .toggleStyle(.switch)
            .controlSize(.small)
            .disabled(!synthIsClock)

            if let summary = model.practiceSummary {
                Divider()
                Label(summary, systemImage: "clock")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(16)
        .frame(width: 300)
    }

    @ViewBuilder
    private var settings: some View {
        let binding = Binding(get: { model.trainerSettings }, set: { model.trainerSettings = $0.normalized })
        Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 8) {
            row("Start", value: binding.startPercent, range: SpeedTrainer.Settings.percentRange, step: 5, unit: "%")
            row("Step", value: binding.stepPercent, range: SpeedTrainer.Settings.stepRange, step: 1, unit: "%")
            row("Target", value: binding.targetPercent, range: SpeedTrainer.Settings.percentRange, step: 5, unit: "%")
            row("Passes per step", value: binding.passesPerStep, range: SpeedTrainer.Settings.passesRange, step: 1, unit: "")
        }
        HStack {
            if model.loop == nil {
                Text("Set a loop first")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            Button("Start", systemImage: "play.fill") { model.startTrainer() }
                .buttonStyle(.borderedProminent)
                .disabled(model.loop == nil)
        }
    }

    private func row(_ title: String, value: Binding<Int>, range: ClosedRange<Int>, step: Int, unit: String) -> some View {
        GridRow {
            Text(title).foregroundStyle(.secondary)
            Stepper(value: value, in: range, step: step) {
                Text("\(value.wrappedValue)\(unit)").monospacedDigit()
                    .frame(minWidth: 44, alignment: .trailing)
            }
            .gridColumnAlignment(.trailing)
        }
    }

    @ViewBuilder
    private func running(_ trainer: SpeedTrainer) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(TabViewer.percent(trainer.speed))
                .font(.system(size: 34, weight: .semibold, design: .rounded))
                .monospacedDigit()
                .contentTransition(.numericText())
            VStack(alignment: .leading, spacing: 2) {
                Text(trainer.atTarget ? "Target reached" : "Pass \(trainer.passes + 1) of \(trainer.settings.passesPerStep)")
                Text("Step \(trainer.stepIndex + 1) of \(trainer.stepCount) · \(trainer.totalPasses) passes")
                    .foregroundStyle(.secondary)
            }
            .font(.callout)
            .monospacedDigit()
        }
        ProgressView(value: Double(trainer.stepIndex), total: Double(max(1, trainer.stepCount - 1)))
            .tint(.orange)
        Text(trainer.schedule.map { TabViewer.percent($0) }.joined(separator: " → "))
            .font(.caption)
            .foregroundStyle(.secondary)
            .lineLimit(2)
        HStack {
            Button("Not Clean") { model.trainerMissedPass() }
                .help("Repeat this speed: start counting passes again")
                .disabled(trainer.atTarget)
            Spacer()
            Button("Stop", systemImage: "stop.fill") { model.stopTrainer() }
        }
    }
}
