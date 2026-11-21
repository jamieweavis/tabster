import AppKit
import TabsterKit
import SwiftUI

/// The library's tabs matching the search. Right-click a column header to show or hide columns.
struct TabTable: View {
    @Environment(AppModel.self) private var model
    /// Column sorting. Starts on Artist, so its header shows the arrow for the order the list
    /// opens in (the core's artist order, which `ArtistOrder` matches).
    @State private var sortOrder = [Self.byArtist]
    private static let byArtist = KeyPathComparator(\TabSummary.self, comparator: ArtistOrder())
    @State private var tagging: TabSummary?
    @State private var deleting: Set<Int64> = []
    /// Practice seconds per tab, for the Practice column.
    @State private var practice: [Int64: Double] = [:]
    /// Bumped when the viewer records practice, to refresh `practice`.
    @State private var practiceRevision = 0
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
    /// View ▸ Show Status Bar.
    @AppStorage(StatusBarSetting.key) private var showStatusBar = true
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
            TableColumn("Artist", sortUsing: Self.byArtist) { tab in
                Text(tab.artist)
            }
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
            TableColumn("Tracking", value: \.trackingOrder) { tab in
                if let tracking = tab.tracking {
                    Text("\(Int((tracking * 100).rounded()))%")
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                        .help("How well the tab tracks its video, from listening to it")
                }
            }
            .width(min: 46, ideal: 56, max: 80)
            .customizationID("tracking")
            TableColumn("Added", value: \.importedAt) { tab in
                Text(Self.formatAdded(tab.importedAt))
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
            .width(min: 60, ideal: 80, max: 120)
            .customizationID("added")
            TableColumn(Self.iconHeader("play.rectangle", "YouTube Video")) { (tab: TabSummary) in
                if tab.hasVideo {
                    Image(systemName: "play.rectangle")
                        .foregroundStyle(.secondary)
                        .help("Has a video")
                        .accessibilityLabel("Has a video")
                } else if tab.findingVideo {
                    ProgressView()
                        .controlSize(.mini)
                        .help("Searching YouTube…")
                        .accessibilityLabel("Searching YouTube")
                } else if tab.videoSuggested {
                    Image(systemName: "sparkle.magnifyingglass")
                        .foregroundStyle(.tint)
                        .help("A video was found: open the tab to check it")
                        .accessibilityLabel("Video found")
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
        .safeAreaInset(edge: .bottom, spacing: 0) {
            if showStatusBar { statusBar }
        }
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
        // Refreshed with the list, when a tab is closed (selection changes) and when practice is recorded.
        .task(id: [model.tabs.map(\.id), [model.selection ?? 0, Int64(practiceRevision)]]) {
            guard let core = model.core else { return }
            practice = await Task.detached { (try? core.practiceTotals()) ?? [:] }.value
        }
        .onReceive(NotificationCenter.default.publisher(for: .practiceRecorded)) { _ in
            practiceRevision += 1
        }
        .navigationTitle("All Tabs")
        .navigationSubtitle(model.tabs.isEmpty ? "" : "\(model.tabs.count) tab\(model.tabs.count == 1 ? "" : "s")")
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
        case "Added": texts = tabs.map { Self.formatAdded($0.importedAt) }
        case "Tracking": texts = tabs.compactMap { $0.tracking.map { "\(Int(($0 * 100).rounded()))%" } }
        default: return nil
        }
        if ["Tracks", "Length", "Practice", "Tracking", "Added"].contains(column) {
            font = .monospacedDigitSystemFont(ofSize: NSFont.systemFontSize, weight: .regular)
        }
        return texts.map { ($0 as NSString).size(withAttributes: [.font: font]).width }.max() ?? 0
    }

    /// Search, above the table (below the window's title bar).
    private var searchBar: some View {
        @Bindable var model = model
        return HStack(spacing: 6) {
            LibrarySearchField(text: $model.searchText, focusRequest: model.searchFocusRequest)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(.bar)
    }

    /// Below the table: the settings cog, then how many tabs are listed and how long they've been practised.
    private var statusBar: some View {
        let count = model.tabs.count
        let practised = model.tabs.reduce(0) { $0 + (practice[$1.id] ?? 0) }
        return HStack(spacing: 6) {
            SettingsLink {
                Image(systemName: "gearshape")
                    .font(.system(size: 15))
                    .frame(width: 24, height: 24)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.borderless)
            .help("Settings")
            .accessibilityLabel("Settings")
            Spacer(minLength: 0)
            Text(verbatim: "\(count) tab\(count == 1 ? "" : "s")"
                + (practised > 0 ? " · \(PracticeFormat.duration(practised)) practised" : ""))
            Spacer(minLength: 0)
            // Balances the cog so the text stays centred.
            Color.clear.frame(width: 24, height: 24)
        }
        .font(.subheadline)
        .foregroundStyle(.secondary)
        .monospacedDigit()
        .lineLimit(1)
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
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
        Button(selected.count == 1 ? "Find YouTube Video" : "Find YouTube Videos",
               systemImage: "magnifyingglass") {
            model.findVideos(ids)
        }
        .disabled(selected.allSatisfy(\.findingVideo))
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

    /// "21/11/2026": the day the tab was imported.
    static func formatAdded(_ unixSecs: Int64) -> String {
        addedFormatter.string(from: Date(timeIntervalSince1970: TimeInterval(unixSecs)))
    }

    private static let addedFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "dd/MM/yyyy"
        return formatter
    }()

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

/// The library's artist order, as the core sorts it: tabs without an artist last, then album and
/// title within an artist.
struct ArtistOrder: SortComparator {
    var order: Foundation.SortOrder = .forward

    func compare(_ lhs: TabSummary, _ rhs: TabSummary) -> ComparisonResult {
        let result = Self.ascending(lhs, rhs)
        guard order == .reverse else { return result }
        switch result {
        case .orderedAscending: return .orderedDescending
        case .orderedDescending: return .orderedAscending
        case .orderedSame: return .orderedSame
        }
    }

    private static func ascending(_ a: TabSummary, _ b: TabSummary) -> ComparisonResult {
        if a.artist.isEmpty != b.artist.isEmpty { return a.artist.isEmpty ? .orderedDescending : .orderedAscending }
        for (x, y) in [(a.artist, b.artist), (a.album, b.album), (a.title, b.title)] {
            let result = x.localizedCaseInsensitiveCompare(y)
            if result != .orderedSame { return result }
        }
        return a.id == b.id ? .orderedSame : (a.id < b.id ? .orderedAscending : .orderedDescending)
    }
}
