//! Logging to the unified log (subsystem `com.jamieweavis.tabster`); `just logs` streams it.

use std::sync::Once;

pub const SUBSYSTEM: &str = "com.jamieweavis.tabster";

pub fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = oslog::OsLogger::new(SUBSYSTEM)
            .level_filter(log::LevelFilter::Debug)
            .init();
    });
}
