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

    /// The tab pane's and the list's minimum widths. The window's minimum is set from them
    /// explicitly: left to the split view, the window shrinks past the list's share and both
    /// panes are cropped.
    static let detailMinWidth: CGFloat = 760
    static let listMinWidth: CGFloat = 240
    /// The list's current width (with the divider). The split view doesn't narrow the list as the
    /// window shrinks, so the window's minimum keeps room for all of it.
    @State private var listWidth = listMinWidth

    var body: some View {
        @Bindable var model = model
        NavigationSplitView(columnVisibility: $columnVisibility) {
            TabTable()
                // A hairline against the tab pane, like the one above the status bar.
                .overlay(alignment: .trailing) {
                    HStack(spacing: 0) { Divider() }
                        .ignoresSafeArea()
                }
                .navigationSplitViewColumnWidth(min: Self.listMinWidth, ideal: 600)
                .toolbar {
                    if !listHidden {
                        ToolbarSpacer(.flexible)
                        // "New thing" at the list's top right, like Mail's Compose.
                        ToolbarItem {
                            Button("Import Tab", systemImage: "plus") { model.beginImport(ImportPanel.run()) }
                                .help("Import a tab and find its video (⌘O)")
                        }
                        // Between Import and the sidebar button (Tahoe puts that at the list's trailing edge).
                        ToolbarItem { BulkSyncButton(app: model) }
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
                }
            }
            .frame(minWidth: Self.detailMinWidth)
        }
        .frame(minWidth: listHidden ? Self.detailMinWidth : Self.detailMinWidth + max(listWidth, Self.listMinWidth))
        // Hiding the list shrinks the window instead of widening the tab pane.
        .background(SidebarWindowResizer(sidebarHidden: listHidden) { listWidth = $0 })
        .toolbar {
            if let progress = model.importProgress {
                ToolbarItem(placement: .status) {
                    ImportProgressView(progress: progress)
                }
            }
        }
        .dropDestination(for: URL.self) { urls, _ in
            let files = urls.filter(\.isFileURL)
            model.beginImport(files)
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
        .sheet(item: $model.importFlow) { request in
            ImportFlowSheet(core: core, request: request)
        }
        .sheet(isPresented: Binding(
            get: { !model.importFailures.isEmpty },
            set: { if !$0 { model.importFailures = [] } }
        )) {
            ImportFailuresSheet(failures: model.importFailures)
        }
        .alert(
            "Bulk Sync",
            isPresented: Binding(get: { model.bulkSyncSummary != nil }, set: { if !$0 { model.bulkSyncSummary = nil } }),
            presenting: model.bulkSyncSummary
        ) { _ in
            Button("OK") {}
        } message: { message in
            Text(message)
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
