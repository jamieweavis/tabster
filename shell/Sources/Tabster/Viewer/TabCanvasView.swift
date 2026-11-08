import AppKit
import SwiftUI

/// The scrolling tab: an `NSScrollView` whose document view paints the `DisplayList` with
/// dirty-rect culling (only systems intersecting the rect being drawn are painted), plus small
/// overlay views for the loop range, the playing bar and the cursor line. The overlays move by
/// frame changes only, so the cursor animates without redrawing the tab.
struct TabCanvasView: NSViewRepresentable {
    let model: TabViewerModel

    func makeNSView(context: Context) -> TabCanvasContainer {
        let view = TabCanvasContainer(model: model)
        model.sink = view
        return view
    }

    func updateNSView(_ view: TabCanvasContainer, context: Context) {}

    static func dismantleNSView(_ view: TabCanvasContainer, coordinator: ()) {
        view.stopDisplayLink()
    }
}

@MainActor
final class TabCanvasContainer: NSView, TabCanvasSink {
    let model: TabViewerModel
    let scrollView = NSScrollView()
    private let document: TabDocumentView
    private let loopLayer = FlippedView()
    private let barHighlight = TabOverlayView(color: { NSColor.systemOrange.withAlphaComponent(0.10) })
    private let cursorLine = TabOverlayView(color: { NSColor.systemOrange })
    private var loopViews: [TabOverlayView] = []
    private var loop: ClosedRange<UInt32>?
    private var displayLink: CADisplayLink?
    private var lastFrameTimestamp: CFTimeInterval = 0
    private var scrollTarget: CGFloat?
    private var cursor: Cursor?

    init(model: TabViewerModel) {
        self.model = model
        document = TabDocumentView(model: model)
        super.init(frame: .zero)

        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = false
        scrollView.autohidesScrollers = true
        scrollView.drawsBackground = true
        scrollView.backgroundColor = .windowBackgroundColor
        scrollView.documentView = document
        scrollView.autoresizingMask = [.width, .height]
        addSubview(scrollView)

        for overlay in [loopLayer, barHighlight, cursorLine] {
            document.addSubview(overlay)
        }

        barHighlight.isHidden = true
        cursorLine.isHidden = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func layout() {
        super.layout()
        scrollView.frame = bounds
        sizeDocument()
        model.viewportWidthChanged(Float(scrollView.contentSize.width))
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard let window else { return }
        DispatchQueue.main.async { [document] in
            window.makeFirstResponder(document)
        }
    }

    private func sizeDocument() {
        let clip = scrollView.contentSize
        let list = document.renderer?.list
        let width = max(CGFloat(list?.width ?? 0), clip.width)
        let height = max(CGFloat(list?.height ?? 0) + 24, clip.height)
        if document.frame.size != CGSize(width: width, height: height) {
            document.setFrameSize(CGSize(width: width, height: height))
        }
        loopLayer.frame = CGRect(origin: .zero, size: document.frame.size)
    }

    // MARK: - TabCanvasSink

    func show(_ list: DisplayList, anchorBar: UInt32?) {
        // Keep the bar at the top of the viewport (or the requested bar) in place across relayouts.
        let anchor = anchorBar ?? topVisibleBar()
        document.renderer = DisplayListRenderer(list: list)
        sizeDocument()
        showLoop(loop)
        if let anchor, let system = list.systems.first(where: { $0.bars.contains { $0.masterBar == anchor } }) {
            scroll(toY: CGFloat(system.frame.y) - 8, animated: false)
        }
    }

    func moveCursor(_ cursor: Cursor?, follow: Bool) {
        self.cursor = cursor
        guard let cursor else {
            barHighlight.isHidden = true
            cursorLine.isHidden = true
            return
        }
        barHighlight.isHidden = false
        cursorLine.isHidden = false
        let bar = cursor.barFrame.cg
        if barHighlight.frame != bar { barHighlight.frame = bar }
        cursorLine.frame = CGRect(x: CGFloat(cursor.x) - 1, y: CGFloat(cursor.y), width: 2, height: CGFloat(cursor.height))
        if follow { keepInView(cursor) }
    }

    func showLoop(_ bars: ClosedRange<UInt32>?) {
        loop = bars
        loopViews.forEach { $0.removeFromSuperview() }
        loopViews.removeAll()
        guard let bars, let list = document.renderer?.list else { return }
        for system in list.systems {
            let frames = system.bars.filter { bars.contains($0.masterBar) }.map(\.frame.cg)
            guard let first = frames.first else { continue }
            let union = frames.dropFirst().reduce(first) { $0.union($1) }
            let view = TabOverlayView(color: { NSColor.systemOrange.withAlphaComponent(0.13) })
            view.frame = union
            loopLayer.addSubview(view)
            loopViews.append(view)
        }
    }

    func playingChanged(_ playing: Bool) {
        playing ? startDisplayLink() : stopDisplayLink()
    }

    // MARK: - Scrolling

    private func topVisibleBar() -> UInt32? {
        guard let list = document.renderer?.list else { return nil }
        let top = scrollView.contentView.bounds.minY
        guard top > 1 else { return nil }
        return list.systems.first { CGFloat($0.frame.y + $0.frame.height) > top }?.bars.first?.masterBar
    }

    /// Keep the playing system in view: scroll when it enters the bottom third (while playing) or
    /// leaves the viewport, putting it near the top.
    private func keepInView(_ cursor: Cursor) {
        guard let list = document.renderer?.list, Int(cursor.system) < list.systems.count else { return }
        let system = list.systems[Int(cursor.system)].frame.cg
        let visible = scrollView.contentView.bounds
        let target = scrollTarget ?? visible.minY
        let shown = CGRect(x: visible.minX, y: target, width: visible.width, height: visible.height)
        let outside = system.minY < shown.minY || system.maxY > shown.maxY
        let inBottomThird = system.maxY > shown.minY + shown.height * 2 / 3
        guard outside || (model.isPlaying && inBottomThird) else { return }
        scroll(toY: system.minY - 8, animated: true)
    }

    private func scroll(toY y: CGFloat, animated: Bool) {
        let clip = scrollView.contentView
        let maxY = max(0, document.frame.height - clip.bounds.height)
        let target = min(max(0, y), maxY)
        guard abs(target - clip.bounds.minY) > 0.5 else { return }
        if animated {
            scrollTarget = target
            NSAnimationContext.runAnimationGroup { ctx in
                ctx.duration = 0.35
                ctx.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
                clip.animator().setBoundsOrigin(NSPoint(x: 0, y: target))
            } completionHandler: { [weak self] in
                MainActor.assumeIsolated {
                    if self?.scrollTarget == target { self?.scrollTarget = nil }
                }
            }
        } else {
            scrollTarget = nil
            clip.setBoundsOrigin(NSPoint(x: 0, y: target))
        }
        scrollView.reflectScrolledClipView(clip)
    }

    // MARK: - Display link

    private func startDisplayLink() {
        guard displayLink == nil else { return }
        let link = displayLink(target: self, selector: #selector(step(_:)))
        link.add(to: .main, forMode: .common)
        displayLink = link
        lastFrameTimestamp = 0
    }

    func stopDisplayLink() {
        displayLink?.invalidate()
        displayLink = nil
    }

    @objc private func step(_ link: CADisplayLink) {
        let start = CACurrentMediaTime()
        if lastFrameTimestamp > 0 {
            ViewerMetrics.frameInterval.add((link.timestamp - lastFrameTimestamp) * 1000)
        }
        lastFrameTimestamp = link.timestamp
        let cursor = model.frameTick(at: link.targetTimestamp)
        if cursor != self.cursor { moveCursor(cursor, follow: true) }
        ViewerMetrics.tick.add((CACurrentMediaTime() - start) * 1000)
    }

    // MARK: - Input (from the document view)

    func bar(at point: CGPoint) -> UInt32? {
        guard let renderer = document.renderer else { return nil }
        let list = renderer.list
        for i in renderer.systems(intersecting: CGRect(x: point.x, y: point.y, width: 1, height: 1)) {
            let system = list.systems[i]
            if let bar = system.bars.first(where: { point.x >= CGFloat($0.frame.x) && point.x < CGFloat($0.frame.x + $0.frame.width) }) {
                return bar.masterBar
            }
        }
        return nil
    }

    fileprivate func previewLoop(_ bars: ClosedRange<UInt32>) {
        showLoop(bars)
    }
}

/// The tab itself, in list coordinates (flipped). Paints only the systems that intersect the
/// rect being drawn; AppKit scrolls by copying, so a scroll step only draws the newly exposed strip.
///
/// Measured against one layer-backed view per system (virtualised): in this hierarchy AppKit
/// still asks every system view to draw each exposed strip, so that was ~3-4× slower per scroll
/// step for no memory win. See the viewer report in the Phase 2 notes.
@MainActor
final class TabDocumentView: NSView {
    private let model: TabViewerModel
    var renderer: DisplayListRenderer? {
        didSet {
            renderedOnce = false
            needsDisplay = true
        }
    }
    private var renderedOnce = false
    private var dragStartBar: UInt32?
    private var dragBars: ClosedRange<UInt32>?

    init(model: TabViewerModel) {
        self.model = model
        super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }
    override var isOpaque: Bool { true }
    override var acceptsFirstResponder: Bool { true }

    private var container: TabCanvasContainer? {
        var view = superview
        while let v = view {
            if let c = v as? TabCanvasContainer { return c }
            view = v.superview
        }
        return nil
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        let start = CACurrentMediaTime()
        let palette = DisplayListRenderer.Palette.current()
        ctx.setFillColor(palette.background)
        ctx.fill(dirtyRect)
        guard let renderer else { return }
        ctx.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
        ctx.setLineCap(.butt)
        for i in renderer.systems(intersecting: dirtyRect) {
            renderer.draw(system: i, in: ctx, palette: palette)
        }
        ViewerMetrics.draw.add((CACurrentMediaTime() - start) * 1000)
        if !renderedOnce {
            renderedOnce = true
            model.didFirstRender()
            // Shape the remaining systems' text now so scrolling never has to.
            DispatchQueue.main.async { [weak renderer] in renderer?.prepareAll() }
        }
    }

    // MARK: Mouse

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let point = convert(event.locationInWindow, from: nil)
        if event.modifierFlags.contains(.shift), let bar = container?.bar(at: point) {
            dragStartBar = bar
            dragBars = bar...bar
            container?.previewLoop(bar...bar)
        } else {
            model.seek(atX: Float(point.x), y: Float(point.y))
        }
    }

    override func mouseDragged(with event: NSEvent) {
        guard let start = dragStartBar else { return }
        autoscroll(with: event)
        let point = convert(event.locationInWindow, from: nil)
        guard let bar = container?.bar(at: point) else { return }
        let range = min(start, bar)...max(start, bar)
        if range != dragBars {
            dragBars = range
            container?.previewLoop(range)
        }
    }

    override func mouseUp(with event: NSEvent) {
        guard dragStartBar != nil else { return }
        dragStartBar = nil
        if let range = dragBars { model.setLoop(range) }
        dragBars = nil
    }

    // MARK: Keys

    override func keyDown(with event: NSEvent) {
        let modifiers = event.modifierFlags.intersection([.command, .control, .option])
        guard modifiers.isEmpty else { return super.keyDown(with: event) }
        switch event.specialKey {
        case .leftArrow?: model.previousBar(); return
        case .rightArrow?: model.nextBar(); return
        default: break
        }
        switch event.charactersIgnoringModifiers {
        case " ": model.togglePlay()
        case "[": model.setLoopStart()
        case "]": model.setLoopEnd()
        case "\\": model.setLoop(nil)
        default: super.keyDown(with: event)
        }
    }
}

/// A solid-colour, click-through overlay. Draws in `draw(_:)` (so window snapshots include it)
/// and only redraws when resized; moving it is a layer reposition.
final class TabOverlayView: NSView {
    private let color: () -> NSColor

    init(color: @escaping () -> NSColor) {
        self.color = color
        super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        color().setFill()
        bounds.intersection(dirtyRect).fill(using: .sourceOver)
    }
}

/// A plain container in list coordinates (y down).
final class FlippedView: NSView {
    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}
