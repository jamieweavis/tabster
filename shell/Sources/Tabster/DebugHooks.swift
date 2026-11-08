#if DEBUG
import AppKit
import os

/// Debug-build launch hooks for scripted checks:
/// - `TABSTER_IMPORT=<path>:<path>…` imports those files/folders at startup.
/// - `TABSTER_SNAPSHOT=<dir>` waits for the import to finish, writes `main.png` (the main
///   window) and `settings.png` (the Settings window) to `<dir>`, then quits.
/// - `TABSTER_SELECT=<n>` selects the n-th (0-based) row before the snapshot.
@MainActor
enum DebugHooks {
    static func run(_ model: AppModel) {
        let env = ProcessInfo.processInfo.environment
        if let paths = env["TABSTER_IMPORT"], !paths.isEmpty {
            let urls = paths.split(separator: ":").map { URL(filePath: String($0)) }
            model.importURLs(urls)
        }
        if let dir = env["TABSTER_SNAPSHOT"], !dir.isEmpty {
            let select = env["TABSTER_SELECT"].flatMap { Int($0) }
            Task { await snapshot(model, to: URL(filePath: dir, directoryHint: .isDirectory), select: select) }
        }
    }

    private static func snapshot(_ model: AppModel, to dir: URL, select: Int?) async {
        NSApp.activate()
        try? await Task.sleep(for: .seconds(1))
        while model.importProgress != nil {
            try? await Task.sleep(for: .milliseconds(100))
        }
        try? await Task.sleep(for: .seconds(1))
        if let select, model.tabs.indices.contains(select) {
            model.selection = model.tabs[select].id
            try? await Task.sleep(for: .seconds(1.5))
        }
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let main = NSApp.windows.first { $0.isVisible && $0.canBecomeMain && $0.sheetParent == nil }
        write(main, to: dir.appending(path: "main.png"))

        // Open Settings the way the user does: App menu ▸ Settings… (⌘,).
        if let appMenu = NSApp.mainMenu?.items.first?.submenu,
           let index = appMenu.items.firstIndex(where: { $0.keyEquivalent == "," }) {
            appMenu.performActionForItem(at: index)
        }
        try? await Task.sleep(for: .seconds(1))
        let settings = NSApp.windows.first { $0.isVisible && $0 !== main && $0.sheetParent == nil }
        write(settings, to: dir.appending(path: "settings.png"))
        // `terminate` can stall with the Settings window up; this is a debug hook, so just exit.
        exit(0)
    }

    /// Writes a PNG of the whole window. Prefers a real screen grab (glass and vibrancy included,
    /// needs Screen Recording permission); falls back to drawing the view hierarchy.
    private static func write(_ window: NSWindow?, to url: URL) {
        guard let window else {
            Logger.app.error("snapshot: no window for \(url.lastPathComponent, privacy: .public)")
            return
        }
        try? FileManager.default.removeItem(at: url)
        let process = Process()
        process.executableURL = URL(filePath: "/usr/sbin/screencapture")
        process.arguments = ["-x", "-o", "-l\(window.windowNumber)", url.path(percentEncoded: false)]
        try? process.run()
        process.waitUntilExit()
        if !FileManager.default.fileExists(atPath: url.path(percentEncoded: false)),
           let view = window.contentView?.superview ?? window.contentView,
           let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) {
            view.cacheDisplay(in: view.bounds, to: rep)
            try? rep.representation(using: .png, properties: [:])?.write(to: url)
        }
        Logger.app.info("snapshot written to \(url.path(percentEncoded: false), privacy: .public)")
    }
}
#endif
