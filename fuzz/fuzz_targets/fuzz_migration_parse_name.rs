#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &str| {
    let _ = migration_shim::MigrationShim::parse_name(data);
});
