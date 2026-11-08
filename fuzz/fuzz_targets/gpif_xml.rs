//! The GPIF XML importer on arbitrary bytes.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = tabster_core::formats::gpif::parse_gpif(data);
});
