import os

/// Unified logging, under the same subsystem as the Rust core; `just logs` streams both.
/// Core error messages and file paths are safe to make public.
extension Logger {
    private static let subsystem = "com.jamieweavis.tabster"

    static let app = Logger(subsystem: subsystem, category: "app")
    static let library = Logger(subsystem: subsystem, category: "library")
    static let settings = Logger(subsystem: subsystem, category: "settings")
}

extension Error {
    /// A message fit for the UI; the generated `CoreError.errorDescription` is debug output.
    var userMessage: String {
        guard let error = self as? CoreError else { return localizedDescription }
        switch error {
        case .Library(let message): return message
        case .InvalidVideo(let message): return message
        case .NotFound: return "The tab is no longer in the library."
        }
    }
}
