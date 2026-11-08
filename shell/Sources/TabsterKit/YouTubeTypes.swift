import Foundation

/// The YouTube player's state, as reported by the IFrame API's `onStateChange`.
public enum PlaybackState: String, Sendable, Equatable, CaseIterable {
    case unstarted, buffering, playing, paused, ended

    /// Maps an IFrame API state code (-1 unstarted, 0 ended, 1 playing, 2 paused, 3 buffering,
    /// 5 cued). A cued video hasn't started yet, so it counts as `unstarted`.
    public init(youTubeCode code: Int) {
        switch code {
        case 0: self = .ended
        case 1: self = .playing
        case 2: self = .paused
        case 3: self = .buffering
        default: self = .unstarted
        }
    }
}

/// Why the YouTube player can't play a video (the IFrame API's `onError` codes).
public enum YouTubePlayerFailure: Error, Sendable, Equatable, Hashable {
    /// 101 or 150: the owner disabled embedding. YouTube also reports a nonexistent id as 150.
    case embeddingDisabled(code: Int)
    /// 100: removed or private.
    case notFound
    /// 2: malformed video id.
    case invalidID
    /// 5: the HTML5 player failed.
    case html5
    /// 153: the embed got no Referer (and the localhost fallback failed or wasn't available).
    case missingReferrer
    /// Anything else. -1: the IFrame API script couldn't load (offline?); -2: the web content
    /// process died; -3: the page never became ready.
    case other(Int)

    public init(code: Int) {
        switch code {
        case 101, 150: self = .embeddingDisabled(code: code)
        case 100: self = .notFound
        case 2: self = .invalidID
        case 5: self = .html5
        case 153: self = .missingReferrer
        default: self = .other(code)
        }
    }

    /// The video can't play embedded no matter how often we retry: offer YouTube itself instead.
    public var isPermanent: Bool {
        switch self {
        case .embeddingDisabled, .notFound, .invalidID: true
        case .html5, .missingReferrer, .other: false
        }
    }

    public var title: String {
        switch self {
        case .embeddingDisabled: "Video can't be embedded"
        case .notFound: "Video not found"
        case .invalidID: "Invalid video"
        case .html5: "Playback failed"
        case .missingReferrer: "YouTube refused the player"
        case .other(-1): "Couldn't reach YouTube"
        case .other: "Video failed to load"
        }
    }

    public var detail: String {
        switch self {
        case .embeddingDisabled: "Its owner only allows it to be played on YouTube."
        case .notFound: "It may have been removed or made private."
        case .invalidID: "The video id isn't valid."
        case .html5: "YouTube's player hit an error."
        case .missingReferrer: "Error 153: the player couldn't identify itself to YouTube."
        case .other(-1): "Check your internet connection."
        case .other(let code): "Error \(code)."
        }
    }
}

public enum YouTubeLinks {
    /// `https://www.youtube.com/watch?v=<id>&t=<whole seconds>s`.
    public static func watchURL(videoID: String, at seconds: Double = 0) -> URL {
        var c = URLComponents(string: "https://www.youtube.com/watch")!
        var items = [URLQueryItem(name: "v", value: videoID)]
        let s = Int(max(0, seconds.isFinite ? seconds : 0))
        if s > 0 { items.append(URLQueryItem(name: "t", value: "\(s)s")) }
        c.queryItems = items
        return c.url!
    }
}
