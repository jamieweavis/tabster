import AppKit
import SwiftUI

/// One bar start on the sync ruler.
struct SyncRulerMark: Identifiable {
    let playedBar: Int
    let masterBar: UInt32
    let video: Double
    let isKeyframe: Bool
    var id: Int { playedBar }
}

/// A strip of the video timeline around the playhead with a marker at every bar start. Drag a
/// marker (when `canDragMarks`) to move that bar; drag anywhere else to shift the whole sync.
/// Dumb: the owner supplies the marks and applies the edits.
struct SyncRuler: View {
    /// The playhead's video time for a host time (nil: no video clock).
    let videoTime: (CFTimeInterval) -> Double?
    let marks: (ClosedRange<Double>) -> [SyncRulerMark]
    let isPlaying: Bool
    /// Bumped whenever the marks change, so a paused ruler redraws.
    let revision: Int
    var canDragMarks: Bool
    /// Move a bar to a video time; false if refused (it would cross a neighbouring keyframe).
    let moveMark: (Int, Double) -> Bool
    /// Shift everything by a delta (seconds).
    let shift: (Double) -> Void

    /// Seconds of video across the ruler's width.
    var span: Double = 8

    @State private var drag: DragState?
    /// The playhead time frozen while dragging, so the axis doesn't move under the pointer.
    @State private var frozenCenter: Double?

    private struct DragState {
        var mark: Int?
        var origin: Double
        var applied: Double
    }

    var body: some View {
        TimelineView(.animation(minimumInterval: nil, paused: !isPlaying || drag != nil)) { _ in
            let center = frozenCenter ?? videoTime(CACurrentMediaTime()) ?? 0
            Canvas { ctx, size in
                draw(in: &ctx, size: size, center: center)
            }
            .id(revision)  // repaint when the marks change while paused
            .contentShape(.rect)
            .gesture(dragGesture(center: center))
            .accessibilityElement()
            .accessibilityLabel("Sync ruler")
            .accessibilityValue(accessibilitySummary(center: center))
        }
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { rulerWidth = $0 }
        .frame(height: 40)
        .background(.quaternary.opacity(0.6), in: .rect(cornerRadius: 6))
        .clipShape(.rect(cornerRadius: 6))
        .help(canDragMarks ? "Drag a bar marker to move that bar; drag elsewhere to shift everything"
                           : "Drag to shift the sync")
    }

    private func pxPerSec(_ width: CGFloat) -> CGFloat { width / CGFloat(span) }

    private func range(center: Double) -> ClosedRange<Double> {
        (center - span / 2)...(center + span / 2)
    }

    private func draw(in ctx: inout GraphicsContext, size: CGSize, center: Double) {
        let scale = pxPerSec(size.width)
        let x = { (t: Double) in size.width / 2 + CGFloat(t - center) * scale }
        // Whole-second ticks along the bottom.
        let secondTick = GraphicsContext.Shading.color(.secondary.opacity(0.35))
        var t = (center - span / 2).rounded(.up)
        while t <= center + span / 2 {
            var p = Path()
            p.move(to: CGPoint(x: x(t), y: size.height - 5))
            p.addLine(to: CGPoint(x: x(t), y: size.height))
            ctx.stroke(p, with: secondTick, lineWidth: 1)
            t += 1
        }
        for mark in marks(range(center: center)) {
            let px = x(mark.video)
            let dragging = drag?.mark == mark.playedBar
            let color: Color = dragging ? .accentColor : mark.isKeyframe ? .accentColor : .primary.opacity(0.55)
            var line = Path()
            line.move(to: CGPoint(x: px, y: 15))
            line.addLine(to: CGPoint(x: px, y: size.height))
            ctx.stroke(line, with: .color(color), lineWidth: mark.isKeyframe || dragging ? 2 : 1)
            if mark.isKeyframe || dragging {
                let knob = Path(ellipseIn: CGRect(x: px - 3.5, y: 13, width: 7, height: 7))
                ctx.fill(knob, with: .color(.accentColor))
            }
            let label = Text("\(mark.masterBar + 1)")
                .font(.system(size: 9, weight: mark.isKeyframe ? .semibold : .regular).monospacedDigit())
                .foregroundStyle(mark.isKeyframe ? Color.accentColor : .secondary)
            ctx.draw(label, at: CGPoint(x: px + 3, y: 2), anchor: .topLeading)
        }
        // The playhead, in the cursor's colour.
        var head = Path()
        head.move(to: CGPoint(x: size.width / 2, y: 0))
        head.addLine(to: CGPoint(x: size.width / 2, y: size.height))
        ctx.stroke(head, with: .color(Color(nsColor: .systemOrange)), lineWidth: 2)
    }

    private func dragGesture(center: Double) -> some Gesture {
        DragGesture(minimumDistance: 2, coordinateSpace: .local)
            .onChanged { value in
                let width = rulerWidth
                let scale = pxPerSec(width)
                if drag == nil {
                    frozenCenter = center
                    let t = center + Double((value.startLocation.x - width / 2) / scale)
                    var hit: SyncRulerMark?
                    if canDragMarks {
                        let near = marks((t - 10 / Double(scale))...(t + 10 / Double(scale)))
                        hit = near.min { abs($0.video - t) < abs($1.video - t) }
                    }
                    drag = DragState(mark: hit?.playedBar, origin: hit?.video ?? 0, applied: 0)
                }
                guard var state = drag else { return }
                let delta = Double(value.translation.width / scale)
                if let mark = state.mark {
                    if moveMark(mark, state.origin + delta) { state.applied = delta }
                } else {
                    shift(delta - state.applied)
                    state.applied = delta
                }
                drag = state
            }
            .onEnded { _ in
                drag = nil
                frozenCenter = nil
            }
    }

    @State private var rulerWidth: CGFloat = 400

    private func accessibilitySummary(center: Double) -> String {
        let bars = marks(range(center: center)).map { "bar \($0.masterBar + 1)" }
        return bars.isEmpty ? "No bars nearby" : bars.joined(separator: ", ")
    }
}
