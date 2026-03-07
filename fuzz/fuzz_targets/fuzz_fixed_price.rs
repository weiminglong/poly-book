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
            assert_eq!(p, p2, "FixedPrice serde roundtrip violated");
        }

        if let Ok(s_val) = FixedSize::try_from(s) {
            let json = serde_json::to_string(&s_val).unwrap();
            let s2: FixedSize = serde_json::from_str(&json).unwrap();
            assert_eq!(s_val, s2, "FixedSize serde roundtrip violated");
        }
    }

    if data.len() >= 4 {
        let raw = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        if let Ok(p) = FixedPrice::new(raw) {
            assert!(p.raw() <= 10_000, "FixedPrice raw exceeds max");
            assert_eq!(p.raw(), raw);
        } else {
            assert!(raw > 10_000, "FixedPrice::new rejected valid raw");
        }
    }

    if data.len() >= 8 {
        let raw = u64::from_le_bytes([
            data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
        ]);
        let s = FixedSize::new(raw);
        assert_eq!(s.raw(), raw);
        assert_eq!(s.is_zero(), raw == 0);
    }
});
