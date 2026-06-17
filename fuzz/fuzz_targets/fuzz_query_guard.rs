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

        // The meaningful safety invariant is that the guarded SQL is a fixed
        // point of the guard: re-guarding it must succeed and return it byte for
        // byte. This subsumes "no trailing statement separator" — a trailing
        // *unquoted* semicolon would be stripped by the second pass's LIMIT
        // injection, breaking this equality. (A `;` inside a trailing comment or
        // string literal is part of a single valid statement and is harmless, so
        // it is deliberately not rejected.)
        let second_pass = guard_sql(&guarded_sql, &guard).expect("second guard pass failed");
        assert_eq!(
            guarded_sql, second_pass,
            "guarded SQL should be stable across repeated normalization"
        );
    }
});
