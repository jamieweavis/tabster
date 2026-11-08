//! `formats::parse` with every extension hint (the hint only matters when no magic matches).
#![no_main]

use libfuzzer_sys::fuzz_target;
use tabster_core::formats;

fuzz_target!(|data: &[u8]| {
    for ext in [None, Some("gp"), Some("gpx"), Some("gp5"), Some("gp4"), Some("txt")] {
        let _ = formats::parse(data, ext);
    }
});
