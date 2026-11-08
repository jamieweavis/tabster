//! The GPX (BCFZ/BCFS) container reader.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = tabster_core::formats::gpx::extract_gpif(data);
});
