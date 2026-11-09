import Testing

@testable import TabsterKit

struct PracticeTests {
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
    }
}
