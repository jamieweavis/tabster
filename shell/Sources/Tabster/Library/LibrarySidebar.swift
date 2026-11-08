import SwiftUI

/// Library smart lists, tags and setlists.
struct LibrarySidebar: View {
    @Environment(AppModel.self) private var model
    @State private var editor: SetlistEditor?
    @State private var setlistName = ""
    @State private var deleting: Setlist?

    var body: some View {
        @Bindable var model = model
        List(selection: Binding<SidebarItem?>(get: { model.sidebar }, set: { model.sidebar = $0 ?? .all })) {
            Section("Library") {
                Label("All Tabs", systemImage: "music.note.list").tag(SidebarItem.all)
                Label("Recently Opened", systemImage: "clock").tag(SidebarItem.recent)
                Label("Favourites", systemImage: "star").tag(SidebarItem.favourites)
            }
            if !model.tags.isEmpty {
                Section("Tags") {
                    ForEach(model.tags, id: \.self) { tag in
                        Label(tag, systemImage: "tag").tag(SidebarItem.tag(tag))
                    }
                }
            }
            Section {
                ForEach(model.setlists, id: \.id) { setlist in
                    Label(setlist.name, systemImage: "list.number")
                        .badge(Int(setlist.tabCount))
                        .tag(SidebarItem.setlist(setlist.id))
                        .contextMenu {
                            Button("Rename…") { edit(.rename(setlist), name: setlist.name) }
                            Divider()
                            Button("Delete Setlist…", role: .destructive) { deleting = setlist }
                        }
                }
            } header: {
                HStack {
                    Text("Setlists")
                    Spacer()
                    Button("New Setlist", systemImage: "plus") { edit(.create, name: "") }
                        .labelStyle(.iconOnly)
                        .buttonStyle(.borderless)
                        .help("New Setlist")
                }
            }
        }
        .listStyle(.sidebar)
        .alert(editor?.title ?? "", isPresented: Binding(get: { editor != nil }, set: { if !$0 { editor = nil } })) {
            TextField("Name", text: $setlistName)
            Button(editor?.confirm ?? "OK") { commit() }
                .keyboardShortcut(.defaultAction)
            Button("Cancel", role: .cancel) {}
        }
        .confirmationDialog(
            "Delete “\(deleting?.name ?? "")”?",
            isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
            presenting: deleting
        ) { setlist in
            Button("Delete Setlist", role: .destructive) { model.deleteSetlist(setlist.id) }
        } message: { _ in
            Text("The tabs stay in your library.")
        }
    }

    private func edit(_ editor: SetlistEditor, name: String) {
        setlistName = name
        self.editor = editor
    }

    private func commit() {
        switch editor {
        case .create:
            if let id = model.createSetlist(named: setlistName) { model.sidebar = .setlist(id) }
        case .rename(let setlist):
            model.renameSetlist(setlist, to: setlistName)
        case nil:
            break
        }
        editor = nil
    }
}

private enum SetlistEditor {
    case create
    case rename(Setlist)

    var title: String {
        switch self {
        case .create: "New Setlist"
        case .rename: "Rename Setlist"
        }
    }

    var confirm: String {
        switch self {
        case .create: "Create"
        case .rename: "Rename"
        }
    }
}
