import SwiftUI

/// The tab viewer for one open tab: renders the tab, transport, track picker, (later) video.
/// Contract between the library UI and the viewer: `TabViewer(core:tabID:)` is the detail view
/// (embed it with `.id(tabID)` so switching tabs makes a fresh viewer).
struct TabViewer: View {
    let core: Core
    let tabID: Int64
    @State private var model: TabViewerModel
    @State private var loopPrompt: LoopPrompt?
    /// Pin the player bar on screen (the default); off, it slides away and comes back while the
    /// pointer is near the bottom of the tab.
    @AppStorage("viewer.showPlayer") private var showPlayer = true
    /// Pointer within `revealZone` of the canvas's bottom edge (only used while unpinned).
    @State private var pointerNearBottom = false
    @State private var hideTask: Task<Void, Never>?
    @State private var canvasHeight: CGFloat = 0
    /// Optional: the debug harness shows a viewer without the app model.
    @Environment(AppModel.self) private var app: AppModel?

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
                if model.videos.isEmpty, let suggestion = model.videoSuggestion {
                    VideoSuggestionBar(
                        match: suggestion,
                        onUse: { model.attachVideo(suggestion.candidate, label: suggestion.label) },
                        onBrowse: { model.showVideoFinder = true },
                        onDismiss: { model.dismissVideoSuggestion() }
                    )
                    Divider()
                }
                if model.showsSyncBar {
                    SyncPane(model: model)
                    Divider()
                }
                canvas
                    .background {
                        // The video is never shown, only heard: its web view stays in the
                        // window, full size but invisible, behind the tab, as clock and audio.
                        if model.selectedVideo != nil, let player = model.player {
                            YouTubePlayerView(controller: player)
                                .frame(width: 356, height: 200)
                                .opacity(0)
                                .allowsHitTesting(false)
                                .accessibilityHidden(true)
                        }
                    }
            }
        }
        .frame(minWidth: 480, minHeight: 320)
        .background {
            // Zoom (⌘= / ⌘+ / ⌘- / ⌘0) without visible buttons; T / ⇧T taps while syncing (as key
            // equivalents, so they work even when the YouTube player has focus).
            Group {
                Button("Zoom In") { model.zoom(by: 0.1) }.keyboardShortcut("+", modifiers: .command)
                Button("Zoom In") { model.zoom(by: 0.1) }.keyboardShortcut("=", modifiers: .command)
                Button("Zoom Out") { model.zoom(by: -0.1) }.keyboardShortcut("-", modifiers: .command)
                Button("Actual Size") { model.resetZoom() }.keyboardShortcut("0", modifiers: .command)
                if model.showsSyncBar {
                    Button("Tap") { model.tapHere() }.keyboardShortcut("t", modifiers: [])
                    Button("Tap Bar 1") { model.tapBarOne() }.keyboardShortcut("t", modifiers: .shift)
                }
            }
            .opacity(0)
            .frame(width: 0, height: 0)
            .accessibilityHidden(true)
        }
        .background(Color(nsColor: .windowBackgroundColor))
        // The window keeps this for the Window menu; the toolbar draws its own "Tabster" (the
        // system title would sit after the leading items and could clash with the centred heading).
        .navigationTitle("Tabster")
        .toolbar(removing: .title)
        .toolbar { ViewerToolbar(model: model, app: app, showPlayer: $showPlayer) }
        .modifier(LoopPromptModifier(model: model, prompt: $loopPrompt))
        .sheet(isPresented: Binding(get: { model.showVideoFinder }, set: { model.showVideoFinder = $0 })) {
            VideoFinderSheet(
                core: core,
                tabID: tabID,
                onAttachMatch: { video, label in model.attachVideo(video, label: label) },
                onAttachID: { id, label in model.addVideo(youtubeID: id, label: label) }
            )
        }
        .sheet(isPresented: Binding(get: { model.showPointsSheet }, set: { model.showPointsSheet = $0 })) {
            BarPointsSheet(videoID: model.selectedVideo?.youtubeId,
                                 playedBarCount: Int(model.info?.playedBarCount ?? 0)) { points in
                model.importBarPoints(points)
            }
        }
        .task { await model.open() }
        // This tab's turn in a bulk sync: listen to its video, then hand back to the library.
        .task(id: bulkSyncTurn) {
            guard bulkSyncTurn != nil, let outcome = await model.syncForBulk(), !Task.isCancelled else { return }
            app?.bulkSyncFinished(tab: tabID, outcome: outcome)
        }
        // Asked to sync by ear (the guided import's last step): listen once, with the sync bar
        // up for its progress and Stop & Sync. Cleared when done or when the viewer closes.
        .task(id: listenToken) {
            guard let token = listenToken else { return }
            model.syncEditing = true
            _ = await model.syncForBulk()
            if app?.listenRequest?.token == token { app?.listenRequest = nil }
        }
        .onChange(of: app?.config.showRhythm) { _, on in
            if let on { model.setShowRhythm(on) }
        }
        .onChange(of: app?.config.showSections) { _, on in
            if let on { model.setShowSections(on) }
        }
        .onChange(of: app?.config.showTempo) { _, on in
            if let on { model.setShowTempo(on) }
        }
        .onChange(of: app?.config.showBarNumbers) { _, on in
            if let on { model.setShowBarNumbers(on) }
        }
        .onChange(of: app?.config.showAnnotations) { _, on in
            if let on { model.setShowAnnotations(on) }
        }
        .onChange(of: app?.config.barsPerRow) { _, count in
            if let count { model.setBarsPerRow(count) }
        }
        .onChange(of: app?.config.followCursor) { _, on in
            if let on { model.followCursor = on }
        }
        .onDisappear {
            model.close()
        }
        .onReceive(NotificationCenter.default.publisher(for: .videoSearchFinished)) { note in
            if note.object as? Int64 == tabID { model.videoSearchFinished() }
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.willTerminateNotification)) { _ in
            model.close(synchronously: true)
        }
    }

    /// This tab's place in the bulk sync in progress, while it's the one being listened to.
    private var bulkSyncTurn: Int? {
        guard let run = app?.bulkSync, run.current == tabID else { return nil }
        return run.index
    }

    /// The pending "sync by ear" request for this tab.
    private var listenToken: UUID? {
        guard let request = app?.listenRequest, request.tab == tabID else { return nil }
        return request.token
    }

    /// The canvas exists while loading too, so its width is known by the time the session opens
    /// and the first layout can start immediately.
    private var canvas: some View {
        TabCanvasView(model: model, reservesPlayerSpace: showPlayer)
            .overlay {
                if !model.hasLayout { ProgressView() }
            }
            .overlay(alignment: .bottom) {
                PlayerBar(model: model, loopPrompt: $loopPrompt)
                    .frame(maxWidth: 980)
                    .padding(.horizontal, 16)
                    .padding(.bottom, 14)
                    .offset(y: playerVisible ? 0 : Self.hiddenOffset)
                    .opacity(playerVisible ? 1 : 0)
                    .allowsHitTesting(playerVisible)
                    .animation(.spring(duration: 0.35, bounce: 0.1), value: playerVisible)
            }
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { canvasHeight = $0 }
            .onContinuousHover(coordinateSpace: .local) { phase in
                switch phase {
                case let .active(point): setPointerNearBottom(point.y >= canvasHeight - Self.revealZone)
                case .ended: setPointerNearBottom(false)
                }
            }
    }

    /// How close to the bottom edge the pointer brings the hidden bar back (a little taller than
    /// the bar and its margin).
    private static let revealZone: CGFloat = 100
    /// Far enough to slide the bar (60pt, 14pt margin) and its glass shadow off the bottom.
    private static let hiddenOffset: CGFloat = 100

    /// Shown when pinned, while the pointer is near the bottom, and while a loop prompt is open
    /// (its popover hangs off the bar).
    private var playerVisible: Bool {
        showPlayer || pointerNearBottom || loopPrompt != nil
    }

    /// Showing is immediate; hiding waits a moment so brushing past the edge doesn't flicker it.
    private func setPointerNearBottom(_ near: Bool) {
        if near {
            hideTask?.cancel()
            hideTask = nil
            if !pointerNearBottom { pointerNearBottom = true }
        } else if pointerNearBottom, hideTask == nil {
            hideTask = Task {
                try? await Task.sleep(for: .milliseconds(600))
                guard !Task.isCancelled else { return }
                pointerNearBottom = false
                hideTask = nil
            }
        }
    }
}

// MARK: - Toolbar

/// The song over its artist, centred in the toolbar while the player bar is hidden.
private struct SongHeading: View {
    let title: String
    let artist: String

    var body: some View {
        VStack(spacing: 0) {
            Text(title)
                .font(.system(size: 13, weight: .semibold))
            if !artist.isEmpty {
                Text(artist)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
        }
        .multilineTextAlignment(.center)
        .lineLimit(1)
        .truncationMode(.tail)
        .frame(maxWidth: 320)
        .padding(.horizontal, 6)
    }
}

private struct ViewerToolbar: ToolbarContent {
    let model: TabViewerModel
    /// Nil in the debug harness, where the toggle only affects this viewer.
    let app: AppModel?
    @Binding var showPlayer: Bool

    /// The toolbar's items never change while tabs open and load: every control is there from
    /// the first frame (disabled until the tab has loaded). Adding, removing or swapping an item
    /// makes SwiftUI rebuild the group and AppKit re-tile the toolbar and animate it into place,
    /// which was most of a tab switch's main-thread time and showed as the toolbar filling in in
    /// chunks. (So the loop controls, and the indicator that comes and goes, live in the player bar.) One
    /// group rather than an item each: separate items are each a glass platter, and cost more.
    var body: some ToolbarContent {
        ToolbarItem(placement: .navigation) {
            Text("Tabster")
                .font(.system(size: 15, weight: .bold))
                .padding(.horizontal, 6)
        }
        .sharedBackgroundVisibility(.hidden)
        // Centred, with the player bar hidden. Always present (empty with the bar shown): without a
        // principal item the trailing items below pack to the left after "Tabster".
        ToolbarItem(placement: .principal) {
            if showPlayer {
                Color.clear.frame(width: 1, height: 1)
            } else {
                SongHeading(title: model.info?.title ?? "", artist: model.info?.artist ?? "")
            }
        }
        .sharedBackgroundVisibility(.hidden)
        // All trailing, in this order: with the centred heading present, default-placed items
        // before it would otherwise split off to its left.
        // Track and sections share one platter, split by a hairline like the zoom buttons'.
        ToolbarItem(placement: .primaryAction) {
            HStack(spacing: 0) {
                trackMenu
                Divider()
                    .frame(height: 16)
                sectionsMenu
            }
        }
        ToolbarItem(placement: .primaryAction) {
            syncToggle
        }
        ToolbarItem(placement: .primaryAction) {
            playerToggle
        }
        ToolbarItem(placement: .primaryAction) {
            followToggle
        }
        ToolbarItem(placement: .primaryAction) {
            rhythmToggle
        }
        ToolbarItem(placement: .primaryAction) {
            sectionsToggle
        }
        ToolbarItem(placement: .primaryAction) {
            tempoToggle
        }
        ToolbarItem(placement: .primaryAction) {
            barNumbersToggle
        }
        ToolbarItem(placement: .primaryAction) {
            annotationsToggle
        }
        ToolbarItem(placement: .primaryAction) {
            barDisplayMenu
        }
        ToolbarItem(placement: .primaryAction) {
            zoomButtons
        }
    }

    /// One split control; the shortcuts themselves live on `TabViewer`'s hidden buttons (which
    /// also catch ⌘+ and ⌘0).
    private var zoomButtons: some View {
        ControlGroup {
            Button("Zoom Out", systemImage: "minus.magnifyingglass") { model.zoom(by: -0.1) }
                .disabled(model.scale <= TabViewerModel.scaleRange.lowerBound)
            Button("Zoom In", systemImage: "plus.magnifyingglass") { model.zoom(by: 0.1) }
                .disabled(model.scale >= TabViewerModel.scaleRange.upperBound)
        }
        .controlGroupStyle(.navigation)
        .help("Zoom (⌘+ / ⌘- / ⌘0)")
    }

    /// Shows the sync bar (listen, tap, nudge). Needs a video playing to sync to.
    private var syncToggle: some View {
        Toggle(isOn: Binding(get: { model.showsSyncBar }, set: { model.syncEditing = $0 })) {
            Label("Sync to Video", systemImage: "ear")
        }
        .toggleStyle(.button)
        .help(model.hearingVideo ? "Show the sync bar: line the tab up with its video" : "Turn on a video to sync the tab to it")
        .disabled(!model.hearingVideo)
    }

    /// Pins the player bar; off, it hides until the pointer nears the bottom of the tab.
    private var playerToggle: some View {
        Toggle(isOn: $showPlayer) {
            Label("Show Player", systemImage: "rectangle.bottomthird.inset.filled")
        }
        .toggleStyle(.button)
        .help("Always show the player (otherwise it appears when the pointer nears the bottom)")
    }

    /// Writes Preferences (`Config::follow_cursor`), so it stays in step with the Settings toggle;
    /// `TabViewer` relays the change to the model.
    private var followToggle: some View {
        Toggle(isOn: Binding(
            get: { app?.config.followCursor ?? model.followCursor },
            set: { on in
                if let app {
                    var config = app.config
                    config.followCursor = on
                    app.updateConfig(config)
                } else {
                    model.followCursor = on
                }
                if on, model.isPlaying { model.refresh(follow: true) }
            }
        )) {
            Label("Follow Playhead", systemImage: "arrow.right.and.line.vertical.and.arrow.left")
        }
        .toggleStyle(.button)
        .help("Scroll to follow the playhead while playing")
    }

    private var rhythmToggle: some View {
        configToggle("Show Rhythm", systemImage: "music.note", help: "Show rhythm notation",
                     key: \.showRhythm, local: model.showRhythm, setLocal: model.setShowRhythm)
    }

    private var sectionsToggle: some View {
        configToggle("Show Sections", systemImage: "flag", help: "Show section labels (Intro, Verse, Chorus …)",
                     key: \.showSections, local: model.showSections, setLocal: model.setShowSections)
    }

    private var tempoToggle: some View {
        configToggle("Show Tempo", systemImage: "metronome", help: "Show tempo markings and changes",
                     key: \.showTempo, local: model.showTempo, setLocal: model.setShowTempo)
    }

    private var barNumbersToggle: some View {
        configToggle("Show Bar Numbers", systemImage: "number", help: "Show bar numbers",
                     key: \.showBarNumbers, local: model.showBarNumbers, setLocal: model.setShowBarNumbers)
    }

    private var annotationsToggle: some View {
        configToggle("Show Text Notes", systemImage: "text.bubble", help: "Show text notes written on the tab",
                     key: \.showAnnotations, local: model.showAnnotations, setLocal: model.setShowAnnotations)
    }

    /// A toolbar toggle that writes Preferences, so it stays in step with the Settings toggle;
    /// `TabViewer` relays the change to the model. In the debug harness (no app) it sets the model.
    private func configToggle(
        _ title: String, systemImage: String, help: String,
        key: WritableKeyPath<Config, Bool>, local: Bool, setLocal: @escaping (Bool) -> Void
    ) -> some View {
        Toggle(isOn: Binding(
            get: { app?.config[keyPath: key] ?? local },
            set: { on in
                if let app {
                    var config = app.config
                    config[keyPath: key] = on
                    app.updateConfig(config)
                } else {
                    setLocal(on)
                }
            }
        )) {
            Label(title, systemImage: systemImage)
        }
        .toggleStyle(.button)
        .help(help)
    }

    /// Always present (disabled when the tab has no sections) so the toolbar never re-tiles.
    private var sectionsMenu: some View {
        let sections = model.info?.sections ?? []
        return HStack {
            Menu {
                ForEach(Array(sections.enumerated()), id: \.offset) { _, section in
                    Button("\(section.label)  (bar \(section.masterBar + 1))") { model.seek(to: section.scoreSecs) }
                }
                Divider()
                Button("Loop Section", systemImage: "repeat") { model.loopCurrentSection() }
            } label: {
                Label("Sections", systemImage: "list.bullet")
                    .labelStyle(.iconOnly)
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
        }
        .padding(.horizontal, 6)
        .help("Jump to or loop a section")
        .disabled(sections.isEmpty)
    }

    /// Bars per row: Auto (as many as fit) or a fixed count. Writes Preferences, like the toggles.
    private var barDisplayMenu: some View {
        let current = app?.config.barsPerRow ?? model.barsPerRow
        return Menu {
                Picker("Bar Display", selection: Binding(
                    get: { current },
                    set: { count in
                        if let app {
                            var config = app.config
                            config.barsPerRow = count
                            app.updateConfig(config)
                        } else {
                            model.setBarsPerRow(count)
                        }
                    }
                )) {
                    ForEach(TabViewer.barsPerRowChoices, id: \.self) { count in
                        Text(TabViewer.barsPerRowLabel(count)).tag(count)
                    }
                }
                .pickerStyle(.inline)
            } label: {
                Label("Bar Display", systemImage: "rectangle.split.3x1")
            }
            .help("Bar display: bars per row (\(TabViewer.barsPerRowLabel(current)))")
    }

    private var trackMenu: some View {
        // A borderless menu wrapped in a stack: toolbar pickers/menus otherwise render
        // icon-only or with a blank title.
        HStack {
            Menu {
                if let info = model.info {
                    Picker("Track", selection: Binding(get: { model.track }, set: { model.setTrack($0) })) {
                        ForEach(info.tracks, id: \.index) { track in
                            Text(TabViewer.trackLabel(track)).tag(track.index)
                        }
                    }
                    .pickerStyle(.inline)
                }
            } label: {
                Text(model.currentTrack.map(TabViewer.trackLabel) ?? "Track")
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
        }
        .padding(.horizontal, 6)
        .help("Track")
        .disabled((model.info?.tracks.count ?? 0) == 0)
    }
}

extension TabViewer {
    /// The Bar Display choices: 0 = Auto.
    static let barsPerRowChoices: [UInt32] = [0, 1, 2, 3, 4, 5, 6, 8]

    static func barsPerRowLabel(_ count: UInt32) -> String {
        switch count {
        case 0: "Auto"
        case 1: "1 Bar per Row"
        default: "\(count) Bars per Row"
        }
    }

    /// Built in the core, e.g. "Bass · E A D G" or "Guitar 2 · E A D G B E · capo 2".
    static func trackLabel(_ track: TrackInfo) -> String { track.label }

    static func loopLabel(_ loop: ClosedRange<UInt32>) -> String {
        loop.lowerBound == loop.upperBound ? "Bar \(loop.lowerBound + 1)" : "Bars \(loop.lowerBound + 1)–\(loop.upperBound + 1)"
    }

    static func percent(_ speed: Double) -> String { "\(Int((speed * 100).rounded()))%" }

    static func time(_ secs: Double) -> String {
        let s = Int(max(0, secs).rounded(.down))
        return String(format: "%d:%02d", s / 60, s % 60)
    }
}
