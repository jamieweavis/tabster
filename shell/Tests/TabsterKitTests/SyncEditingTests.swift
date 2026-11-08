import Foundation
import Testing

@testable import TabsterKit

struct SyncEditingTests {
    let starts = [0.0, 2, 4, 6, 8]

    @Test func nearestIndex() {
        #expect(SyncEditing.nearestIndex(in: [], to: 1) == nil)
        #expect(SyncEditing.nearestIndex(in: starts, to: -5) == 0)
        #expect(SyncEditing.nearestIndex(in: starts, to: 0.9) == 0)
        #expect(SyncEditing.nearestIndex(in: starts, to: 1.1) == 1)
        #expect(SyncEditing.nearestIndex(in: starts, to: 5.9) == 3)
        #expect(SyncEditing.nearestIndex(in: starts, to: 100) == 4)
    }

    @Test func offsetTap() {
        #expect(SyncEditing.offset(placing: 0, at: 4.25, stretch: 1.02) == 4.25)
        #expect(abs(SyncEditing.offset(placing: 10, at: 14, stretch: 1.1) - 3) < 1e-12)
        #expect(SyncEditing.offset(placing: 10, at: 14, stretch: 0) == 4)
    }

    @Test func firstTapShiftsEverything() throws {
        let videos = starts.map { $0 + 1 }
        let out = try #require(SyncEditing.retime(videos: videos, scoreStarts: starts, index: 2, to: 5.5, keyframes: []))
        #expect(out == [1.5, 3.5, 5.5, 7.5, 9.5])
    }

    @Test func tapBetweenKeyframesInterpolates() throws {
        let videos = starts.map { $0 + 1 }
        // Keyframe at bar 0 (video 1); move bar 4 from 9 to 11: bars 1-3 spread evenly.
        let out = try #require(SyncEditing.retime(videos: videos, scoreStarts: starts, index: 4, to: 11, keyframes: [0]))
        #expect(out == [1, 3.5, 6, 8.5, 11])
        // Keyframes on both sides of bar 2: only bars 1 and 3 change.
        let mid = try #require(SyncEditing.retime(videos: out, scoreStarts: starts, index: 2, to: 5, keyframes: [0, 4]))
        #expect(mid == [1, 3, 5, 8, 11])
    }

    @Test func rejectsNonMonotonicTap() {
        let videos = starts
        #expect(SyncEditing.retime(videos: videos, scoreStarts: starts, index: 2, to: 1.9, keyframes: [1]) == nil)
        #expect(SyncEditing.retime(videos: videos, scoreStarts: starts, index: 2, to: 6, keyframes: [3]) == nil)
        #expect(SyncEditing.retime(videos: videos, scoreStarts: starts, index: 9, to: 6, keyframes: []) == nil)
    }

    @Test func parsesBarPoints() {
        #expect(SyncEditing.parseBarPoints("[1, 2.5, 4]") == [1, 2.5, 4])
        #expect(SyncEditing.parseBarPoints(#"{"id": 3, "points": [0.5, 1]}"#) == [0.5, 1])
        let list = #"[{"videoId": "aaaaaaaaaaa", "points": [1]}, {"videoId": "cJRFMfztgBg", "points": [2, 3]}]"#
        #expect(SyncEditing.parseBarPoints(list, preferVideoID: "cJRFMfztgBg") == [2, 3])
        #expect(SyncEditing.parseBarPoints(list) == [1])
        #expect(SyncEditing.parseBarPoints("[true, 1]") == nil)
        #expect(SyncEditing.parseBarPoints("[]") == nil)
        #expect(SyncEditing.parseBarPoints("nonsense") == nil)
    }

    @Test func snapsRates() {
        let rates = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 1.75, 2]
        #expect(SyncEditing.snapRate(0.85, to: rates) == 0.75)
        #expect(SyncEditing.snapRate(0.9, to: rates) == 1)
        #expect(SyncEditing.snapRate(3, to: rates) == 2)
        #expect(SyncEditing.snapRate(0.9, to: []) == 0.9)
    }
}
