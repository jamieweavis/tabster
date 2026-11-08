import Foundation
import QuartzCore
import TabsterKit

/// Practice tools: saved loops, the speed trainer, count-in and practice-time history.
extension TabViewerModel {
    /// How often practice time is written while playing.
    static let practiceFlushInterval: Duration = .seconds(30)

    // MARK: - Saved loops

    func loadPracticeData() {
        let core = core
        let id = tabID
        Task {
            let (loops, stats) = await Task.detached {
                ((try? core.savedLoops(tab: id)) ?? [], try? core.practiceStats(tab: id))
            }.value
            savedLoops = loops
            practiceStats = stats
        }
    }

    private func reloadSavedLoops() {
        savedLoops = (try? core.savedLoops(tab: tabID)) ?? savedLoops
    }

    /// The saved loop matching the current loop, if any.
    var currentSavedLoop: SavedLoop? {
        guard let loop else { return nil }
        return savedLoops.first { $0.startBar == loop.lowerBound && $0.endBar == loop.upperBound }
    }

    /// A name for the current loop: the section it starts in ("Chorus"), else its bars.
    var suggestedLoopName: String {
        guard let loop else { return "" }
        if let section = section(atBar: loop.lowerBound), !section.label.isEmpty { return section.label }
        return TabViewer.loopLabel(loop)
    }

    func saveCurrentLoop(name: String) {
        guard let loop else { return }
        let trimmed = name.trimmingCharacters(in: .whitespaces)
        do {
            _ = try core.saveLoop(tab: tabID, name: trimmed.isEmpty ? suggestedLoopName : trimmed,
                                  startBar: loop.lowerBound, endBar: loop.upperBound)
        } catch {
            ViewerLog.log.error("Saving loop failed: \(error)")
        }
        reloadSavedLoops()
    }

    func select(_ saved: SavedLoop) {
        guard let info, saved.startBar <= saved.endBar, saved.endBar < info.barCount else { return }
        setLoop(saved.startBar...saved.endBar)
        if !isPlaying, let span = loopSpan { seek(to: span.lowerBound) }
    }

    func rename(_ saved: SavedLoop, to name: String) {
        let trimmed = name.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return }
        do { try core.renameLoop(id: saved.id, name: trimmed) } catch {
            ViewerLog.log.error("Renaming loop failed: \(error)")
        }
        reloadSavedLoops()
    }

    func delete(_ saved: SavedLoop) {
        do { try core.deleteLoop(id: saved.id) } catch {
            ViewerLog.log.error("Deleting loop failed: \(error)")
        }
        reloadSavedLoops()
    }

    // MARK: - Sections

    /// The section bar `bar` is in (the last one starting at or before it).
    func section(atBar bar: UInt32) -> SectionInfo? {
        info?.sections.filter { $0.masterBar <= bar }.max { $0.masterBar < $1.masterBar }
    }

    /// The master bars of the section `bar` is in (before the first section: the bars up to it).
    func sectionRange(containing bar: UInt32) -> ClosedRange<UInt32>? {
        guard let info, info.barCount > 0 else { return nil }
        let starts = Set(info.sections.map(\.masterBar)).sorted()
        let start = starts.last { $0 <= bar } ?? 0
        let next = starts.first { $0 > bar } ?? info.barCount
        return start...max(start, min(next, info.barCount) - 1)
    }

    /// Loop the section the cursor is in.
    func loopCurrentSection() {
        guard let bar = cursorBar ?? playedBars.first?.masterBar, let range = sectionRange(containing: bar) else { return }
        setLoop(range)
    }

    // MARK: - Speed trainer

    /// The clock's speeds the trainer has to snap to (nil: any).
    private var trainerRates: [Double]? { usingVideoClock ? Self.videoSpeeds : nil }

    /// Start the trainer on the current loop from its first speed, playing from the loop start.
    func startTrainer() {
        guard loop != nil else { return }
        let trainer = SpeedTrainer(settings: trainerSettings, rates: trainerRates)
        self.trainer = trainer
        applyTrainerSpeed(trainer.speed)
        updateLoopSpan(near: currentTime)
        if let span = loopSpan { seek(to: span.lowerBound) }
        if !isPlaying { play() }
        ViewerLog.log.info("Trainer started: \(trainer.schedule.map { TabViewer.percent($0) }.joined(separator: " → "), privacy: .public)")
    }

    func stopTrainer() {
        guard trainer != nil else { return }
        trainer = nil
        // Back on the menu's speeds.
        setSpeed(speed)
    }

    func trainerMissedPass() { trainer?.markMiss() }

    private func applyTrainerSpeed(_ value: Double) {
        speed = value
        clock.rate = value
        scheduleSave()
    }

    /// Playback wrapped the loop (`span`): count a trainer pass, step the speed up if earned,
    /// and count in again before the faster pass.
    func loopWrapped(_ span: Range<Double>) {
        guard var trainer else { return }
        let event = trainer.completePass()
        self.trainer = trainer
        switch event {
        case .counted:
            break
        case let .stepped(to: speed), let .reachedTarget(speed):
            applyTrainerSpeed(speed)
            if countIn, let synthClock, clock === synthClock {
                seekClock(toScore: span.lowerBound)
                synthClock.synth.countIn(bars: 1)
                lastTickScore = nil
            }
        }
        trainerObserver?(trainer, event)
    }

    /// The clock jumped from late in the loop back to its start without us seeking (the synth
    /// wraps its own loop).
    static func wrappedOnItsOwn(from last: Double, to t: Double, in span: Range<Double>) -> Bool {
        let middle = span.lowerBound + (span.upperBound - span.lowerBound) / 2
        return t < last - 0.05 && last >= middle && t < middle && t >= span.lowerBound - 0.05
    }

    // MARK: - Practice time

    /// Time the playing, writing it every ~30 s and when playback stops.
    func practicePlayingChanged() {
        let now = CACurrentMediaTime()
        practiceFlushTask?.cancel()
        if isPlaying {
            practiceTimer.start(at: now)
            practiceFlushTask = Task { [weak self] in
                while !Task.isCancelled {
                    try? await Task.sleep(for: Self.practiceFlushInterval)
                    guard !Task.isCancelled else { return }
                    self?.flushPractice()
                }
            }
        } else {
            practiceTimer.stop(at: now)
            flushPractice()
        }
    }

    /// Record the practice time not yet written. `synchronously` when closing.
    func flushPractice(synchronously: Bool = false) {
        if synchronously { practiceFlushTask?.cancel() }
        let secs = practiceTimer.take(at: CACurrentMediaTime())
        guard secs >= 0.5, session != nil else { return }
        let core = core
        let id = tabID
        let write: @Sendable () -> PracticeStats? = {
            do {
                try core.recordPractice(tab: id, seconds: secs)
                return try core.practiceStats(tab: id)
            } catch {
                ViewerLog.log.error("Recording practice failed: \(error)")
                return nil
            }
        }
        if synchronously {
            _ = write()
        } else {
            Task {
                if let stats = await Task.detached(operation: write).value { practiceStats = stats }
            }
        }
    }

    /// "Practised 1h 12m · 24m this week", once there's a minute of practice.
    var practiceSummary: String? {
        practiceStats.flatMap { PracticeFormat.summary(totalSecs: $0.totalSecs, weekSecs: $0.last7DaysSecs) }
    }
}
