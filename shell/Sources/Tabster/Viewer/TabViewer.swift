import SwiftUI

/// The tab viewer for one open tab: renders the tab, transport, track picker, (later) video.
/// Contract between the library UI and the viewer: `TabViewer(core:tabID:)` is the detail view
/// (embed it with `.id(tabID)` so switching tabs makes a fresh viewer).
struct TabViewer: View {
    let core: Core
    let tabID: Int64
    @State private var model: TabViewerModel
    /// Where the video sits: "top" or "right" (remembered for the app).
    @AppStorage("viewer.videoPlacement") private var videoPlacement = "top"

    init(core: Core, tabID: Int64) {
        self.core = core
        self.tabID = tabID
        _model = State(initialValue: TabViewerModel(core: core, tabID: tabID))
    }

    /// Debug harness: a viewer whose model starts playing once laid out.
    init(model: TabViewerModel) {
        core = model.core
        tabID = model.tabID
        _model = State(initialValue: model)
    }

    var body: some View {
        VStack(spacing: 0) {
            if case let .failed(message) = model.phase {
                ContentUnavailableView("Couldn't Open Tab", systemImage: "exclamationmark.triangle",
                                       description: Text(message))
            } else {
                if model.showVideo, model.selectedVideo != nil {
                    if videoPlacement == "right" {
                        HSplitView {
                            canvas.frame(minWidth: 320)
                            VideoPane(model: model, beside: true)
                                .frame(minWidth: 280, idealWidth: 440, maxWidth: 900)
                        }
                    } else {
                        VSplitView {
                            VideoPane(model: model)
                                .frame(minHeight: 140, idealHeight: model.syncEditing ? 440 : 320, maxHeight: 760)
                            canvas.frame(minHeight: 160)
                        }
                    }
                } else {
                    canvas
                }
                Divider()
                TransportBar(model: model)
                    .disabled(model.info == nil)
            }
        }
        .frame(minWidth: 480, minHeight: 320)
        .background {
            // ⌘+ (shifted =) and ⌘0 without visible buttons; T / ⇧T taps while syncing (as key
            // equivalents, so they work even when the YouTube player has focus).
            Group {
                Button("Zoom In") { model.zoom(by: 0.1) }.keyboardShortcut("+", modifiers: .command)
                Button("Actual Size") { model.resetZoom() }.keyboardShortcut("0", modifiers: .command)
                if model.syncEditing {
                    Button("Tap") { model.tapHere() }.keyboardShortcut("t", modifiers: [])
                    Button("Tap Bar 1") { model.tapBarOne() }.keyboardShortcut("t", modifiers: .shift)
                }
            }
            .opacity(0)
            .frame(width: 0, height: 0)
            .accessibilityHidden(true)
        }
        .background(Color(nsColor: .windowBackgroundColor))
        .navigationTitle(model.info?.title ?? "")
        .navigationSubtitle(model.info?.artist ?? "")
        .toolbar { ViewerToolbar(model: model, videoPlacement: $videoPlacement) }
        .sheet(isPresented: Binding(get: { model.showAttachSheet }, set: { model.showAttachSheet = $0 })) {
            VideoAttachSheet(onAttach: { id, label in model.addVideo(youtubeID: id, label: label) })
        }
        .sheet(isPresented: Binding(get: { model.showPointsSheet }, set: { model.showPointsSheet = $0 })) {
            BarPointsSheet(videoID: model.selectedVideo?.youtubeId,
                                 playedBarCount: Int(model.info?.playedBarCount ?? 0)) { points in
                model.importBarPoints(points)
            }
        }
        .task { await model.open() }
        .onDisappear { model.close() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.willTerminateNotification)) { _ in
            model.close(synchronously: true)
        }
    }

    /// The canvas exists while loading too, so its width is known by the time the session opens
    /// and the first layout can start immediately.
    private var canvas: some View {
        TabCanvasView(model: model)
            .overlay {
                if !model.hasLayout { ProgressView() }
            }
    }
}

// MARK: - Toolbar

private struct ViewerToolbar: ToolbarContent {
    let model: TabViewerModel
    @Binding var videoPlacement: String

    var body: some ToolbarContent {
        ToolbarItemGroup(placement: .primaryAction) {
            if let info = model.info, info.tracks.count > 0 {
                // A borderless menu wrapped in a stack: toolbar pickers/menus otherwise render
                // icon-only or with a blank title.
                HStack {
                    Menu {
                        Picker("Track", selection: Binding(get: { model.track }, set: { model.setTrack($0) })) {
                            ForEach(info.tracks, id: \.index) { track in
                                Text(TabViewer.trackLabel(track)).tag(track.index)
                            }
                        }
                        .pickerStyle(.inline)
                    } label: {
                        Text(model.currentTrack.map(TabViewer.trackLabel) ?? "Track")
                    }
                    .menuStyle(.borderlessButton)
                    .fixedSize()
                }
                .padding(.horizontal, 6)
                .help("Track")
            }

            if let loop = model.loop {
                Button {
                    model.setLoop(nil)
                } label: {
                    Text("\(Image(systemName: "repeat")) \(TabViewer.loopLabel(loop))")
                }
                .tint(.orange)
                .help("Looping — click (or press \\) to clear")
            }

            HStack {
                Menu {
                    Picker("Speed", selection: Binding(get: { model.speed }, set: { model.setSpeed($0) })) {
                        ForEach(model.speedOptions, id: \.self) { speed in
                            Text(TabViewer.percent(speed)).tag(speed)
                        }
                    }
                    .pickerStyle(.inline)
                } label: {
                    Text(TabViewer.percent(model.speed)).monospacedDigit()
                }
                .menuStyle(.borderlessButton)
                .fixedSize()
            }
            .padding(.horizontal, 6)
            .help(model.usingVideoClock ? "Playback speed (YouTube's rates)" : "Playback speed")

            if model.synthAvailable {
                ControlGroup {
                    Toggle(isOn: Binding(get: { model.tabAudio }, set: { model.tabAudio = $0 })) {
                        Label("Tab Audio", systemImage: model.tabAudio ? "speaker.wave.2" : "speaker.slash")
                    }
                    .help(model.usingVideoClock ? "Tab audio plays when there's no video" : "Hear the tab")
                    Toggle(isOn: Binding(get: { model.metronome }, set: { model.metronome = $0 })) {
                        Label("Metronome", systemImage: "metronome")
                    }
                    .help("Metronome")
                    .disabled(!model.tabAudio || model.usingVideoClock)
                }
            }

            ControlGroup {
                Button("Zoom Out", systemImage: "minus.magnifyingglass") { model.zoom(by: -0.1) }
                    .keyboardShortcut("-", modifiers: .command)
                Button("Zoom In", systemImage: "plus.magnifyingglass") { model.zoom(by: 0.1) }
                    .keyboardShortcut("=", modifiers: .command)
            }
            .help("Zoom (⌘+ / ⌘- / ⌘0)")

            Toggle(isOn: Binding(get: { model.showRhythm }, set: { model.setShowRhythm($0) })) {
                Label("Rhythm", systemImage: "music.note")
            }
            .help("Show rhythm")

            if model.showVideo, model.selectedVideo != nil {
                Toggle(isOn: Binding(get: { model.syncEditing }, set: { model.syncEditing = $0 })) {
                    Label("Sync", systemImage: "metronome")
                }
                .help("Sync the tab to the video")
            }

            if model.videos.isEmpty {
                Button("Add YouTube Video…", systemImage: "play.rectangle") { model.showAttachSheet = true }
                    .help("Add YouTube video…")
            } else {
                Menu {
                    Toggle("Show Video", isOn: Binding(get: { model.showVideo }, set: { model.setVideoVisible($0) }))
                    Picker("Position", selection: $videoPlacement) {
                        Text("Video Above Tab").tag("top")
                        Text("Video Beside Tab").tag("right")
                    }
                    .pickerStyle(.inline)
                    Divider()
                    VideoPicker(
                        videos: model.videos,
                        selectedID: model.selectedVideoID,
                        onSelect: { video in
                            model.setVideoVisible(true)
                            model.select(video)
                        },
                        onRemove: { model.removeVideo($0) },
                        onSetDefault: { model.makeDefault($0) },
                        onAdd: { model.showAttachSheet = true }
                    )
                } label: {
                    Label("Video", systemImage: model.showVideo ? "play.rectangle.fill" : "play.rectangle")
                } primaryAction: {
                    model.setVideoVisible(!model.showVideo)
                }
                .help("Show or hide the video (click), choose a video (hold)")
            }
        }
    }
}

// MARK: - Transport

private struct TransportBar: View {
    let model: TabViewerModel

    var body: some View {
        HStack(spacing: 12) {
            HStack(spacing: 4) {
                Button("Previous Bar", systemImage: "backward.end.fill") { model.previousBar() }
                Button(model.isPlaying ? "Pause" : "Play", systemImage: model.isPlaying ? "pause.fill" : "play.fill") {
                    model.togglePlay()
                }
                .font(.title2)
                .frame(width: 32)
                Button("Next Bar", systemImage: "forward.end.fill") { model.nextBar() }
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)

            Text(TabViewer.time(model.displayTime))
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .frame(width: 44, alignment: .trailing)
            Slider(
                value: Binding(get: { min(model.displayTime, model.duration) }, set: { model.seek(to: $0) }),
                in: 0...max(model.duration, 0.01)
            )
            .controlSize(.small)
            Text(TabViewer.time(model.duration))
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .frame(width: 44, alignment: .leading)

            if let sections = model.info?.sections, !sections.isEmpty {
                Menu {
                    ForEach(Array(sections.enumerated()), id: \.offset) { _, section in
                        Button("\(section.label)  (bar \(section.masterBar + 1))") { model.seek(to: section.scoreSecs) }
                    }
                } label: {
                    Label("Sections", systemImage: "list.bullet")
                }
                .menuStyle(.borderlessButton)
                .fixedSize()
                .help("Jump to section")
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }
}

extension TabViewer {
    /// "Lead Guitar · E A D G B E" (tuning lowest string first, like a guitarist reads it).
    static func trackLabel(_ track: TrackInfo) -> String {
        var label = track.name.isEmpty ? "Track \(track.index + 1)" : track.name
        if !track.isPercussion, !track.tuning.isEmpty {
            label += " · " + track.tuning.reversed().joined(separator: " ")
        }
        if track.capo > 0 { label += " · capo \(track.capo)" }
        return label
    }

    static func loopLabel(_ loop: ClosedRange<UInt32>) -> String {
        loop.lowerBound == loop.upperBound ? "Bar \(loop.lowerBound + 1)" : "Bars \(loop.lowerBound + 1)–\(loop.upperBound + 1)"
    }

    static func percent(_ speed: Double) -> String { "\(Int((speed * 100).rounded()))%" }

    static func time(_ secs: Double) -> String {
        let s = Int(max(0, secs).rounded(.down))
        return String(format: "%d:%02d", s / 60, s % 60)
    }
}
