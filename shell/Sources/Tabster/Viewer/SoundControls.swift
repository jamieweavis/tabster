import AppKit
import SwiftUI

// MARK: - Full-width segmented picker

/// A segmented picker whose equal-width segments fill the width offered (SwiftUI's segmented
/// `Picker` on macOS stays at its labels' width).
struct FillSegmentedPicker<Value: Hashable>: NSViewRepresentable {
    let options: [Value]
    @Binding var selection: Value
    let title: (Value) -> String

    func makeNSView(context: Context) -> NSSegmentedControl {
        let control = NSSegmentedControl(labels: options.map(title), trackingMode: .selectOne,
                                         target: context.coordinator, action: #selector(Coordinator.changed(_:)))
        control.segmentDistribution = .fillEqually
        control.setContentHuggingPriority(.defaultLow, for: .horizontal)
        control.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return control
    }

    func updateNSView(_ control: NSSegmentedControl, context: Context) {
        context.coordinator.parent = self
        control.selectedSegment = options.firstIndex(of: selection) ?? -1
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView: NSSegmentedControl, context: Context) -> CGSize? {
        CGSize(width: proposal.width ?? nsView.intrinsicContentSize.width, height: nsView.intrinsicContentSize.height)
    }

    func makeCoordinator() -> Coordinator { Coordinator(parent: self) }

    @MainActor final class Coordinator: NSObject {
        var parent: FillSegmentedPicker

        init(parent: FillSegmentedPicker) { self.parent = parent }

        @objc func changed(_ sender: NSSegmentedControl) {
            guard parent.options.indices.contains(sender.selectedSegment) else { return }
            parent.selection = parent.options[sender.selectedSegment]
        }
    }
}

// MARK: - Split toggle

/// A player-bar control split like a menu button: the icon toggles, the chevron opens a popover
/// of options. Drawn like the bar's other buttons, tinted with the accent colour while on.
struct SplitToggleButton<Options: View>: View {
    let title: String
    /// An SF Symbol, or (with `glyph`) ignored.
    var systemImage = ""
    /// A text glyph drawn instead of a symbol, for icons SF Symbols lacks (♫).
    var glyph: String?
    @Binding var isOn: Bool
    @Binding var showOptions: Bool
    let help: String
    let optionsHelp: String
    @ViewBuilder let options: () -> Options
    @State private var hovering = false
    @Environment(\.isEnabled) private var isEnabled

    @ViewBuilder private var icon: some View {
        if let glyph {
            Text(glyph).font(.system(size: 17, weight: .medium))
        } else {
            Image(systemName: systemImage).font(.system(size: 15, weight: .medium))
        }
    }

    var body: some View {
        let on = isOn && isEnabled
        HStack(spacing: 0) {
            Button {
                isOn.toggle()
            } label: {
                icon
                    .frame(width: 28, height: 30)
                    .padding(.leading, 4)
                    .contentShape(Rectangle())
            }
            .help(help)
            .accessibilityLabel(title)
            .accessibilityAddTraits(isOn ? .isSelected : [])

            Button {
                showOptions.toggle()
            } label: {
                Image(systemName: "chevron.down")
                    .font(.system(size: 8, weight: .bold))
                    .opacity(0.7)
                    .frame(width: 12, height: 30)
                    .padding(.trailing, 6)
                    .contentShape(Rectangle())
            }
            .help(optionsHelp)
            .accessibilityLabel("\(title) Options")
        }
        .buttonStyle(.plain)
        .foregroundStyle(on ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.primary))
        .background {
            Capsule().fill(PlayerPillStyle.fill(on: on, hovering: hovering && isEnabled))
        }
        .opacity(isEnabled ? 1 : 0.35)
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
        .popover(isPresented: $showOptions, arrowEdge: .top) {
            options()
                .controlSize(.regular)
                .padding(16)
                .frame(width: 320)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// Popover building blocks shared by the sound panels.
enum SoundPanel {
    /// The panel's title with its on/off switch.
    static func header(_ title: String, isOn: Binding<Bool>) -> some View {
        HStack {
            Text(title).font(.headline)
            Spacer()
            Toggle(title, isOn: isOn)
                .toggleStyle(.switch)
                .labelsHidden()
        }
    }

    /// A labelled slider with its value shown on the right.
    static func slider(_ title: String, value: String, @ViewBuilder slider: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(title).font(.subheadline)
                Spacer()
                Text(value)
                    .font(.subheadline)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
            slider()
        }
    }

    /// A volume slider between quiet / loud speaker icons.
    static func volume(_ value: Binding<Float>, in range: ClosedRange<Float>,
                       onEditingChanged: @escaping (Bool) -> Void = { _ in }) -> some View {
        Self.slider("Volume", value: percent(value.wrappedValue)) {
            HStack {
                Image(systemName: "speaker.fill").foregroundStyle(.secondary)
                Slider(value: value, in: range, onEditingChanged: onEditingChanged)
                Image(systemName: "speaker.wave.3.fill").foregroundStyle(.secondary)
            }
        }
    }

    static func percent(_ value: Float) -> String {
        "\(Int((value * 100).rounded()))%"
    }
}

// MARK: - Tab audio

struct TabAudioButton: View {
    let model: TabViewerModel

    var body: some View {
        SplitToggleButton(
            title: "Tab Audio",
            glyph: "♫",
            isOn: Binding(get: { model.tabAudio }, set: { model.tabAudio = $0 }),
            showOptions: Binding(get: { model.showTabAudioOptions }, set: { model.showTabAudioOptions = $0 }),
            help: "Hear the tab (turns the video off)",
            optionsHelp: "Tab audio volume and kit"
        ) {
            TabAudioPanel(model: model)
        }
    }
}

struct TabAudioPanel: View {
    let model: TabViewerModel

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            SoundPanel.header("Tab Audio", isOn: Binding(get: { model.tabAudio }, set: { model.tabAudio = $0 }))

            VStack(alignment: .leading, spacing: 4) {
                Text("Kit").font(.subheadline)
                FillSegmentedPicker(options: TabKit.allCases,
                                    selection: Binding(get: { model.tabKit }, set: { model.tabKit = $0 }),
                                    title: \.title)
                    .accessibilityLabel("Kit")
                Text(model.tabKit.detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            SoundPanel.volume(Binding(get: { model.tabVolume }, set: { model.tabVolume = $0 }), in: 0...1.5)
        }
        .disabled(!model.synthAvailable)
    }
}

// MARK: - Video audio

/// The YouTube video as the audio source: on plays the selected video (hidden) and turns tab
/// audio off; the options pick, attach and manage the tab's videos. With no video attached the
/// toggle opens the options, to attach one.
struct VideoAudioButton: View {
    let model: TabViewerModel

    var body: some View {
        SplitToggleButton(
            title: "Video",
            systemImage: "play.rectangle",
            isOn: Binding(get: { model.hearingVideo }, set: { on in
                if model.videos.isEmpty {
                    model.showAttachSheet = true
                } else {
                    model.videoAudio = on
                }
            }),
            // With nothing attached the panel would only offer "Attach Video…", so go straight there.
            showOptions: Binding(get: { model.showVideoOptions }, set: { show in
                if show && model.videos.isEmpty {
                    model.showAttachSheet = true
                } else {
                    model.showVideoOptions = show
                }
            }),
            help: model.videos.isEmpty ? "Attach a YouTube video to play with the tab"
                                       : "Hear the YouTube video (turns tab audio off)",
            optionsHelp: "Choose, attach or remove videos"
        ) {
            VideoPanel(model: model)
        }
    }
}

struct VideoPanel: View {
    let model: TabViewerModel

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            SoundPanel.header("YouTube", isOn: Binding(get: { model.hearingVideo }, set: { model.videoAudio = $0 }))
                .disabled(model.videos.isEmpty)

            if model.videos.isEmpty {
                Text("No videos attached to this tab.")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            } else {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(model.videos, id: \.id) { video in
                        row(video)
                    }
                }
            }

            Button("Attach Video…", systemImage: "plus") {
                model.showVideoOptions = false
                model.showAttachSheet = true
            }

            Button(model.syncEditing ? "Stop Syncing" : "Sync Tab to Video…", systemImage: "arrow.left.and.right") {
                model.showVideoOptions = false
                model.syncEditing.toggle()
            }
            .help(model.hearingVideo ? "Line the tab's bars up with the video" : "Turn on a video to sync the tab to it")
            .disabled(!model.hearingVideo)
        }
    }

    private func row(_ video: Video) -> some View {
        let selected = video.id == model.selectedVideoID
        return HStack(spacing: 6) {
            Button {
                model.chooseVideo(video)
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "checkmark")
                        .font(.system(size: 11, weight: .semibold))
                        .opacity(selected ? 1 : 0)
                    Text(Self.title(video))
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
            }
            .help("Play this video")

            Button {
                model.makeDefault(video)
            } label: {
                Image(systemName: video.isDefault ? "star.fill" : "star")
                    .foregroundStyle(video.isDefault ? AnyShapeStyle(.yellow) : AnyShapeStyle(.secondary))
            }
            .help(video.isDefault ? "Opens with the tab" : "Make this the video the tab opens with")
            .disabled(video.isDefault)

            Button {
                model.removeVideo(video)
            } label: {
                Image(systemName: "trash").foregroundStyle(.secondary)
            }
            .help("Remove this video")
        }
        .buttonStyle(.plain)
        .padding(.vertical, 3)
    }

    static func title(_ v: Video) -> String {
        v.label.isEmpty ? v.youtubeId : "\(v.label) (\(v.youtubeId))"
    }
}

// MARK: - Metronome

struct MetronomeButton: View {
    let model: TabViewerModel

    var body: some View {
        SplitToggleButton(
            title: "Metronome",
            systemImage: "metronome",
            isOn: Binding(get: { model.metronome }, set: { model.metronome = $0 }),
            showOptions: Binding(get: { model.showMetronomeOptions }, set: { model.showMetronomeOptions = $0 }),
            help: model.usingVideoClock ? "Metronome over the video" : "Metronome",
            optionsHelp: "Metronome sound, volume and pitch"
        ) {
            MetronomePanel(model: model)
        }
    }
}

struct MetronomePanel: View {
    let model: TabViewerModel

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            SoundPanel.header("Metronome", isOn: Binding(get: { model.metronome }, set: { model.metronome = $0 }))

            FillSegmentedPicker(options: MetronomeSound.allCases,
                                selection: Binding(get: { model.metronomeSound }, set: { model.metronomeSound = $0 }),
                                title: \.title)
                .accessibilityLabel("Sound")
                .onChange(of: model.metronomeSound) { model.previewMetronome() }

            SoundPanel.volume(Binding(get: { model.metronomeVolume }, set: { model.metronomeVolume = $0 }),
                              in: 0...3) { editing in
                if !editing { model.previewMetronome() }
            }

            SoundPanel.slider("Pitch", value: Self.semitones(model.metronomePitch)) {
                Slider(value: Binding(get: { model.metronomePitch }, set: { model.metronomePitch = $0 }),
                       in: -12...12, step: 1)
                    .onChange(of: model.metronomePitch) { model.previewMetronome() }
            }

            Divider()

            Toggle(isOn: Binding(get: { model.countIn }, set: { model.countIn = $0 })) {
                Text("Count in one bar")
                Text(countInNote)
            }
            .toggleStyle(.switch)
            .disabled(!countInAvailable)
        }
        .disabled(!model.synthAvailable)
    }

    /// Count-in is played by the synth, so it needs the synth to be the clock: no video, and
    /// tab audio or the metronome on.
    private var countInAvailable: Bool {
        !model.usingVideoClock && (model.tabAudio || model.metronome)
    }

    private var countInNote: String {
        if model.usingVideoClock { return "Not available with the video" }
        return countInAvailable ? "Before playing" : "Needs tab audio or the metronome"
    }

    static func semitones(_ value: Float) -> String {
        let n = Int(value.rounded())
        return n == 0 ? "0 st" : String(format: "%+d st", n)
    }
}
