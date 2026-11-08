import SwiftUI

/// Quick-sync controls for a tab's video: the current offset, ±10/±100 ms nudges, "Tap bar 1"
/// and a stretch slider. Dumb: it shows values and reports intent; the owner updates the
/// `SyncSpec` (e.g. `TabSession.setSync` + `core.setVideoSync`).
struct SyncNudgeBar: View {
    let sync: SyncSpec
    /// Called with the nudge in seconds (±0.01 / ±0.1).
    let onNudge: (Double) -> Void
    /// "Tap bar 1": the owner sets the offset so bar 1 starts at the current video time.
    let onTapBarOne: () -> Void
    /// Called while the stretch slider moves (0.90–1.10).
    let onStretch: (Double) -> Void

    static let stretchRange = 0.90...1.10

    @State private var stretchDraft: Double?

    private var offset: Double? {
        if case .offset(let offsetSecs, _) = sync { offsetSecs } else { nil }
    }

    private var stretch: Double {
        if case .offset(_, let stretch) = sync { stretch } else { 1 }
    }

    private var modeLabel: String {
        switch sync {
        case .offset: "Offset"
        case .points(let points): "Per-bar points (\(points.count))"
        }
    }

    var body: some View {
        HStack(spacing: 12) {
            Text(modeLabel)
                .font(.caption.weight(.semibold))
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(.quaternary, in: .capsule)

            Button("Tap Bar 1", systemImage: "hand.tap", action: onTapBarOne)
                .help("Set the offset so bar 1 starts now")

            if let offset {
                HStack(spacing: 2) {
                    nudge(-0.1, "−100")
                    nudge(-0.01, "−10")
                    Text(Self.format(offset))
                        .font(.body.monospacedDigit())
                        .frame(minWidth: 84)
                        .accessibilityLabel("Offset \(Self.format(offset))")
                    nudge(0.01, "+10")
                    nudge(0.1, "+100")
                }

                HStack(spacing: 6) {
                    Text("Stretch").foregroundStyle(.secondary)
                    Slider(value: Binding(get: { stretchDraft ?? stretch },
                                          set: { stretchDraft = $0; onStretch($0) }),
                           in: Self.stretchRange) { editing in
                        if !editing { stretchDraft = nil }
                    }
                    .frame(width: 120)
                    Text(String(format: "%.3f×", stretchDraft ?? stretch))
                        .font(.caption.monospacedDigit())
                        .frame(minWidth: 48, alignment: .leading)
                }
                .onTapGesture(count: 2) { onStretch(1) }
                .help("How much faster or slower the recording runs than the tab (double-click to reset)")
            }
        }
        .controlSize(.small)
    }

    private func nudge(_ delta: Double, _ title: String) -> some View {
        Button(title) { onNudge(delta) }
            .help("\(title) ms")
            .accessibilityLabel("\(title) milliseconds")
    }

    /// "+1.234 s".
    static func format(_ seconds: Double) -> String {
        String(format: "%+.3f s", seconds)
    }
}
