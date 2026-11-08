import SwiftUI

/// The tab viewer for one open tab: renders the tab, transport, track picker, (later) video.
/// Contract between the library UI and the viewer: `TabViewer(core:tabID:)` is the detail view
/// (embed it with `.id(tabID)` so switching tabs makes a fresh viewer).
struct TabViewer: View {
    let core: Core
    let tabID: Int64
    @State private var model: TabViewerModel

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
                if model.showVideo {
                    VideoPane()
                    Divider()
                }
                // The canvas exists while loading too, so its width is known by the time the
                // session opens and the first layout can start immediately.
                TabCanvasView(model: model)
                    .overlay {
                        if !model.hasLayout { ProgressView() }
                    }
                Divider()
                TransportBar(model: model)
                    .disabled(model.info == nil)
            }
        }
        .frame(minWidth: 480, minHeight: 320)
        .background {
            // ⌘+ (shifted =) and ⌘0 without visible buttons.
            Group {
                Button("Zoom In") { model.zoom(by: 0.1) }.keyboardShortcut("+", modifiers: .command)
                Button("Actual Size") { model.resetZoom() }.keyboardShortcut("0", modifiers: .command)
            }
            .opacity(0)
            .frame(width: 0, height: 0)
            .accessibilityHidden(true)
        }
        .background(Color(nsColor: .windowBackgroundColor))
        .navigationTitle(model.info?.title ?? "")
        .navigationSubtitle(model.info?.artist ?? "")
        .toolbar { ViewerToolbar(model: model) }
        .task { await model.open() }
        .onDisappear { model.close() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.willTerminateNotification)) { _ in
            model.close(synchronously: true)
        }
    }
}

// MARK: - Toolbar

private struct ViewerToolbar: ToolbarContent {
    let model: TabViewerModel

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
                        ForEach(TabViewerModel.speeds, id: \.self) { speed in
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
            .help("Playback speed")

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

            Toggle(isOn: Binding(get: { model.showVideo }, set: { model.showVideo = $0 })) {
                Label("Video", systemImage: "play.rectangle")
            }
            .help("Show video")
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
