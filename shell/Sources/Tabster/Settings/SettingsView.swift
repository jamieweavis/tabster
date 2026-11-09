import AppKit
import SwiftUI

struct SettingsView: View {
    var body: some View {
        TabView {
            Tab("General", systemImage: "gearshape") {
                GeneralSettings()
            }
        }
        .frame(width: 520)
    }
}

private struct GeneralSettings: View {
    @Environment(AppModel.self) private var model
    @State private var selectedFolder: String?

    var body: some View {
        Form {
            Section {
                List(model.config.watchedFolders, id: \.self, selection: $selectedFolder) { folder in
                    Label {
                        Text(URL(filePath: folder).lastPathComponent)
                        Text(folder).font(.caption).foregroundStyle(.secondary)
                    } icon: {
                        Image(systemName: "folder")
                    }
                    .help(folder)
                    .contextMenu {
                        Button("Show in Finder") {
                            NSWorkspace.shared.activateFileViewerSelecting([URL(filePath: folder)])
                        }
                        Button("Remove", role: .destructive) { remove(folder) }
                    }
                }
                .frame(minHeight: 140)
                .overlay {
                    if model.config.watchedFolders.isEmpty {
                        Text("No watched folders")
                            .foregroundStyle(.secondary)
                    }
                }
                HStack(spacing: 4) {
                    Button("Add Folder…", systemImage: "plus") { add() }
                    Button("Remove", systemImage: "minus") {
                        if let selectedFolder { remove(selectedFolder) }
                    }
                    .disabled(selectedFolder == nil)
                }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
            } header: {
                Text("Watched Folders")
            } footer: {
                Text("New Guitar Pro files saved in these folders are imported automatically.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Section("Viewer") {
                Toggle("Follow the cursor while playing", isOn: configBinding(\.followCursor))
                Toggle("Show rhythm notation", isOn: configBinding(\.showRhythm))
                Picker("Open new tabs on", selection: configBinding(\.preferredInstrument)) {
                    Text("First track").tag(Instrument?.none)
                    ForEach(Self.instruments, id: \.0) { instrument, name in
                        Text(name).tag(Instrument?.some(instrument))
                    }
                }
                .help("The track a tab starts on the first time it's opened, when it has one for this instrument.")
            }
        }
        .formStyle(.grouped)
        .scrollDisabled(true)
        .fixedSize(horizontal: false, vertical: true)
    }

    private static let instruments: [(Instrument, String)] = [
        (.guitar, "Guitar"), (.bass, "Bass"), (.drums, "Drums"), (.keys, "Keys"),
    ]

    private func configBinding<T>(_ keyPath: WritableKeyPath<Config, T>) -> Binding<T> {
        Binding(
            get: { model.config[keyPath: keyPath] },
            set: { value in
                var config = model.config
                config[keyPath: keyPath] = value
                model.updateConfig(config)
            }
        )
    }

    private func add() {
        let panel = NSOpenPanel()
        panel.title = "Watch Folder"
        panel.prompt = "Watch"
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = true
        guard panel.runModal() == .OK else { return }
        var config = model.config
        for url in panel.urls {
            let path = url.path(percentEncoded: false)
            if !config.watchedFolders.contains(path) { config.watchedFolders.append(path) }
        }
        model.updateConfig(config)
    }

    private func remove(_ folder: String) {
        var config = model.config
        config.watchedFolders.removeAll { $0 == folder }
        model.updateConfig(config)
        if selectedFolder == folder { selectedFolder = nil }
    }
}
