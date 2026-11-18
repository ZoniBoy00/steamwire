#![no_main]

use libfuzzer_sys::fuzz_target;
use steamwire::kv::KeyValue;

fuzz_target!(|data: &[u8]| {
    let _ = KeyValue::parse_binary_all(data);
    if let Ok(kv) = KeyValue::parse_binary(data) {
        let _ = kv.to_text();
    }
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(kv) = KeyValue::parse_text(text) {
            let _ = kv.to_binary();
        }
    }
});
