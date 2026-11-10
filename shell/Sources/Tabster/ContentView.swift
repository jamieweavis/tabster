import SwiftUI

/// The main window: tab table, and the open tab.
struct ContentView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            if let core = model.core {
                LibraryView(core: core)
            } else {
                ContentUnavailableView {
                    Label("Tabster Couldn't Start", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(model.startupError ?? "The library couldn't be opened.")
                } actions: {
                    Button("Quit Tabster") { NSApp.terminate(nil) }
                }
                .frame(minWidth: LibraryView.detailMinWidth)
            }
        }
        .frame(minHeight: 480)
    }
}

struct LibraryView: View {
    let core: Core
    @Environment(AppModel.self) private var model
    @State private var isDropTargeted = false
    /// Whether the tab list is shown beside the viewer (the toolbar's sidebar button).
    @State private var columnVisibility = NavigationSplitViewVisibility.all
    private var listHidden: Bool { columnVisibility == .detailOnly }
    /// Shared with `TabViewer`'s toggle: with the player bar hidden, its title and artist head the window.
    @AppStorage("viewer.showPlayer") private var showPlayer = true

    /// The tab pane's and the list's minimum widths. The window's minimum is set from them
    /// explicitly: left to the split view, the window shrinks past the list's share and both
    /// panes are cropped.
    static let detailMinWidth: CGFloat = 760
    static let listMinWidth: CGFloat = 240
    /// The list's current width (with the divider). The split view doesn't narrow the list as the
    /// window shrinks, so the window's minimum keeps room for all of it.
    @State private var listWidth = listMinWidth

    /// The open tab's title and artist, for the header while the player bar is hidden.
    private var headerCaption: OpenTabCaption? {
        guard !showPlayer, let open = model.openTab, open.tabID == model.selection else { return nil }
        return open
    }

    var body: some View {
        @Bindable var model = model
        NavigationSplitView(columnVisibility: $columnVisibility) {
            TabTable()
                .navigationSplitViewColumnWidth(min: Self.listMinWidth, ideal: 600)
                .toolbar {
                    // Heads the list; goes with it when it's hidden (the window's title takes over).
                    if !listHidden {
                        ToolbarItem { AppTitle() }
                            .sharedBackgroundVisibility(.hidden)
                        ToolbarSpacer(.flexible)
                    }
                }
        } detail: {
            Group {
                if let id = model.selection {
                    TabViewer(core: core, tabID: id)
                        .id(id)
                } else {
                    ContentUnavailableView(
                        "No Tab Selected",
                        systemImage: "music.note.list",
                        description: Text("Choose a tab from the library to open it.")
                    )
                    // "Tabster" is the title, over the list or centred above everything.
                    .toolbar(removing: .title)
                }
            }
            .frame(minWidth: Self.detailMinWidth)
        }
        .frame(minWidth: listHidden ? Self.detailMinWidth : Self.detailMinWidth + max(listWidth, Self.listMinWidth))
        // Hiding the list shrinks the window instead of widening the tab pane.
        .background(SidebarWindowResizer(sidebarHidden: listHidden) { listWidth = $0 })
        .toolbar {
            // Centred: "Tabster" (list hidden) over the open tab's title and artist (player hidden).
            if listHidden || headerCaption != nil {
                ToolbarItem(placement: .principal) {
                    VStack(spacing: 0) {
                        if listHidden { AppTitle() }
                        if let caption = headerCaption { TabCaption(caption: caption, compact: listHidden) }
                    }
                }
                .sharedBackgroundVisibility(.hidden)
            }
            if let progress = model.importProgress {
                ToolbarItem(placement: .status) {
                    ImportProgressView(progress: progress)
                }
            }
        }
        .dropDestination(for: URL.self) { urls, _ in
            let files = urls.filter(\.isFileURL)
            model.importURLs(files)
            return !files.isEmpty
        } isTargeted: { isDropTargeted = $0 }
        .overlay {
            if isDropTargeted {
                RoundedRectangle(cornerRadius: 12)
                    .strokeBorder(Color.accentColor, lineWidth: 3)
                    .padding(4)
                    .allowsHitTesting(false)
            }
        }
        .sheet(isPresented: Binding(
            get: { !model.importFailures.isEmpty },
            set: { if !$0 { model.importFailures = [] } }
        )) {
            ImportFailuresSheet(failures: model.importFailures)
        }
        .alert(
            "Something Went Wrong",
            isPresented: Binding(get: { model.errorMessage != nil }, set: { if !$0 { model.errorMessage = nil } }),
            presenting: model.errorMessage
        ) { _ in
            Button("OK") {}
        } message: { message in
            Text(message)
        }
    }
}

/// "**Title** - Artist", like the player bar's; smaller when it sits under "Tabster".
private struct TabCaption: View {
    let caption: OpenTabCaption
    let compact: Bool

    var body: some View {
        let title = Text(caption.title).fontWeight(.medium)
        (caption.artist.isEmpty ? title : Text("\(title) - \(caption.artist)"))
            .font(.system(size: compact ? 11 : 13))
            .foregroundStyle(compact ? .secondary : .primary)
            .lineLimit(1)
            .truncationMode(.tail)
            .frame(maxWidth: 420)
            .padding(.horizontal, 6)
    }
}

/// "Tabster", heading the window's toolbar.
private struct AppTitle: View {
    var body: some View {
        Text("Tabster")
            .font(.title3.weight(.semibold))
            .padding(.horizontal, 6)
    }
}
