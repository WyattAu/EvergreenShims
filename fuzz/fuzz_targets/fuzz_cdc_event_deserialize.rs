#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &str| {
    let _ = serde_json::from_str::<cdc_shim::CdcEvent>(data);
});
