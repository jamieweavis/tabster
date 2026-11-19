import CoreAudio
import Foundation
import Synchronization
import os

/// A stretch of tapped audio: mono `Float32` samples (native endian) and the host time
/// (`CACurrentMediaTime` base) of the first one.
struct TappedAudio: Sendable {
    var samples: Data
    var hostTime: Double?
}

enum SystemAudioTapError: Error, CustomStringConvertible {
    case failed(String, OSStatus)
    case unsupportedFormat

    var description: String {
        switch self {
        case let .failed(step, status): "\(step) failed (\(status))"
        case .unsupportedFormat: "the system audio isn't 32-bit float PCM"
        }
    }
}

/// Listens to what the Mac is playing through a Core Audio process tap (macOS 14.2+), so the
/// video's audio can be aligned to the tab without ever fetching it from YouTube.
///
/// Spike: the tap is global (everything except this process, which leaves the tab's own synth
/// out), because the video's sound comes from WebKit's GPU helper process rather than ours.
/// Anything else playing on the Mac lands in the capture too. The first use asks for the
/// "System Audio Recording" permission (`NSAudioCaptureUsageDescription`); when it's refused
/// the tap still runs but delivers silence.
///
/// Audio arrives on a private queue and waits in `pending` until the owner calls `take()`.
final class SystemAudioTap: Sendable {
    private static let log = Logger(subsystem: "com.jamieweavis.tabster", category: "audio")

    let sampleRate: Double
    private let tapID: AudioObjectID
    private let deviceID: AudioObjectID
    private let procID: AudioDeviceIOProcID
    private let pending: Pending
    private let stopped = Mutex(false)

    /// Audio waiting to be taken, shared with the tap's reader block.
    private final class Pending: Sendable {
        let chunks = Mutex<[TappedAudio]>([])
    }

    init() throws {
        let description = CATapDescription(stereoGlobalTapButExcludeProcesses: Self.ownProcessObject().map { [$0] } ?? [])
        description.uuid = UUID()
        description.name = "Tabster sync listener"
        description.isPrivate = true
        description.muteBehavior = .unmuted

        var tap = AudioObjectID(kAudioObjectUnknown)
        try Self.check(AudioHardwareCreateProcessTap(description, &tap), "Creating the audio tap")

        var device = AudioObjectID(kAudioObjectUnknown)
        var proc: AudioDeviceIOProcID?
        do {
            var format = AudioStreamBasicDescription()
            try Self.read(tap, kAudioTapPropertyFormat, into: &format, "Reading the tap's format")
            let isFloat = format.mFormatID == kAudioFormatLinearPCM
                && format.mFormatFlags & kAudioFormatFlagIsFloat != 0
                && format.mBitsPerChannel == 32
            guard isFloat, format.mSampleRate > 0 else { throw SystemAudioTapError.unsupportedFormat }

            // A private aggregate device carries the tap; the output device is in it only to
            // give it a clock.
            var settings: [String: Any] = [
                kAudioAggregateDeviceNameKey: "Tabster Sync Listener",
                kAudioAggregateDeviceUIDKey: UUID().uuidString,
                kAudioAggregateDeviceIsPrivateKey: true,
                kAudioAggregateDeviceIsStackedKey: false,
                kAudioAggregateDeviceTapAutoStartKey: true,
                kAudioAggregateDeviceTapListKey: [[
                    kAudioSubTapUIDKey: description.uuid.uuidString,
                    kAudioSubTapDriftCompensationKey: true,
                ]],
            ]
            if let output = Self.defaultOutputDeviceUID() {
                settings[kAudioAggregateDeviceMainSubDeviceKey] = output
                settings[kAudioAggregateDeviceSubDeviceListKey] = [[kAudioSubDeviceUIDKey: output]]
            }
            try Self.check(AudioHardwareCreateAggregateDevice(settings as CFDictionary, &device),
                           "Creating the tap's device")

            let pending = Pending()
            let queue = DispatchQueue(label: "com.jamieweavis.tabster.audio-tap", qos: .userInitiated)
            let status = AudioDeviceCreateIOProcIDWithBlock(&proc, device, queue) { _, input, inputTime, output, _ in
                // The device also has the real output in it: play nothing through it.
                for buffer in UnsafeMutableAudioBufferListPointer(output) {
                    if let data = buffer.mData { memset(data, 0, Int(buffer.mDataByteSize)) }
                }
                guard let chunk = Self.mono(UnsafeMutableAudioBufferListPointer(UnsafeMutablePointer(mutating: input)),
                                            time: inputTime.pointee)
                else { return }
                pending.chunks.withLock { $0.append(chunk) }
            }
            try Self.check(status, "Installing the tap's reader")
            guard let proc else { throw SystemAudioTapError.failed("Installing the tap's reader", status) }
            try Self.check(AudioDeviceStart(device, proc), "Starting the tap")

            sampleRate = format.mSampleRate
            tapID = tap
            deviceID = device
            procID = proc
            self.pending = pending
        } catch {
            if let proc { AudioDeviceDestroyIOProcID(device, proc) }
            if device != kAudioObjectUnknown { AudioHardwareDestroyAggregateDevice(device) }
            AudioHardwareDestroyProcessTap(tap)
            throw error
        }
    }

    deinit { stop() }

    /// The audio that has arrived since the last call, oldest first.
    func take() -> [TappedAudio] {
        pending.chunks.withLock { chunks in
            defer { chunks.removeAll(keepingCapacity: true) }
            return chunks
        }
    }

    /// Stop listening and release the tap. Safe to call twice.
    func stop() {
        let already = stopped.withLock { was in
            defer { was = true }
            return was
        }
        guard !already else { return }
        var status = AudioDeviceStop(deviceID, procID)
        if status == noErr { status = AudioDeviceDestroyIOProcID(deviceID, procID) }
        if status == noErr { status = AudioHardwareDestroyAggregateDevice(deviceID) }
        if status == noErr { status = AudioHardwareDestroyProcessTap(tapID) }
        if status != noErr { Self.log.error("Tearing down the audio tap failed (\(status))") }
    }

    /// Mix every channel of a buffer list of `Float32`s down to mono.
    private static func mono(_ buffers: UnsafeMutableAudioBufferListPointer, time: AudioTimeStamp) -> TappedAudio? {
        var frames = 0
        var channels = 0
        for buffer in buffers where buffer.mData != nil && buffer.mNumberChannels > 0 {
            let count = Int(buffer.mDataByteSize) / (MemoryLayout<Float32>.size * Int(buffer.mNumberChannels))
            frames = frames == 0 ? count : min(frames, count)
            channels += Int(buffer.mNumberChannels)
        }
        guard frames > 0, channels > 0 else { return nil }

        var mixed = [Float32](repeating: 0, count: frames)
        for buffer in buffers {
            guard let data = buffer.mData, buffer.mNumberChannels > 0 else { continue }
            let stride = Int(buffer.mNumberChannels)
            let samples = UnsafeBufferPointer(start: data.assumingMemoryBound(to: Float32.self), count: frames * stride)
            for frame in 0..<frames {
                var sum: Float32 = 0
                for channel in 0..<stride { sum += samples[frame * stride + channel] }
                mixed[frame] += sum
            }
        }
        let scale = 1 / Float32(channels)
        for i in mixed.indices { mixed[i] *= scale }

        let hostTime: Double? = time.mFlags.contains(.hostTimeValid)
            ? Double(AudioConvertHostTimeToNanos(time.mHostTime)) / 1e9
            : nil
        return TappedAudio(samples: mixed.withUnsafeBufferPointer { Data(buffer: $0) }, hostTime: hostTime)
    }

    // MARK: - Core Audio properties

    private static func check(_ status: OSStatus, _ step: String) throws {
        guard status == noErr else { throw SystemAudioTapError.failed(step, status) }
    }

    private static func address(_ selector: AudioObjectPropertySelector) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress(mSelector: selector, mScope: kAudioObjectPropertyScopeGlobal,
                                   mElement: kAudioObjectPropertyElementMain)
    }

    private static func read<T>(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector, into value: inout T,
                                _ step: String) throws {
        var address = address(selector)
        var size = UInt32(MemoryLayout<T>.size)
        let status = withUnsafeMutablePointer(to: &value) {
            AudioObjectGetPropertyData(object, &address, 0, nil, &size, $0)
        }
        try check(status, step)
    }

    /// This process as Core Audio knows it, if it has made any sound yet.
    private static func ownProcessObject() -> AudioObjectID? {
        var address = address(kAudioHardwarePropertyTranslatePIDToProcessObject)
        var pid = getpid()
        var object = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        let status = AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address,
                                                UInt32(MemoryLayout<pid_t>.size), &pid, &size, &object)
        return status == noErr && object != kAudioObjectUnknown ? object : nil
    }

    private static func defaultOutputDeviceUID() -> String? {
        var device = AudioObjectID(kAudioObjectUnknown)
        guard (try? read(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultOutputDevice,
                         into: &device, "")) != nil,
              device != kAudioObjectUnknown
        else { return nil }
        var uid: Unmanaged<CFString>?
        guard (try? read(device, kAudioDevicePropertyDeviceUID, into: &uid, "")) != nil else { return nil }
        return uid?.takeRetainedValue() as String?
    }
}
