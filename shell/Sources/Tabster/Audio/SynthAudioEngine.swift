import AVFoundation
import Synchronization
import os

/// Where the last rendered buffer sits on both clocks, written by the audio thread and read by
/// `SynthPlaybackClock` on the main thread.
struct SynthAnchor: Sendable {
    /// Score seconds at the first / past the last frame of the buffer.
    var scoreStart: Double
    var scoreEnd: Double
    /// Host time (seconds, `CACurrentMediaTime` base) when the buffer's first frame is heard.
    var heardAt: Double
    /// The score clock was held for a count-in during this buffer.
    var countingIn: Bool
}

/// State shared with the real-time render block.
final class SynthRenderState: Sendable {
    let anchor = Mutex<SynthAnchor?>(nil)
    /// Seconds from a buffer's render timestamp until it reaches the speakers.
    let outputLatency = Mutex<Double>(0)
    /// Speed, for extrapolating between buffers.
    let rate = Mutex<Double>(1)
}

/// Plays a `TabSynth` through AVAudioEngine: an `AVAudioSourceNode` pulls interleaved stereo
/// from `TabSynth.render` on the audio thread and deinterleaves it (the main mixer converts to
/// the device's rate). The render block captures only the synth and the shared state, never the
/// main actor.
@MainActor
final class SynthAudioEngine {
    static let log = Logger(subsystem: "com.jamieweavis.tabster", category: "audio")

    let synth: TabSynth
    let state = SynthRenderState()
    private let engine = AVAudioEngine()
    private let source: AVAudioSourceNode
    private var configObserver: NSObjectProtocol?

    init(synth: TabSynth) {
        self.synth = synth
        let rate = Double(synth.sampleRate())
        let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 2)!
        source = AVAudioSourceNode(format: format, renderBlock: Self.renderBlock(synth: synth, state: state, sampleRate: rate))
        engine.attach(source)
        engine.connect(source, to: engine.mainMixerNode, format: format)
        engine.prepare()
        configObserver = NotificationCenter.default.addObserver(
            forName: .AVAudioEngineConfigurationChange, object: engine, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.restartAfterConfigurationChange() }
        }
    }

    isolated deinit {
        if let configObserver { NotificationCenter.default.removeObserver(configObserver) }
        engine.stop()
    }

    var isRunning: Bool { engine.isRunning }

    func start() throws {
        guard !engine.isRunning else { return }
        try engine.start()
        updateLatency()
    }

    /// Pause the audio device (no CPU while idle). The synth keeps its state.
    func pause() {
        engine.pause()
        state.anchor.withLock { $0 = nil }
    }

    func stop() {
        engine.stop()
        state.anchor.withLock { $0 = nil }
    }

    private func updateLatency() {
        let output = engine.outputNode
        let latency = output.presentationLatency + engine.mainMixerNode.outputPresentationLatency
        state.outputLatency.withLock { $0 = max(0, latency) }
    }

    private func restartAfterConfigurationChange() {
        // The output device changed (headphones, sample rate …); the engine stopped itself.
        guard !engine.isRunning else { return }
        do {
            try engine.start()
            updateLatency()
            Self.log.info("Audio engine restarted after a configuration change")
        } catch {
            Self.log.error("Couldn't restart the audio engine: \(error.localizedDescription, privacy: .public)")
        }
    }

    /// Built outside the main actor so the closure doesn't inherit its isolation.
    private nonisolated static func renderBlock(
        synth: TabSynth, state: SynthRenderState, sampleRate: Double
    ) -> AVAudioSourceNodeRenderBlock {
        { _, timestamp, frameCount, audioBufferList -> OSStatus in
            let before = synth.positionSecs()
            let samples = synth.render(frames: frameCount)
            let after = synth.positionSecs()
            let countingIn = synth.isCountingIn()

            let buffers = UnsafeMutableAudioBufferListPointer(audioBufferList)
            let frames = Int(frameCount)
            samples.withUnsafeBufferPointer { src in
                for (channel, buffer) in buffers.enumerated() {
                    guard let data = buffer.mData?.assumingMemoryBound(to: Float.self) else { continue }
                    let offset = min(channel, 1)
                    for i in 0..<min(frames, src.count / 2) {
                        data[i] = src[i * 2 + offset]
                    }
                }
            }

            let ts = timestamp.pointee
            let hostTime = ts.mFlags.contains(.hostTimeValid) ? ts.mHostTime : mach_absolute_time()
            let latency = state.outputLatency.withLock { $0 }
            let heardAt = AVAudioTime.seconds(forHostTime: hostTime) + latency
            let anchor: SynthAnchor
            if after >= before {
                anchor = SynthAnchor(scoreStart: before, scoreEnd: after, heardAt: heardAt, countingIn: countingIn)
            } else {
                // Wrapped (loop): anchor at the buffer's end on the new pass.
                let end = heardAt + Double(frameCount) / sampleRate
                anchor = SynthAnchor(scoreStart: after, scoreEnd: after, heardAt: end, countingIn: countingIn)
            }
            state.anchor.withLock { $0 = anchor }
            return noErr
        }
    }
}
