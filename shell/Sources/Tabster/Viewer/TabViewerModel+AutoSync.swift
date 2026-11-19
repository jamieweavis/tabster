import Foundation

/// Where an automatic sync is (see `TabViewerModel.toggleAutoSync`).
enum AutoSyncState: Equatable {
    case idle
    /// The video is playing and being listened to; `seconds` heard so far.
    case listening(seconds: Double)
    /// Listening is over; the core is aligning what was heard.
    case aligning
    case finished(String)
    case failed(String)

    /// One line for the sync bar.
    var status: String? {
        switch self {
        case .idle: nil
        case let .listening(seconds):
            String(format: "Listening… %d:%02d (a few bars of the song place the tab; the whole song follows its tempo. Then Stop & Sync)", Int(seconds) / 60, Int(seconds) % 60)
        case .aligning: "Lining the tab up with what was heard…"
        case let .finished(message), let .failed(message): message
        }
    }
}

/// One listening session: the system-audio tap, the core's capture it feeds, and the task that
/// moves audio from one to the other.
@MainActor
final class SyncListener {
    let tap: SystemAudioTap
    let capture: AudioCapture
    /// The video being listened to.
    let videoID: Int64
    var pump: Task<Void, Never>?
    /// The video has actually started (so a pause or its end means "that's all").
    var heardPlaying = false

    init(tap: SystemAudioTap, videoID: Int64) {
        self.tap = tap
        self.videoID = videoID
        #if DEBUG
        // Debug builds keep the audio so a bad sync can be reproduced (see `dumpCapture`).
        capture = AudioCapture.recording(sampleRate: tap.sampleRate)
        #else
        capture = AudioCapture(sampleRate: tap.sampleRate)
        #endif
    }
}

/// Automatic sync (spike): play the video from its start, listen to the Mac's audio output while
/// it plays, then let the core find every bar in what was heard (`TabSession.syncFromAudio`).
/// Nothing is fetched from YouTube and no audio is kept: the capture holds alignment features only.
extension TabViewerModel {
    /// The sync bar's Listen button: start listening, or stop and sync with what was heard.
    func toggleAutoSync() {
        switch autoSync {
        case .listening: finishListening()
        case .aligning: break
        case .idle, .finished, .failed: startListening()
        }
    }

    private func startListening() {
        guard usingVideoClock, session != nil, syncListener == nil, let videoID = selectedVideoID else { return }
        let tap: SystemAudioTap
        do {
            tap = try SystemAudioTap()
        } catch {
            ViewerLog.log.error("Starting the audio tap failed: \(String(describing: error), privacy: .public)")
            autoSync = .failed("Couldn't listen to the Mac's audio: \(error)")
            return
        }
        let listener = SyncListener(tap: tap, videoID: videoID)
        syncListener = listener

        // One uninterrupted pass at normal speed, from the top of the video: the capture's
        // clock and the video's must stay in step.
        pause()
        setSpeed(1)
        clock.seek(to: 0)
        play()
        autoSync = .listening(seconds: 0)
        ViewerLog.log.info("Auto sync: listening at \(tap.sampleRate, format: .fixed(precision: 0)) Hz")

        listener.pump = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(250))
                guard let self, !Task.isCancelled else { return }
                self.pumpListener()
            }
        }
    }

    /// Move the audio heard since the last call into the capture, stamped with the video time
    /// it was heard at.
    private func drainTap(_ listener: SyncListener) {
        let playing = player?.state == .playing
        for chunk in listener.tap.take() {
            // Only while the video runs: a stalled video's time says nothing about the audio's.
            let video = playing ? chunk.hostTime.flatMap { videoTime(at: $0) } : nil
            listener.capture.push(pcm: chunk.samples, videoSecs: video)
        }
        if playing { listener.heardPlaying = true }
    }

    private func pumpListener() {
        guard let listener = syncListener else { return }
        drainTap(listener)
        autoSync = .listening(seconds: listener.capture.durationSecs())
        // The video ended, or was paused: that's all there is to hear.
        if listener.heardPlaying, let state = player?.state, state == .ended || state == .paused {
            finishListening()
        } else if !usingVideoClock || selectedVideoID != listener.videoID {
            cancelAutoSync()
        }
    }

    private func finishListening() {
        guard let listener = syncListener, let session else { return }
        syncListener = nil
        listener.pump?.cancel()
        drainTap(listener)
        listener.tap.stop()
        pause()
        autoSync = .aligning

        let capture = listener.capture
        let videoID = listener.videoID
        let tabPath = (try? core.tabFilePath(id: tabID)) ?? ""
        let youtubeID = selectedVideo?.youtubeId ?? ""
        Task { [weak self] in
            let started = Date()
            let result = await Task.detached {
                #if DEBUG
                Self.dumpCapture(capture, tabPath: tabPath, video: youtubeID)
                #endif
                return Result { try session.syncFromAudio(capture: capture) }
            }.value
            guard let self, self.autoSync == .aligning else { return }
            switch result {
            case let .success(found):
                ViewerLog.log.notice("""
                    Auto sync: bars \(found.firstBar + 1)–\(found.lastBar + 1), \(found.matchedBars) by onsets, \
                    confidence \(found.confidence, format: .fixed(precision: 2)), \
                    stretch \(found.stretch, format: .fixed(precision: 3)), transpose \(found.transpose), \
                    \(Date().timeIntervalSince(started), format: .fixed(precision: 2)) s
                    """)
                guard self.selectedVideoID == videoID else {
                    self.autoSync = .idle
                    return
                }
                self.syncKeyframes = []
                self.applySync(found.sync)
                // Save the sync first: saving one with nothing heard in it would clear the tracking.
                self.flushSyncSave(synchronously: true)
                do {
                    try self.core.setVideoTracking(id: videoID, tracking: found.confidence)
                    if let i = self.videos.firstIndex(where: { $0.id == videoID }) {
                        self.videos[i].tracking = found.confidence
                    }
                } catch {
                    ViewerLog.log.error("Saving the tracking failed: \(error)")
                }
                self.autoSync = .finished(Self.summary(of: found, bars: self.playedBars.count))
            case let .failure(error):
                ViewerLog.log.notice("Auto sync failed: \(String(describing: error), privacy: .public)")
                self.autoSync = .failed(Self.explanation(of: error))
            }
        }
    }

    // MARK: - Bulk sync

    /// This tab's turn in a bulk sync (`AppModel.startBulkSync`): once the tab has opened,
    /// listen to its video from start to end (or until Stop & Sync) and sync it. Nil when
    /// cancelled (the viewer closed, or the run was stopped), which also stops the listening.
    func syncForBulk() async -> BulkSyncOutcome? {
        /// One poll; false when cancelled.
        func tick() async -> Bool {
            try? await Task.sleep(for: .milliseconds(250))
            if Task.isCancelled {
                if syncListener != nil { cancelAutoSync() }
                return false
            }
            return true
        }

        while case .loading = phase {
            guard await tick() else { return nil }
        }
        guard case .ready = phase else { return .skipped("the tab couldn't be opened") }
        guard selectedVideo != nil else { return .skipped("no video") }
        if !videoAudio { videoAudio = true }
        // The video takes over as the clock once its player has it (or fails to).
        var waited = 0.0
        while !usingVideoClock {
            if player?.failure != nil || waited > 10 { return .skipped("the video couldn't be played") }
            guard await tick() else { return nil }
            waited += 0.25
        }
        // A listen already under way (started by hand) counts as this one.
        while autoSync == .aligning {
            guard await tick() else { return nil }
        }
        if syncListener == nil { toggleAutoSync() }
        // The tap itself failed: so would every other tab's.
        if case let .failed(message) = autoSync { return .abort(message) }

        var silent = 0.0
        while true {
            guard await tick() else { return nil }
            switch autoSync {
            case .listening:
                if syncListener?.heardPlaying == true { continue }
                silent += 0.25
                if silent > 45 {
                    cancelAutoSync()
                    return .skipped("the video never started")
                }
            case .aligning:
                continue
            case .finished:
                return .synced
            case let .failed(message):
                // Nothing heard at all is the Mac's audio (permission, output), not this tab.
                return message == Self.explanation(of: AudioSyncError.Silent) ? .abort(message) : .failed(message)
            case .idle:
                return .skipped("the video stopped being the clock")
            }
        }
    }

    /// Stop listening without syncing (the tab is closing, or the video changed).
    func cancelAutoSync() {
        if let listener = syncListener {
            listener.pump?.cancel()
            listener.tap.stop()
            syncListener = nil
        }
        autoSync = .idle
    }

    #if DEBUG
    /// Write the capture's audio and clock data to the temporary directory, to reproduce the
    /// sync offline: `cargo run --release --example audio_sync -- <tab> <wav> <clock_origin>`.
    private nonisolated static func dumpCapture(_ capture: AudioCapture, tabPath: String, video: String) {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("tabster-autosync", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let stem = dir.appendingPathComponent("capture-\(Int(Date().timeIntervalSince1970))").path
        if capture.saveDebug(stem: stem, tabPath: tabPath, video: video) {
            ViewerLog.log.notice("Auto sync: capture saved to \(stem, privacy: .public).wav")
        } else {
            ViewerLog.log.error("Auto sync: saving the capture to \(stem, privacy: .public) failed")
        }
    }
    #endif

    private static func summary(of found: AudioSync, bars: Int) -> String {
        var parts: [String] = []
        if found.firstBar == 0, Int(found.lastBar) == bars - 1 {
            parts.append("Synced all \(bars) bars by ear")
        } else {
            parts.append("Synced bars \(found.firstBar + 1)–\(found.lastBar + 1) of \(bars) by ear; the rest keep that tempo")
        }
        parts.append(String(format: "%.0f%% tracking", found.confidence * 100))
        if found.transpose != 0 {
            let n = abs(Int(found.transpose))
            parts.append("recording is \(n) semitone\(n == 1 ? "" : "s") \(found.transpose > 0 ? "higher" : "lower") than the tab")
        }
        if found.confidence < 0.4 { parts.append("check it before trusting it") }
        return parts.joined(separator: " · ")
    }

    private static func explanation(of error: Error) -> String {
        guard let error = error as? AudioSyncError else { return "Automatic sync failed: \(error)" }
        return switch error {
        case .TooShort: "Not enough to go on: listen for at least 15 seconds."
        case .Silent:
            "Heard nothing. Check the video isn't muted, and that Tabster is allowed under System Settings › Privacy & Security › Screen & System Audio Recording."
        case .UnsteadyClock: "The video paused, buffered or jumped while listening. Try again without touching it."
        case .TooLong: "That's too much audio to line up in one go."
        case .NoMatch: "Couldn't hear the tab in what was playing. Is it the right video, and is anything else making sound?"
        }
    }
}
