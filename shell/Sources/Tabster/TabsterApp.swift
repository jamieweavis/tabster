import AppKit
import SwiftUI
import os

@main
struct TabsterApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    var body: some Scene {
        WindowGroup("Tabster") {
            ViewerHarness.debugHarnessIfRequested { ContentView() }
                .environment(appDelegate.model)
        }
        .defaultSize(width: 1280, height: 800)
        .windowResizability(.contentMinSize)
        .commands {
            LibraryCommands(model: appDelegate.model)
        }

        Settings {
            SettingsView()
                .environment(appDelegate.model)
        }
    }
}

struct LibraryCommands: Commands {
    let model: AppModel

    var body: some Commands {
        // The library is one window over shared state, so File ▸ New Window would only confuse.
        CommandGroup(replacing: .newItem) {
            Button("Import…") { model.importURLs(ImportPanel.run()) }
                .keyboardShortcut("o")
                .disabled(model.core == nil)
        }
        CommandGroup(after: .textEditing) {
            Button("Find") { model.requestSearchFocus() }
                .keyboardShortcut("f")
                .disabled(model.core == nil)
        }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let model = AppModel()

    func applicationDidFinishLaunching(_ notification: Notification) {
        Logger.app.info("launched; data dir \(AppModel.dataDirectory().path(percentEncoded: false), privacy: .public)")
        #if DEBUG
        DebugHooks.run(model)
        #endif
    }

    /// Tab files opened from Finder (the bundle declares the Guitar Pro document types).
    func application(_ application: NSApplication, open urls: [URL]) {
        let files = urls.filter(\.isFileURL)
        Logger.app.info("asked to open \(files.count) file(s)")
        model.importURLs(files, select: true)
    }
}
