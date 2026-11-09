import AppKit
import TabsterKit
import SwiftUI

/// The library's tabs matching the search. Right-click a column header to show or hide columns.
struct TabTable: View {
    @Environment(AppModel.self) private var model
    /// Column sorting, applied on top of the core's order. Empty means the core's order.
    @State private var sortOrder: [KeyPathComparator<TabSummary>] = []
    @State private var tagging: TabSummary?
    @State private var deleting: Set<Int64> = []
    /// Practice seconds per tab, for the Practice column.
    @State private var practice: [Int64: Double] = [:]
    /// Which columns are shown, their order and widths, as last seen at a usable width;
    /// persisted across launches.
    @AppStorage("libraryColumns") private var savedColumns = TableColumnCustomization<TabSummary>()
    /// The live column layout. Hiding the list squeezes it to nothing, which crushes every column
    /// to its minimum; those changes aren't saved, and the saved layout comes back on reopening.
    @State private var columns: TableColumnCustomization<TabSummary>?
    @State private var tableWidth: CGFloat = 0
    /// False until `ColumnAutosizer` has stopped the columns autoresizing: the table's first layout
    /// stretches them to fill it, and that mustn't overwrite the saved widths.
    @State private var layoutSettled = false
    /// Below this the table is collapsing (the list's minimum width is 240).
    private static let usableWidth: CGFloat = 200

    private var rows: [TabSummary] {
        sortOrder.isEmpty ? model.tabs : model.tabs.sorted(using: sortOrder)
    }

    var body: some View {
        @Bindable var model = model
        Table(rows, selection: $model.selection, sortOrder: $sortOrder, columnCustomization: Binding(
            get: { columns ?? savedColumns },
            set: { columns = $0 }
        )) {
            TableColumn("Title", value: \.title) { tab in
                Text(tab.title.isEmpty ? "Untitled" : tab.title)
                    .foregroundStyle(tab.title.isEmpty ? .secondary : .primary)
            }
            .width(min: 100, ideal: 170)
            .customizationID("title")
            .disabledCustomizationBehavior(.visibility)
            TableColumn("Artist", value: \.artist)
                .width(min: 70, ideal: 110)
                .customizationID("artist")
            TableColumn("Tuning", value: \.tuning) { tab in
                Text(tab.tuning).foregroundStyle(.secondary)
            }
            .width(min: 50, ideal: 80)
            .customizationID("tuning")
            TableColumn("Tracks", value: \.trackCount) { tab in
                Text("\(tab.trackCount)").monospacedDigit()
            }
            .width(min: 40, ideal: 44, max: 64)
            .customizationID("tracks")
            TableColumn("Length", value: \.durationSecs) { tab in
                Text(Self.formatLength(tab.durationSecs)).monospacedDigit()
            }
            .width(min: 46, ideal: 50, max: 72)
            .customizationID("length")
            TableColumn("Practice") { (tab: TabSummary) in
                if let secs = practice[tab.id], secs >= 60 {
                    Text(PracticeFormat.duration(secs)).monospacedDigit().foregroundStyle(.secondary)
                }
            }
            .width(min: 46, ideal: 56, max: 80)
            .customizationID("practice")
            TableColumn(Self.iconHeader("play.rectangle", "YouTube Video")) { (tab: TabSummary) in
                if tab.hasVideo {
                    Image(systemName: "play.rectangle")
                        .foregroundStyle(.secondary)
                        .help("Has a video")
                        .accessibilityLabel("Has a video")
                }
            }
            .width(22)
            .customizationID("video")
            TableColumn(Self.iconHeader("star", "Favourite")) { (tab: TabSummary) in
                FavouriteButton(tab: tab)
            }
            .width(22)
            .customizationID("favourite")
        }
        .background(ColumnAutosizer(contentWidth: contentWidth) {
            // Undo the first layout's stretching, then start saving.
            columns = savedColumns
            layoutSettled = true
        })
        .safeAreaInset(edge: .top, spacing: 0) { searchBar }
        .onGeometryChange(for: CGFloat.self, of: \.size.width) { width in
            let wasUsable = tableWidth >= Self.usableWidth
            tableWidth = width
            // Reopened: put back the layout from before it was squeezed.
            if !wasUsable, width >= Self.usableWidth { columns = savedColumns }
        }
        .onChange(of: columns) { _, new in
            if let new, layoutSettled, tableWidth >= Self.usableWidth { savedColumns = new }
        }
        .contextMenu(forSelectionType: Int64.self) { ids in
            if !ids.isEmpty { menu(for: ids) }
        } primaryAction: { ids in
            if let id = ids.first { model.selection = id }
        }
        .onDeleteCommand {
            if let id = model.selection { deleting = [id] }
        }
        .overlay {
            if model.tabs.isEmpty { emptyState }
        }
        // Refreshed with the list and when a tab is closed (selection changes).
        .task(id: [model.tabs.map(\.id), [model.selection ?? 0]]) {
            guard let core = model.core else { return }
            practice = await Task.detached { (try? core.practiceTotals()) ?? [:] }.value
        }
        .navigationTitle("All Tabs")
        .navigationSubtitle(model.tabs.isEmpty ? "" : "\(model.tabs.count) tab\(model.tabs.count == 1 ? "" : "s")")
        .toolbar {
            ToolbarItem {
                Text("Tabster")
                    .font(.title3.weight(.semibold))
                    .padding(.horizontal, 6)
            }
            .sharedBackgroundVisibility(.hidden)
            ToolbarSpacer(.flexible)
        }
        .sheet(item: $tagging) { tab in
            TagsSheet(tab: tab)
        }
        .confirmationDialog(
            deleting.count == 1 ? "Delete this tab?" : "Delete \(deleting.count) tabs?",
            isPresented: Binding(get: { !deleting.isEmpty }, set: { if !$0 { deleting = [] } })
        ) {
            Button("Delete", role: .destructive) { [deleting] in model.delete(deleting) }
        } message: {
            Text("The tab file, its videos and sync points are removed from the library.")
        }
    }

    /// The widest text in the column titled `column`, for sizing it to fit; nil for the icon
    /// columns, which have a fixed width.
    private func contentWidth(_ column: String) -> CGFloat? {
        let tabs = model.tabs
        let texts: [String]
        var font = NSFont.systemFont(ofSize: NSFont.systemFontSize)
        switch column {
        case "Title": texts = tabs.map { $0.title.isEmpty ? "Untitled" : $0.title }
        case "Artist": texts = tabs.map(\.artist)
        case "Tuning": texts = tabs.map(\.tuning)
        case "Tracks": texts = tabs.map { "\($0.trackCount)" }
        case "Length": texts = tabs.map { Self.formatLength($0.durationSecs) }
        case "Practice": texts = practice.values.filter { $0 >= 60 }.map(PracticeFormat.duration)
        default: return nil
        }
        if ["Tracks", "Length", "Practice"].contains(column) {
            font = .monospacedDigitSystemFont(ofSize: NSFont.systemFontSize, weight: .regular)
        }
        return texts.map { ($0 as NSString).size(withAttributes: [.font: font]).width }.max() ?? 0
    }

    /// Search and sort, above the table (below the window's title bar).
    private var searchBar: some View {
        @Bindable var model = model
        return HStack(spacing: 6) {
            LibrarySearchField(text: $model.searchText, focusRequest: model.searchFocusRequest)
            Menu("Sort", systemImage: "arrow.up.arrow.down") {
                Picker("Sort By", selection: Binding(get: { model.sort }, set: {
                    model.sort = $0
                    sortOrder = []
                })) {
                    Text("Title").tag(SortOrder.title)
                    Text("Artist").tag(SortOrder.artist)
                    Text("Recently Added").tag(SortOrder.recentlyAdded)
                    Text("Recently Opened").tag(SortOrder.recentlyOpened)
                }
                .pickerStyle(.inline)
            }
            .labelStyle(.iconOnly)
            .menuIndicator(.hidden)
            .buttonStyle(.borderless)
            .fixedSize()
            .help("Sort the library")
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(.bar)
    }

    @ViewBuilder
    private var emptyState: some View {
        if !model.searchText.isEmpty {
            ContentUnavailableView.search(text: model.searchText)
        } else {
            ContentUnavailableView(
                "No Tabs",
                systemImage: "square.and.arrow.down",
                description: Text("Import tabs (⌘O) or drop Guitar Pro files here")
            )
        }
    }

    @ViewBuilder
    private func menu(for ids: Set<Int64>) -> some View {
        let selected = model.tabs.filter { ids.contains($0.id) }
        let allFavourite = selected.allSatisfy(\.favourite)
        Button(allFavourite ? "Remove from Favourites" : "Add to Favourites",
               systemImage: allFavourite ? "star.slash" : "star") {
            model.setFavourite(ids, !allFavourite)
        }
        if selected.count == 1, let tab = selected.first {
            Button("Tags…", systemImage: "tag") { tagging = tab }
        }
        Divider()
        Button("Reveal in Finder", systemImage: "folder") {
            let urls = ids.compactMap { model.filePath($0) }
            NSWorkspace.shared.activateFileViewerSelecting(urls)
        }
        Divider()
        Button("Delete…", systemImage: "trash", role: .destructive) { deleting = ids }
    }

    /// A header that shows only `symbol`. `name` rides along invisibly so the header's
    /// right-click column menu (which lists the headers' plain text) has a label for it.
    private static func iconHeader(_ symbol: String, _ name: String) -> Text {
        Text("\(Image(systemName: symbol))\(Text(name).font(.system(size: 1)).foregroundStyle(.clear))")
            .accessibilityLabel(name)
    }

    static func formatLength(_ secs: Double) -> String {
        let total = Int(secs.rounded())
        return String(format: "%d:%02d", total / 60, total % 60)
    }
}

/// The star in a row; toggles the tab's favourite.
private struct FavouriteButton: View {
    let tab: TabSummary
    @Environment(AppModel.self) private var model

    var body: some View {
        Button {
            model.setFavourite([tab.id], !tab.favourite)
        } label: {
            Image(systemName: tab.favourite ? "star.fill" : "star")
                .foregroundStyle(tab.favourite ? AnyShapeStyle(.yellow) : AnyShapeStyle(.tertiary))
        }
        .buttonStyle(.borderless)
        .help(tab.favourite ? "Remove from Favourites" : "Add to Favourites")
        .accessibilityLabel(tab.favourite ? "Remove from Favourites" : "Add to Favourites")
    }
}
