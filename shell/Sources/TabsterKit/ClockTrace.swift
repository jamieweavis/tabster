import Foundation

/// Records what a `VideoClock` displayed frame by frame and measures how smooth it was. Used by
/// the unit tests (replaying recorded sample streams) and by the live debug harness.
public struct ClockTrace: Sendable {
    public struct Frame: Sendable {
        public var host: Double
        public var time: Double
        public var running: Bool
        public var rate: Double
        /// Frames in different segments are separated by an explicit seek, pause or resume.
        public var segment: Int
    }

    public private(set) var frames: [Frame] = []
    private var segment = 0

    public init() {}

    /// The next frame follows an explicit jump (seek, pause, resume): don't compare across it.
    public mutating func markDiscontinuity() { segment += 1 }

    public mutating func record(host: Double, time: Double, running: Bool, rate: Double) {
        frames.append(Frame(host: host, time: time, running: running, rate: rate, segment: segment))
    }

    private func runningPairs(_ include: (Frame) -> Bool = { _ in true }) -> [(Frame, Frame)] {
        zip(frames, frames.dropFirst()).filter { a, b in
            a.running && b.running && a.segment == b.segment && include(a) && include(b)
        }
    }

    /// The largest step backwards (s) between consecutive running frames of one segment.
    public func maxBackwardStep(where include: (Frame) -> Bool = { _ in true }) -> Double {
        runningPairs(include).map { a, b in a.time - b.time }.max().map { max(0, $0) } ?? 0
    }

    /// The largest deviation (s) of a frame-to-frame step from `Δhost × rate`.
    public func maxFrameJitter(where include: (Frame) -> Bool = { _ in true }) -> Double {
        runningPairs(include).map { a, b in abs((b.time - a.time) - (b.host - a.host) * b.rate) }.max() ?? 0
    }
}

/// Least-squares line `y = intercept + slope × x`.
public struct LinearFit: Sendable {
    public var intercept: Double
    public var slope: Double

    public init?(_ points: [(x: Double, y: Double)]) {
        guard points.count >= 2 else { return nil }
        let n = Double(points.count)
        let mx = points.map(\.x).reduce(0, +) / n
        let my = points.map(\.y).reduce(0, +) / n
        var sxy = 0.0, sxx = 0.0
        for p in points {
            sxy += (p.x - mx) * (p.y - my)
            sxx += (p.x - mx) * (p.x - mx)
        }
        guard sxx > 0 else { return nil }
        slope = sxy / sxx
        intercept = my - slope * mx
    }

    public func callAsFunction(_ x: Double) -> Double { intercept + slope * x }
}
