import SwiftUI

/// The player: one Liquid Glass bar floating over the bottom of the tab, like Music's. Transport
/// on the left; what's playing and the scrubber in the middle; what you hear (tab audio, the
/// YouTube video, the metronome) and the speed on the right. The canvas leaves
/// `TabCanvasContainer.playerClearance` under the tab (while the bar is pinned) so it never
/// hides the last line.
struct PlayerBar: View {
    let model: TabViewerModel
    @Binding var loopPrompt: LoopPrompt?

    var body: some View {
        HStack(spacing: 14) {
            transport
                .fixedSize()
            NowPlaying(model: model)
                .frame(minWidth: 140, maxWidth: .infinity)
            sound
                .fixedSize()
        }
        .padding(.leading, 16)
        .padding(.trailing, 12)
        .frame(height: 60)
        .glassEffect(.regular, in: .capsule)
    }

    private var transport: some View {
        HStack(spacing: 6) {
            LoopButton(model: model, prompt: $loopPrompt)
            PlayerIconButton(title: "Rewind", systemImage: "backward.end.fill", size: 16) {
                model.rewind()
            }
            PlayerIconButton(title: model.isPlaying ? "Pause" : "Play",
                             systemImage: model.isPlaying ? "pause.fill" : "play.fill", size: 26, width: 40) {
                model.togglePlay()
            }
            .contentTransition(.symbolEffect(.replace))
        }
        .disabled(model.info == nil)
    }

    private var sound: some View {
        HStack(spacing: 4) {
            if SynthPlaybackClock.isBundled {
                TabAudioButton(model: model)
                    .disabled(!model.synthAvailable)
            }
            VideoAudioButton(model: model)
                .disabled(model.info == nil)
            if SynthPlaybackClock.isBundled {
                MetronomeButton(model: model)
                    .disabled(!model.synthAvailable)
            }
            SpeedMenu(model: model)
                .disabled(model.info == nil)
        }
    }
}

// MARK: - Now playing

/// The bar's middle, like Music's display: the title and artist (and the loop), over the scrubber.
private struct NowPlaying: View {
    let model: TabViewerModel

    var body: some View {
        VStack(spacing: 2) {
            HStack(spacing: 6) {
                caption
                    .font(.system(size: 12))
                    .lineLimit(1)
                    .truncationMode(.tail)
                if let loop = model.loop {
                    Text(TabViewer.loopLabel(loop))
                        .font(.system(size: 11, weight: .semibold))
                        .monospacedDigit()
                        .foregroundStyle(.orange)
                        .lineLimit(1)
                        .fixedSize()
                }
            }
            .frame(height: 16)

            HStack(spacing: 8) {
                Text(TabViewer.time(model.displayTime))
                    .frame(minWidth: 34, alignment: .trailing)
                Slider(
                    value: Binding(get: { min(model.displayTime, model.duration) }, set: { model.seek(to: $0) }),
                    in: 0...max(model.duration, 0.01)
                )
                .controlSize(.mini)
                .accessibilityLabel("Position")
                Text(TabViewer.time(model.duration))
                    .frame(minWidth: 34, alignment: .leading)
            }
            .font(.system(size: 11))
            .monospacedDigit()
            .foregroundStyle(.secondary)
        }
        .disabled(model.info == nil)
    }

    /// "**Title** - Artist": the title in medium weight, the artist regular.
    private var caption: Text {
        guard let info = model.info else { return Text(" ") }
        let title = Text(info.title).fontWeight(.medium)
        return info.artist.isEmpty ? title : Text("\(title) - \(info.artist)")
    }
}

// MARK: - Speed

private struct SpeedMenu: View {
    let model: TabViewerModel

    var body: some View {
        Menu {
            Picker("Speed", selection: Binding(get: { model.speed }, set: { model.setSpeed($0) })) {
                ForEach(model.speedOptions, id: \.self) { speed in
                    Text(TabViewer.percent(speed)).tag(speed)
                }
            }
            .pickerStyle(.inline)
        } label: {
            Label(TabViewer.percent(model.speed), systemImage: "gauge.with.dots.needle.33percent")
                .labelStyle(.titleAndIcon)
                .font(.system(size: 13, weight: .medium))
                .monospacedDigit()
                .padding(.horizontal, 8)
        }
        .menuStyle(.button)
        .buttonStyle(PlayerPillStyle(isOn: model.speed != 1))
        .menuIndicator(.hidden)
        .fixedSize()
        .help(model.usingVideoClock ? "Playback speed (YouTube's rates)" : "Playback speed")
        .accessibilityLabel("Speed")
    }
}

// MARK: - Building blocks

/// A borderless icon button for the player bar: highlights on hover, dims while pressed, and
/// tints with the accent colour while `isOn`.
struct PlayerIconButton: View {
    let title: String
    let systemImage: String
    var size: CGFloat = 15
    var width: CGFloat = 30
    var isOn = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: size, weight: .medium))
                .frame(width: width, height: 30)
                .contentShape(Rectangle())
        }
        .buttonStyle(PlayerPillStyle(isOn: isOn))
        .accessibilityLabel(title)
        .help(title)
    }
}

/// Plain content on a capsule that appears on hover (and in the accent colour while on), as
/// Music's bar draws its buttons.
struct PlayerPillStyle: ButtonStyle {
    var isOn = false

    func makeBody(configuration: Configuration) -> some View {
        PillBody(configuration: configuration, isOn: isOn)
    }

    private struct PillBody: View {
        let configuration: Configuration
        let isOn: Bool
        @State private var hovering = false
        @Environment(\.isEnabled) private var isEnabled

        var body: some View {
            let on = isOn && isEnabled
            configuration.label
                .frame(minHeight: 30)
                .padding(.horizontal, 2)
                .foregroundStyle(on ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.primary))
                .background {
                    Capsule().fill(PlayerPillStyle.fill(on: on, hovering: hovering && isEnabled))
                }
                .opacity(isEnabled ? (configuration.isPressed ? 0.6 : 1) : 0.35)
                .contentShape(Capsule())
                .onHover { hovering = $0 }
                .animation(.easeOut(duration: 0.12), value: hovering)
        }
    }

    static func fill(on: Bool, hovering: Bool) -> Color {
        if on { return Color.accentColor.opacity(hovering ? 0.22 : 0.15) }
        return Color.primary.opacity(hovering ? 0.08 : 0)
    }
}
