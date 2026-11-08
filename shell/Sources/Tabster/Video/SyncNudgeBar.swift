import SwiftUI

/// Sync controls for a tab's video. Dumb: it shows values and reports intent; the owner updates
/// the `SyncSpec` (`TabSession.setSync` + `core.setVideoSync`).
///
/// Row 1: mode (offset / per-bar), "Tap Bar 1", "Tap", import menu. Row 2: ±10/±100 ms nudges
/// with the offset (offset mode) or a shift of every point (per-bar mode), and the stretch slider
/// (offset mode).
struct SyncNudgeBar: View {
    struct Actions {
        /// Called with the nudge in seconds (±0.01 / ±0.1).
        var nudge: (Double) -> Void
        /// Bar 1 starts now.
        var tapBarOne: () -> Void
        /// The bar boundary nearest the cursor starts now.
        var tap: () -> Void
        /// Called while the stretch slider moves (0.90–1.10).
        var stretch: (Double) -> Void
        var setPerBar: (Bool) -> Void
        /// nil: the tab file has no sync points.
        var importFromFile: (() -> Void)?
        var pasteBarPoints: () -> Void
        var reset: () -> Void
    }

    let sync: SyncSpec
    /// Whether taps can work (a video is the clock).
    var canTap = true
    /// Feedback for the last tap, e.g. "Bar 12 → 0:23.456".
    var tapFeedback: String?
    let actions: Actions

    static let stretchRange = 0.90...1.10

    @State private var stretchDraft: Double?

    private var offset: Double? {
        if case .offset(let offsetSecs, _) = sync { offsetSecs } else { nil }
    }

    private var stretch: Double {
        if case .offset(_, let stretch) = sync { stretch } else { 1 }
    }

    private var isPerBar: Bool {
        if case .points = sync { true } else { false }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Picker("Sync mode", selection: Binding(get: { isPerBar }, set: { actions.setPerBar($0) })) {
                    Text("Offset").tag(false)
                    Text("Per Bar").tag(true)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                .help("Offset: one offset (and stretch) for the whole video. Per bar: a video time for every bar.")

                Spacer(minLength: 4)

                Button("Bar 1", systemImage: "1.circle", action: actions.tapBarOne)
                    .help("Bar 1 starts now (⇧T)")
                    .disabled(!canTap)
                Button(isPerBar ? "Tap Along" : "Tap", systemImage: "hand.tap", action: actions.tap)
                    .help(isPerBar
                          ? "Press T on each downbeat while the video plays: the nearest bar starts there"
                          : "The bar nearest the cursor starts now (T)")
                    .disabled(!canTap)

                Menu {
                    if let importFromFile = actions.importFromFile {
                        Button("From Tab File", systemImage: "doc", action: importFromFile)
                    }
                    Button("Paste Per-Bar Points…", systemImage: "doc.on.clipboard", action: actions.pasteBarPoints)
                    Divider()
                    Button("Reset Sync", systemImage: "arrow.counterclockwise", role: .destructive, action: actions.reset)
                } label: {
                    Label("Import", systemImage: "square.and.arrow.down")
                        .labelStyle(.iconOnly)
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
                .help("Import or reset sync")
            }

            HStack(spacing: 8) {
                HStack(spacing: 2) {
                    nudge(-0.1, "−100")
                    nudge(-0.01, "−10")
                    Group {
                        if let offset {
                            Text(Self.format(offset))
                                .accessibilityLabel("Offset \(Self.format(offset))")
                        } else {
                            Text("Shift all")
                                .foregroundStyle(.secondary)
                        }
                    }
                    .font(.callout.monospacedDigit())
                    .frame(minWidth: 76)
                    nudge(0.01, "+10")
                    nudge(0.1, "+100")
                }
                .help(offset == nil ? "Shift every bar (ms)" : "Nudge the offset (ms)")

                Spacer(minLength: 4)

                if offset != nil {
                    HStack(spacing: 6) {
                        Text("Stretch").foregroundStyle(.secondary)
                        Slider(value: Binding(get: { stretchDraft ?? stretch },
                                              set: { stretchDraft = $0; actions.stretch($0) }),
                               in: Self.stretchRange) { editing in
                            if !editing { stretchDraft = nil }
                        }
                        .frame(minWidth: 70, maxWidth: 120)
                        Text(String(format: "%.3f×", stretchDraft ?? stretch))
                            .font(.caption.monospacedDigit())
                            .frame(minWidth: 44, alignment: .leading)
                    }
                    .onTapGesture(count: 2) { actions.stretch(1) }
                    .help("How much slower (>1) or faster (<1) the recording runs than the tab (double-click to reset)")
                } else if let tapFeedback {
                    Text(tapFeedback)
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(.secondary)
                }
            }
        }
        .controlSize(.small)
    }

    private func nudge(_ delta: Double, _ title: String) -> some View {
        Button(title) { actions.nudge(delta) }
            .font(.caption.monospacedDigit())
            .accessibilityLabel("\(title) milliseconds")
    }

    /// "+1.234 s".
    static func format(_ seconds: Double) -> String {
        String(format: "%+.3f s", seconds)
    }
}
