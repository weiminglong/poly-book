#![no_main]
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use pb_service::{guard_sql, QueryGuard};

#[derive(Arbitrary, Debug)]
struct FuzzInput {
    sql: String,
    max_rows: u16,
}

fuzz_target!(|input: FuzzInput| {
    let guard = QueryGuard {
        max_rows: input.max_rows as usize,
        timeout_secs: 1,
    };

    let result = guard_sql(&input.sql, &guard);

    if let Ok(guarded_sql) = result {
        assert!(!guarded_sql.trim().is_empty(), "guarded SQL must not be empty");
        assert!(
            !guarded_sql.trim_end().ends_with(';'),
            "guarded SQL should not retain a trailing semicolon"
        );

        let second_pass = guard_sql(&guarded_sql, &guard).expect("second guard pass failed");
        assert_eq!(
            guarded_sql, second_pass,
            "guarded SQL should be stable across repeated normalization"
        );
    }
});
