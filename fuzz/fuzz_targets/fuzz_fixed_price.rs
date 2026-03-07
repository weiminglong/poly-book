#![no_main]
use libfuzzer_sys::fuzz_target;
use pb_types::{FixedPrice, FixedSize};

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = FixedPrice::try_from(s);
        let _ = FixedSize::try_from(s);

        if let Ok(p) = FixedPrice::try_from(s) {
            let json = serde_json::to_string(&p).unwrap();
            let p2: FixedPrice = serde_json::from_str(&json).unwrap();
            assert_eq!(p, p2, "serde roundtrip invariant violated");
        }
    }

    if data.len() >= 4 {
        let raw = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let _ = FixedPrice::new(raw);
    }
});
