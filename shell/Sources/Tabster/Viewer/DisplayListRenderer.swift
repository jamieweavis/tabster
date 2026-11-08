import AppKit
import CoreText

/// Paints `DisplayList` primitives with Core Graphics, with the same semantics as the Rust
/// reference renderer (`src/layout/svg.rs`): butt-capped strokes, quadratic curves, filled
/// rects/dots, knockouts in the background colour, and text in a monospaced font (the layout
/// estimates text width as 0.6 × size, which matches SF Mono's advance).
///
/// Text is shaped once per system into `CTLine`s that take their colour from the context, so a
/// prepared system is appearance-independent and redraws are just fills, strokes and glyph blits.
@MainActor
final class DisplayListRenderer {
    /// Resolved colours for one draw pass (resolve inside the view's drawing appearance).
    struct Palette {
        var primary: CGColor
        var secondary: CGColor
        var accent: CGColor
        var background: CGColor

        static func current() -> Palette {
            Palette(
                primary: NSColor.labelColor.cgColor,
                secondary: NSColor.secondaryLabelColor.cgColor,
                accent: NSColor.systemOrange.cgColor,
                background: NSColor.windowBackgroundColor.cgColor
            )
        }

        func color(_ ink: Ink) -> CGColor {
            switch ink {
            case .primary: primary
            case .secondary: secondary
            case .accent: accent
            }
        }
    }

    private enum Op {
        case line(CGPoint, CGPoint, width: CGFloat, Ink)
        case rect(CGRect, Ink)
        case knockout(CGRect)
        /// `origin` is the baseline start, already adjusted for the anchor.
        case text(CTLine, origin: CGPoint, Ink)
        case curve(CGPoint, control: CGPoint, CGPoint, width: CGFloat, Ink)
        case dot(CGRect, Ink)
    }

    private struct FontKey: Hashable {
        let size: Float
        let bold: Bool
        let italic: Bool
    }

    private(set) var list: DisplayList
    private var prepared: [[Op]?]
    private var fonts: [FontKey: CTFont] = [:]

    init(list: DisplayList) {
        self.list = list
        prepared = Array(repeating: nil, count: list.systems.count)
    }

    /// Indices of systems whose frame intersects `rect` (systems are ordered top to bottom).
    func systems(intersecting rect: CGRect) -> Range<Int> {
        let systems = list.systems
        var lo = 0
        var hi = systems.count
        // First system whose bottom is below rect.minY.
        while lo < hi {
            let mid = (lo + hi) / 2
            let f = systems[mid].frame
            if CGFloat(f.y + f.height) < rect.minY { lo = mid + 1 } else { hi = mid }
        }
        var end = lo
        while end < systems.count, CGFloat(systems[end].frame.y) <= rect.maxY { end += 1 }
        return lo..<end
    }

    func draw(system index: Int, in ctx: CGContext, palette: Palette) {
        let ops = prepared[index] ?? prepare(index)
        for op in ops {
            switch op {
            case let .line(a, b, width, ink):
                ctx.setStrokeColor(palette.color(ink))
                ctx.setLineWidth(width)
                ctx.beginPath()
                ctx.move(to: a)
                ctx.addLine(to: b)
                ctx.strokePath()
            case let .rect(r, ink):
                ctx.setFillColor(palette.color(ink))
                ctx.fill(r)
            case let .knockout(r):
                ctx.setFillColor(palette.background)
                ctx.fill(r)
            case let .text(line, origin, ink):
                ctx.setFillColor(palette.color(ink))
                ctx.textPosition = origin
                CTLineDraw(line, ctx)
            case let .curve(a, c, b, width, ink):
                ctx.setStrokeColor(palette.color(ink))
                ctx.setLineWidth(width)
                ctx.beginPath()
                ctx.move(to: a)
                ctx.addQuadCurve(to: b, control: c)
                ctx.strokePath()
            case let .dot(r, ink):
                ctx.setFillColor(palette.color(ink))
                ctx.fillEllipse(in: r)
            }
        }
    }

    /// Prepare every system up front (off the critical path, e.g. right after the first draw).
    func prepareAll() {
        for i in prepared.indices where prepared[i] == nil { _ = prepare(i) }
    }

    private func prepare(_ index: Int) -> [Op] {
        let primitives = list.systems[index].primitives
        var ops: [Op] = []
        ops.reserveCapacity(primitives.count)
        for p in primitives {
            switch p {
            case let .line(x1, y1, x2, y2, width, ink):
                ops.append(.line(CGPoint(x: CGFloat(x1), y: CGFloat(y1)), CGPoint(x: CGFloat(x2), y: CGFloat(y2)),
                                 width: CGFloat(width), ink))
            case let .rect(r, ink):
                ops.append(.rect(r.cg, ink))
            case let .knockout(r):
                ops.append(.knockout(r.cg))
            case let .text(x, y, text, size, bold, italic, anchor, ink):
                let font = font(size: size, bold: bold, italic: italic)
                let attributed = NSAttributedString(string: text, attributes: [
                    NSAttributedString.Key(kCTFontAttributeName as String): font,
                    NSAttributedString.Key(kCTForegroundColorFromContextAttributeName as String): true,
                ])
                let line = CTLineCreateWithAttributedString(attributed)
                let origin: CGPoint
                switch anchor {
                case .leading:
                    origin = CGPoint(x: CGFloat(x), y: CGFloat(y))
                case .center:
                    // SVG `text-anchor=middle` + `dominant-baseline=central`: centre the advance
                    // horizontally and the cap height vertically (digits are cap-height glyphs).
                    let width = CTLineGetTypographicBounds(line, nil, nil, nil)
                    origin = CGPoint(x: CGFloat(x) - width / 2, y: CGFloat(y) + CTFontGetCapHeight(font) / 2)
                }
                ops.append(.text(line, origin: origin, ink))
            case let .curve(x1, y1, cx, cy, x2, y2, width, ink):
                ops.append(.curve(CGPoint(x: CGFloat(x1), y: CGFloat(y1)), control: CGPoint(x: CGFloat(cx), y: CGFloat(cy)),
                                  CGPoint(x: CGFloat(x2), y: CGFloat(y2)), width: CGFloat(width), ink))
            case let .dot(x, y, radius, ink):
                let r = CGFloat(radius)
                ops.append(.dot(CGRect(x: CGFloat(x) - r, y: CGFloat(y) - r, width: 2 * r, height: 2 * r), ink))
            }
        }
        prepared[index] = ops
        return ops
    }

    private func font(size: Float, bold: Bool, italic: Bool) -> CTFont {
        let key = FontKey(size: size, bold: bold, italic: italic)
        if let f = fonts[key] { return f }
        var font = NSFont.monospacedSystemFont(ofSize: CGFloat(size), weight: bold ? .bold : .regular)
        if italic {
            let descriptor = font.fontDescriptor.withSymbolicTraits(.italic)
            font = NSFont(descriptor: descriptor, size: CGFloat(size)) ?? font
        }
        fonts[key] = font
        return font
    }
}

extension Rect {
    var cg: CGRect { CGRect(x: CGFloat(x), y: CGFloat(y), width: CGFloat(width), height: CGFloat(height)) }
}
