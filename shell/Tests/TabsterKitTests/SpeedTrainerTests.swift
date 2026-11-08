import Testing

@testable import TabsterKit

struct SpeedTrainerTests {
    typealias Settings = SpeedTrainer.Settings

    @Test func exactSchedule() {
        #expect(SpeedTrainer.schedule(for: Settings(), rates: nil)
            == [0.6, 0.65, 0.7, 0.75, 0.8, 0.85, 0.9, 0.95, 1.0])
        // A step that overshoots ends on the target.
        #expect(SpeedTrainer.schedule(for: Settings(startPercent: 70, stepPercent: 20, targetPercent: 100), rates: nil)
            == [0.7, 0.9, 1.0])
        // Target at or below the start: just the start.
        #expect(SpeedTrainer.schedule(for: Settings(startPercent: 80, stepPercent: 5, targetPercent: 60), rates: nil)
            == [0.8])
    }

    @Test func snapsToVideoRates() {
        let youtube = [0.5, 0.75, 1, 1.25]
        let trainer = SpeedTrainer(settings: Settings(), rates: youtube)
        #expect(trainer.schedule == [0.5, 0.75, 1])
        #expect(trainer.isSnapped)
        #expect(!SpeedTrainer(settings: Settings()).isSnapped)
    }

    @Test func stepsAfterEachPass() {
        var trainer = SpeedTrainer(settings: Settings(startPercent: 60, stepPercent: 20, targetPercent: 100))
        #expect(trainer.speed == 0.6)
        #expect(trainer.completePass() == .stepped(to: 0.8))
        #expect(trainer.completePass() == .reachedTarget(1.0))
        #expect(trainer.atTarget)
        // At the target it keeps counting passes.
        #expect(trainer.completePass() == .counted)
        #expect(trainer.completePass() == .counted)
        #expect(trainer.speed == 1.0)
        #expect(trainer.totalPasses == 4)
        #expect(trainer.passes == 2)
    }

    @Test func needsNPassesPerStep() {
        var trainer = SpeedTrainer(settings: Settings(startPercent: 90, stepPercent: 5, targetPercent: 100, passesPerStep: 3))
        #expect(trainer.completePass() == .counted)
        #expect(trainer.completePass() == .counted)
        #expect(trainer.passes == 2)
        trainer.markMiss()
        #expect(trainer.passes == 0)
        #expect(trainer.completePass() == .counted)
        #expect(trainer.completePass() == .counted)
        #expect(trainer.completePass() == .stepped(to: 0.95))
        #expect(trainer.passes == 0)
        for _ in 0..<2 { _ = trainer.completePass() }
        #expect(trainer.completePass() == .reachedTarget(1.0))
    }

    @Test func switchingClocksKeepsTheStep() {
        var trainer = SpeedTrainer(settings: Settings())
        for _ in 0..<4 { _ = trainer.completePass() }
        #expect(trainer.speed == 0.8)
        trainer.setRates([0.5, 0.75, 1, 1.25])
        #expect(trainer.speed == 1.0)  // first YouTube rate at or above 80 %
        #expect(trainer.atTarget)
        trainer.setRates(nil)
        #expect(trainer.speed == 1.0)
        var early = SpeedTrainer(settings: Settings(), rates: [0.5, 0.75, 1, 1.25])
        #expect(early.speed == 0.5)
        early.setRates(nil)
        #expect(early.speed == 0.6)
    }

    @Test func settingsParseAndClamp() {
        #expect(Settings(string: "60,5,100,1") == Settings())
        #expect(Settings(string: "70, 10, 110") == Settings(startPercent: 70, stepPercent: 10, targetPercent: 110, passesPerStep: 1))
        #expect(Settings(string: "60,5") == nil)
        #expect(Settings(string: "a,b,c") == nil)
        #expect(Settings(string: Settings().string) == Settings())
        let wild = Settings(startPercent: 5, stepPercent: 0, targetPercent: 900, passesPerStep: 0).normalized
        #expect(wild == Settings(startPercent: 25, stepPercent: 1, targetPercent: 200, passesPerStep: 1))
    }

    @Test func practiceTimer() {
        var timer = PracticeTimer()
        #expect(timer.take(at: 5) == 0)
        timer.start(at: 10)
        timer.start(at: 12)  // already running: ignored
        #expect(timer.take(at: 40) == 30)
        #expect(timer.take(at: 70) == 30)
        timer.stop(at: 75)
        #expect(!timer.isRunning)
        timer.start(at: 100)
        timer.stop(at: 102.5)
        #expect(timer.take(at: 500) == 7.5)
        #expect(timer.take(at: 600) == 0)
    }

    @Test func formatsDurations() {
        #expect(PracticeFormat.duration(30) == "<1m")
        #expect(PracticeFormat.duration(24 * 60 + 59) == "24m")
        #expect(PracticeFormat.duration(3600 + 12 * 60) == "1h 12m")
        #expect(PracticeFormat.duration(7200) == "2h")
        #expect(PracticeFormat.summary(totalSecs: 59, weekSecs: 59) == nil)
        #expect(PracticeFormat.summary(totalSecs: 4320, weekSecs: 1440) == "Practised 1h 12m · 24m this week")
        #expect(PracticeFormat.summary(totalSecs: 4320, weekSecs: 0) == "Practised 1h 12m")
        #expect(PracticeFormat.summary(totalSecs: 600, weekSecs: 600) == "Practised 10m, all this week")
    }
}
