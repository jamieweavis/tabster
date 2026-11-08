import Foundation
import Observation
import os

/// Owns the Rust `Core` and the library state the UI observes. All behaviour lives in Rust; this
/// forwards intent and holds UI state.
@MainActor
@Observable
final class AppModel {
    /// Nil only if the core failed to start; `startupError` then says why.
    let core: Core?
    private(set) var startupError: String?

    // MARK: Library state

    /// The search field's text. The table follows it after a short pause.
    var searchText = "" {
        didSet {
            if searchText != oldValue { scheduleSearch() }
        }
    }
    /// The core's ordering, used when no table column sort is active.
    var sort: SortOrder = .title {
        didSet {
            if sort != oldValue { reloadTabs() }
        }
    }
    private(set) var tabs: [TabSummary] = []
    private(set) var tags: [String] = []
    var selection: Int64?

    // MARK: Import state

    private(set) var importProgress: ImportProgress?
    /// The failures of the last import, shown in a sheet until dismissed.
    var importFailures: [ImportOutcome] = []
    /// A failed action's message, shown in an alert until dismissed.
    var errorMessage: String?
    /// Bumped by Edit ▸ Find; the library view focuses its search field.
    private(set) var searchFocusRequest = 0

    // MARK: Settings

    /// Mirrors the core's config (`~/.config/tabster.toml`); change it through `updateConfig`.
    private(set) var config = Config(scale: 1.0, showRhythm: true, watchedFolders: [], followCursor: true)

    @ObservationIgnored private var query = ""
    @ObservationIgnored private var searchTask: Task<Void, Never>?
    @ObservationIgnored private var reloadGeneration = 0

    init() {
        let dataDir = AppModel.dataDirectory()
        do {
            try FileManager.default.createDirectory(at: dataDir, withIntermediateDirectories: true)
            core = try Core(dataDir: dataDir.path(percentEncoded: false))
        } catch {
            Logger.app.fault("couldn't start the core: \(error, privacy: .public)")
            core = nil
            startupError = error.userMessage
            return
        }
        if let core { config = core.config() }
        core?.setLibraryObserver(observer: LibraryObserverBridge(model: self))
        reloadAll()
    }

    /// `~/Library/Application Support/Tabster`, or `TABSTER_DATA_DIR` when set.
    static func dataDirectory() -> URL {
        if let override = ProcessInfo.processInfo.environment["TABSTER_DATA_DIR"], !override.isEmpty {
            return URL(filePath: override, directoryHint: .isDirectory)
        }
        return URL.applicationSupportDirectory.appending(path: "Tabster", directoryHint: .isDirectory)
    }

    var filter: TabFilter {
        TabFilter(query: query, favouritesOnly: false, tag: nil, setlist: nil, sort: sort)
    }

    var selectedTab: TabSummary? {
        tabs.first { $0.id == selection }
    }

    func requestSearchFocus() {
        searchFocusRequest += 1
    }

    // MARK: Loading

    func reloadAll() {
        guard let core else { return }
        do {
            tags = try core.tags()
        } catch {
            Logger.library.error("couldn't load tags: \(error, privacy: .public)")
        }
        reloadTabs()
    }

    /// Lists the tabs off the main actor; results of a superseded reload are dropped.
    func reloadTabs() {
        guard let core else { return }
        let filter = filter
        reloadGeneration += 1
        let generation = reloadGeneration
        Task {
            let result = await Task.detached { Result { try core.listTabs(filter: filter) } }.value
            guard generation == reloadGeneration else { return }
            switch result {
            case .success(let list):
                tabs = list
            case .failure(let error):
                Logger.library.error("couldn't list tabs: \(error, privacy: .public)")
                errorMessage = error.userMessage
            }
        }
    }

    private func scheduleSearch() {
        searchTask?.cancel()
        searchTask = Task {
            try? await Task.sleep(for: .milliseconds(150))
            guard !Task.isCancelled else { return }
            query = searchText
            reloadTabs()
        }
    }

    // MARK: Import

    /// Imports files and folders in the background. `select` selects the first imported tab.
    func importURLs(_ urls: [URL], select: Bool = false) {
        guard let core, !urls.isEmpty else { return }
        let paths = urls.map { $0.path(percentEncoded: false) }
        Logger.library.info("importing \(paths.count) path(s)")
        importProgress = ImportProgress(done: 0, total: UInt32(paths.count))
        Task {
            let outcomes = await Task.detached { core.importPaths(paths: paths) }.value
            importProgress = nil
            let failures = outcomes.filter { $0.error != nil }
            let imported = outcomes.count - failures.count
            Logger.library.info("imported \(imported) of \(outcomes.count) file(s)")
            for failure in failures {
                Logger.library.error(
                    "couldn't import \(failure.path, privacy: .public): \(failure.error ?? "", privacy: .public)")
            }
            importFailures = failures
            if select, let id = outcomes.lazy.compactMap(\.id).first {
                // The tab must be in the table to be selected.
                if !tabs.contains(where: { $0.id == id }) { searchText = "" }
                selection = id
            }
        }
    }

    fileprivate func importProgressed(done: UInt32, total: UInt32) {
        // Ignore a straggler arriving after the import finished.
        guard importProgress != nil else { return }
        importProgress = ImportProgress(done: done, total: total)
    }

    // MARK: Tab actions

    func setFavourite(_ ids: Set<Int64>, _ favourite: Bool) {
        perform { core in
            for id in ids { try core.setFavourite(id: id, favourite: favourite) }
        }
    }

    func setTags(_ id: Int64, _ tags: [String]) {
        perform { core in try core.setTags(id: id, tags: tags) }
    }

    func delete(_ ids: Set<Int64>) {
        perform { core in
            for id in ids { try core.deleteTab(id: id) }
        }
        if let selection, ids.contains(selection) { self.selection = nil }
    }

    func filePath(_ id: Int64) -> URL? {
        guard let core else { return nil }
        do {
            return URL(filePath: try core.tabFilePath(id: id))
        } catch {
            errorMessage = error.userMessage
            return nil
        }
    }

    // MARK: Settings

    func updateConfig(_ config: Config) {
        core?.setConfig(config: config)
        self.config = config
        Logger.settings.info("settings changed")
    }

    /// Runs quick core calls (single-row writes) on the main actor and reports failure.
    private func perform(_ body: (Core) throws -> Void) {
        guard let core else { return }
        do {
            try body(core)
        } catch {
            Logger.library.error("library action failed: \(error, privacy: .public)")
            errorMessage = error.userMessage
        }
    }
}

struct ImportProgress: Equatable {
    var done: UInt32
    var total: UInt32
}

extension TabSummary: Identifiable {}

/// Receives the core's library callbacks on background threads and forwards them, in order, to
/// the main actor.
final class LibraryObserverBridge: LibraryObserver {
    private weak let model: AppModel?

    init(model: AppModel) {
        self.model = model
    }

    func importProgress(done: UInt32, total: UInt32) {
        DispatchQueue.main.async { [weak model] in
            MainActor.assumeIsolated { model?.importProgressed(done: done, total: total) }
        }
    }

    func libraryChanged() {
        DispatchQueue.main.async { [weak model] in
            MainActor.assumeIsolated { model?.reloadAll() }
        }
    }
}
