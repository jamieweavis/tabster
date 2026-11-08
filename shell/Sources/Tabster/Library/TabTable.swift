import AppKit
import SwiftUI

/// The tabs matching the sidebar item and search.
struct TabTable: View {
    @Environment(AppModel.self) private var model
    /// Column sorting, applied on top of the core's order. Empty means the core's order.
    @State private var sortOrder: [KeyPathComparator<TabSummary>] = []
    @State private var tagging: TabSummary?
    @State private var deleting: Set<Int64> = []

    private var rows: [TabSummary] {
        sortOrder.isEmpty ? model.tabs : model.tabs.sorted(using: sortOrder)
    }

    var body: some View {
        @Bindable var model = model
        Table(rows, selection: $model.selection, sortOrder: $sortOrder) {
            TableColumn("Title", value: \.title) { tab in
                Text(tab.title.isEmpty ? "Untitled" : tab.title)
                    .foregroundStyle(tab.title.isEmpty ? .secondary : .primary)
            }
            .width(min: 100, ideal: 170)
            TableColumn("Artist", value: \.artist)
                .width(min: 70, ideal: 110)
            TableColumn("Tuning", value: \.tuning) { tab in
                Text(tab.tuning).foregroundStyle(.secondary)
            }
            .width(min: 50, ideal: 80)
            TableColumn("Tracks", value: \.trackCount) { tab in
                Text("\(tab.trackCount)").monospacedDigit()
            }
            .width(min: 40, ideal: 44, max: 64)
            TableColumn("Length", value: \.durationSecs) { tab in
                Text(Self.formatLength(tab.durationSecs)).monospacedDigit()
            }
            .width(min: 46, ideal: 50, max: 72)
            TableColumn(Text(Image(systemName: "play.rectangle")).accessibilityLabel("Video")) { (tab: TabSummary) in
                if tab.hasVideo {
                    Image(systemName: "play.rectangle")
                        .foregroundStyle(.secondary)
                        .help("Has a video")
                        .accessibilityLabel("Has a video")
                }
            }
            .width(22)
            TableColumn(Text(Image(systemName: "star")).accessibilityLabel("Favourite")) { (tab: TabSummary) in
                FavouriteButton(tab: tab)
            }
            .width(22)
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
        .navigationTitle(title)
        .navigationSubtitle(model.tabs.isEmpty ? "" : "\(model.tabs.count) tab\(model.tabs.count == 1 ? "" : "s")")
        .toolbar {
            ToolbarItem {
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
                .help("Sort the library")
            }
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

    private var title: String {
        switch model.sidebar {
        case .all: "All Tabs"
        case .recent: "Recently Opened"
        case .favourites: "Favourites"
        case .tag(let tag): tag
        case .setlist(let id): model.setlists.first { $0.id == id }?.name ?? "Setlist"
        }
    }

    @ViewBuilder
    private var emptyState: some View {
        if !model.searchText.isEmpty {
            ContentUnavailableView.search(text: model.searchText)
        } else {
            switch model.sidebar {
            case .all:
                ContentUnavailableView(
                    "No Tabs",
                    systemImage: "square.and.arrow.down",
                    description: Text("Import tabs (⌘O) or drop Guitar Pro files here")
                )
            case .recent:
                ContentUnavailableView("Nothing Opened Yet", systemImage: "clock",
                                       description: Text("Tabs you open appear here."))
            case .favourites:
                ContentUnavailableView("No Favourites", systemImage: "star",
                                       description: Text("Mark tabs with the star to keep them here."))
            case .tag:
                ContentUnavailableView("No Tabs", systemImage: "tag")
            case .setlist:
                ContentUnavailableView("Empty Setlist", systemImage: "list.number",
                                       description: Text("Add tabs with Add to Setlist in their context menu."))
            }
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
        Menu("Add to Setlist", systemImage: "text.badge.plus") {
            ForEach(model.setlists, id: \.id) { setlist in
                Button(setlist.name) { model.add(ids, toSetlist: setlist.id) }
            }
            if !model.setlists.isEmpty { Divider() }
            Button("New Setlist from Selection") {
                let name = "Setlist \(model.setlists.count + 1)"
                if let id = model.createSetlist(named: name) { model.add(ids, toSetlist: id) }
            }
        }
        if case .setlist(let setlist) = model.sidebar {
            Button("Remove from Setlist", systemImage: "minus.circle") {
                model.remove(ids, fromSetlist: setlist)
            }
        }
        Divider()
        Button("Reveal in Finder", systemImage: "folder") {
            let urls = ids.compactMap { model.filePath($0) }
            NSWorkspace.shared.activateFileViewerSelecting(urls)
        }
        Divider()
        Button("Delete…", systemImage: "trash", role: .destructive) { deleting = ids }
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
