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
            }
        }
        .frame(minWidth: 1000, minHeight: 650)
    }
}

struct LibraryView: View {
    let core: Core
    @Environment(AppModel.self) private var model
    @State private var isDropTargeted = false
    /// Whether the tab list is shown beside the viewer (the toolbar's sidebar button).
    @State private var columnVisibility = NavigationSplitViewVisibility.all

    var body: some View {
        @Bindable var model = model
        NavigationSplitView(columnVisibility: $columnVisibility) {
            TabTable()
                .navigationSplitViewColumnWidth(min: 240, ideal: 600)
        } detail: {
            if let id = model.selection {
                TabViewer(core: core, tabID: id)
                    .id(id)
            } else {
                ContentUnavailableView(
                    "No Tab Selected",
                    systemImage: "music.note.list",
                    description: Text("Choose a tab from the library to open it.")
                )
                // "Tabster" already heads the table column.
                .navigationTitle("")
            }
        }
        .toolbar {
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
