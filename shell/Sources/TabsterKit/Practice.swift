import Foundation

/// Accumulates time spent playing, handed out in pieces (to record every ~30 s and on pause).
public struct PracticeTimer: Equatable, Sendable {
    private var runningSince: Double?
    private var banked: Double = 0

    public init() {}

    public var isRunning: Bool { runningSince != nil }

    public mutating func start(at now: Double) {
        guard runningSince == nil else { return }
        runningSince = now
    }

    public mutating func stop(at now: Double) {
        guard let since = runningSince else { return }
        banked += max(0, now - since)
        runningSince = nil
    }

    /// Seconds played since the last `take`, resetting the count (keeps running if it was).
    public mutating func take(at now: Double) -> Double {
        if let since = runningSince {
            banked += max(0, now - since)
            runningSince = now
        }
        defer { banked = 0 }
        return banked
    }
}

public enum PracticeFormat {
    /// "1h 12m", "24m", "2h"; under a minute is "<1m".
    public static func duration(_ secs: Double) -> String {
        let minutes = Int((max(0, secs) / 60).rounded(.down))
        if minutes == 0 { return "<1m" }
        let (h, m) = (minutes / 60, minutes % 60)
        if h == 0 { return "\(m)m" }
        return m == 0 ? "\(h)h" : "\(h)h \(m)m"
    }
}
