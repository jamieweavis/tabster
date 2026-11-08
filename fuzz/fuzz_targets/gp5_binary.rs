//! The GP5 binary importer.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = tabster_core::formats::gp5::parse(data);
});
