import Foundation

/// The speed trainer: loop a section, and after `passesPerStep` clean passes at a speed, step the
/// speed up until the target. Pure logic; the viewer counts a pass each time playback wraps the
/// loop and applies `speed` to its clock.
public struct SpeedTrainer: Equatable, Sendable {
    /// Percentages, as the UI edits them.
    public struct Settings: Equatable, Sendable {
        public var startPercent: Int
        public var stepPercent: Int
        public var targetPercent: Int
        public var passesPerStep: Int

        public static let percentRange = 25...200
        public static let stepRange = 1...50
        public static let passesRange = 1...20

        public init(startPercent: Int = 60, stepPercent: Int = 5, targetPercent: Int = 100, passesPerStep: Int = 1) {
            self.startPercent = startPercent
            self.stepPercent = stepPercent
            self.targetPercent = targetPercent
            self.passesPerStep = passesPerStep
        }

        /// Every field clamped to its range, and the target no lower than the start.
        public var normalized: Settings {
            let start = startPercent.clamped(to: Self.percentRange)
            return Settings(
                startPercent: start,
                stepPercent: stepPercent.clamped(to: Self.stepRange),
                targetPercent: max(start, targetPercent.clamped(to: Self.percentRange)),
                passesPerStep: passesPerStep.clamped(to: Self.passesRange)
            )
        }

        /// "start,step,target,passes" in percent, e.g. "60,5,100,1" (the last field is optional).
        public init?(string: String) {
            let fields = string.split(separator: ",").map { Int($0.trimmingCharacters(in: .whitespaces)) }
            guard (3...4).contains(fields.count), fields.allSatisfy({ $0 != nil }) else { return nil }
            let values = fields.compactMap { $0 }
            self.init(startPercent: values[0], stepPercent: values[1], targetPercent: values[2],
                      passesPerStep: values.count > 3 ? values[3] : 1)
        }

        public var string: String { "\(startPercent),\(stepPercent),\(targetPercent),\(passesPerStep)" }
    }

    public enum Event: Equatable, Sendable {
        /// A pass was counted; the speed stays.
        case counted
        /// The speed stepped up (not yet to the target).
        case stepped(to: Double)
        /// The speed stepped up to the target.
        case reachedTarget(Double)
    }

    public let settings: Settings
    /// The speeds the trainer goes through, as multipliers (1 = 100 %).
    public private(set) var schedule: [Double]
    /// The clock's speeds (nil: any speed). Each step snaps to the nearest one.
    public private(set) var rates: [Double]?
    public private(set) var stepIndex = 0
    /// Passes completed at the current speed.
    public private(set) var passes = 0
    /// Passes completed in all.
    public private(set) var totalPasses = 0

    public init(settings: Settings, rates: [Double]? = nil) {
        self.settings = settings.normalized
        self.rates = rates
        schedule = Self.schedule(for: self.settings, rates: rates)
    }

    public var speed: Double { schedule[stepIndex] }
    public var atTarget: Bool { stepIndex == schedule.count - 1 }
    public var stepCount: Int { schedule.count }
    /// The rates forced the schedule off the exact percentages.
    public var isSnapped: Bool { schedule != Self.schedule(for: settings, rates: nil) }

    /// Playback wrapped the loop: count a clean pass, and step up after enough of them.
    public mutating func completePass() -> Event {
        totalPasses += 1
        passes += 1
        guard !atTarget, passes >= settings.passesPerStep else { return .counted }
        stepIndex += 1
        passes = 0
        return atTarget ? .reachedTarget(speed) : .stepped(to: speed)
    }

    /// The last pass wasn't clean: start counting again at this speed.
    public mutating func markMiss() { passes = 0 }

    /// The clock changed (e.g. to a video, which only plays its own rates). Keeps the trainer
    /// at the step nearest its current speed.
    public mutating func setRates(_ rates: [Double]?) {
        guard rates != self.rates else { return }
        let current = speed
        self.rates = rates
        schedule = Self.schedule(for: settings, rates: rates)
        stepIndex = schedule.firstIndex { $0 >= current - 1e-9 } ?? schedule.count - 1
        passes = 0
    }

    /// Start, start + step, … then the target; snapped to `rates` (consecutive duplicates
    /// dropped) when given.
    public static func schedule(for settings: Settings, rates: [Double]?) -> [Double] {
        let s = settings.normalized
        var percents = Array(stride(from: s.startPercent, to: s.targetPercent, by: s.stepPercent))
        percents.append(s.targetPercent)
        var speeds = percents.map { Double($0) / 100 }
        if let rates, !rates.isEmpty {
            speeds = speeds.map { speed in rates.min { abs($0 - speed) < abs($1 - speed) } ?? speed }
            speeds = speeds.reduce(into: []) { out, speed in
                if out.last.map({ abs($0 - speed) > 1e-9 }) ?? true { out.append(speed) }
            }
        }
        return speeds
    }
}

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

    /// "Practised 1h 12m · 24m this week", or nil before a minute of practice.
    public static func summary(totalSecs: Double, weekSecs: Double) -> String? {
        guard totalSecs >= 60 else { return nil }
        var text = "Practised \(duration(totalSecs))"
        if weekSecs >= 60, Int(weekSecs / 60) < Int(totalSecs / 60) {
            text += " · \(duration(weekSecs)) this week"
        } else if weekSecs >= 60 {
            text += ", all this week"
        }
        return text
    }
}

extension Comparable {
    fileprivate func clamped(to range: ClosedRange<Self>) -> Self {
        min(max(self, range.lowerBound), range.upperBound)
    }
}
