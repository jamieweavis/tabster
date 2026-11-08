import Foundation

/// Pure helpers for the sync editor (offset taps, per-bar tap-along and marker drags, pasted
/// per-bar points, rate snapping). Times are seconds; "bars" are played bars (repeats
/// unrolled), identified by index into the session's played-bar list.
public enum SyncEditing {
    /// Index of the value in `starts` (sorted ascending) closest to `t`; nil if empty.
    public static func nearestIndex(in starts: [Double], to t: Double) -> Int? {
        guard !starts.isEmpty else { return nil }
        var lo = 0
        var hi = starts.count
        while lo < hi {
            let mid = (lo + hi) / 2
            if starts[mid] < t { lo = mid + 1 } else { hi = mid }
        }
        if lo == 0 { return 0 }
        if lo == starts.count { return starts.count - 1 }
        return t - starts[lo - 1] <= starts[lo] - t ? lo - 1 : lo
    }

    /// Offset mode: the offset that makes a bar starting at `scoreStart` (score seconds) start at
    /// video time `videoNow`, given `video = offset + score × stretch`.
    public static func offset(placing scoreStart: Double, at videoNow: Double, stretch: Double) -> Double {
        videoNow - scoreStart * (stretch > 0 && stretch.isFinite ? stretch : 1)
    }

    /// Points mode: move bar `index` to `video` and regenerate the bars around it.
    ///
    /// `videos[i]` is the video time of bar `i`'s start, `scoreStarts[i]` its score time; both
    /// have one entry per played bar. `keyframes` are bars the user placed explicitly (taps,
    /// drags); `index` itself need not be one. Between `index` and the nearest keyframe on either
    /// side, bars are re-interpolated linearly in score time; on a side with no keyframe, bars
    /// shift by the same amount as `index` (keeping their spacing).
    ///
    /// Returns nil (no change) if `video` isn't strictly between the neighbouring keyframes' video
    /// times, which would make the map non-monotonic.
    public static func retime(videos: [Double], scoreStarts: [Double], index: Int, to video: Double,
                              keyframes: Set<Int>) -> [Double]? {
        guard videos.count == scoreStarts.count, videos.indices.contains(index), video.isFinite else { return nil }
        let before = keyframes.filter { $0 < index && videos.indices.contains($0) }.max()
        let after = keyframes.filter { $0 > index && videos.indices.contains($0) }.min()
        if let before, video <= videos[before] { return nil }
        if let after, video >= videos[after] { return nil }
        var out = videos
        let delta = video - videos[index]
        out[index] = video
        func interpolate(_ range: Range<Int>, from a: Int, to b: Int) {
            let s0 = scoreStarts[a], s1 = scoreStarts[b]
            let v0 = out[a], v1 = out[b]
            guard s1 > s0 else { return }
            for i in range { out[i] = v0 + (scoreStarts[i] - s0) / (s1 - s0) * (v1 - v0) }
        }
        if let before {
            interpolate(before + 1..<index, from: before, to: index)
        } else {
            for i in 0..<index { out[i] = videos[i] + delta }
        }
        if let after {
            interpolate(index + 1..<after, from: index, to: after)
        } else {
            for i in index + 1..<out.count { out[i] = videos[i] + delta }
        }
        return out
    }

    /// Parses pasted per-bar sync data: a JSON array of numbers, an object with a `points`
    /// array, or an array of such objects (a `video-points` style list; the entry for
    /// `preferVideoID` wins, else the first with points). Returns nil if nothing usable.
    public static func parseBarPoints(_ text: String, preferVideoID: String? = nil) -> [Double]? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let data = trimmed.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed])
        else { return nil }
        func numbers(_ value: Any?) -> [Double]? {
            guard let array = value as? [Any], !array.isEmpty else { return nil }
            var out: [Double] = []
            out.reserveCapacity(array.count)
            for item in array {
                guard let n = item as? NSNumber, CFGetTypeID(n) != CFBooleanGetTypeID() else { return nil }
                let d = n.doubleValue
                guard d.isFinite else { return nil }
                out.append(d)
            }
            return out
        }
        if let direct = numbers(json) { return direct }
        if let object = json as? [String: Any] { return numbers(object["points"]) }
        if let list = json as? [[String: Any]] {
            let candidates = list.filter { numbers($0["points"]) != nil }
            let preferred = candidates.first { entry in
                guard let preferVideoID else { return false }
                return ["videoId", "videoID", "video_id", "youtubeId"].contains { entry[$0] as? String == preferVideoID }
            }
            return numbers((preferred ?? candidates.first)?["points"])
        }
        return nil
    }

    /// The playback rate in `rates` nearest to `rate` (or `rate` itself if `rates` is empty).
    public static func snapRate(_ rate: Double, to rates: [Double]) -> Double {
        rates.min { abs($0 - rate) < abs($1 - rate) } ?? rate
    }
}
