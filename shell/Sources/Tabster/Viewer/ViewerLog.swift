import Foundation
import os

/// Logging and lightweight perf stats for the tab viewer, under the app's subsystem
/// (`just logs` streams it; signposts show up in Instruments' Points of Interest).
enum ViewerLog {
    static let log = Logger(subsystem: "com.jamieweavis.tabster", category: "viewer")
    static let signposter = OSSignposter(subsystem: "com.jamieweavis.tabster", category: .pointsOfInterest)
}

/// Rolling timing stats (milliseconds): count, mean, max.
struct TimingStats {
    private(set) var count = 0
    private(set) var total = 0.0
    private(set) var max = 0.0

    mutating func add(_ ms: Double) {
        count += 1
        total += ms
        max = Swift.max(max, ms)
    }

    var mean: Double { count == 0 ? 0 : total / Double(count) }

    var summary: String {
        String(format: "n=%d mean=%.3fms max=%.3fms", count, mean, max)
    }
}

/// Viewer perf counters, logged by the debug harness.
@MainActor
enum ViewerMetrics {
    static var draw = TimingStats()
    static var tick = TimingStats()
    static var frameInterval = TimingStats()
    static var layout = TimingStats()
    static var firstRenderMs: Double?
    static var openMs: Double?
    /// Background part of opening: `openTab` alone, and everything (incl. the first layout).
    static var openWork: (openTab: Double, total: Double)?
    static var firstLayoutMs: Double?
}
