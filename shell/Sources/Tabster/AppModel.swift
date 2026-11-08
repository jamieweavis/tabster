import Foundation
import Observation

/// Owns the Rust `Core`. All behaviour lives in Rust; this forwards intent and holds UI state.
@MainActor
@Observable
final class AppModel {
    let core: Core?
    var startupError: String?

    init() {
        let dataDir = AppModel.dataDirectory()
        do {
            core = try Core(dataDir: dataDir.path)
        } catch {
            core = nil
            startupError = "\(error)"
        }
    }

    /// `~/Library/Application Support/Tabster`, or `TABSTER_DATA_DIR` when set.
    static func dataDirectory() -> URL {
        if let override = ProcessInfo.processInfo.environment["TABSTER_DATA_DIR"] {
            return URL(fileURLWithPath: override)
        }
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return base.appendingPathComponent("Tabster", isDirectory: true)
    }
}
