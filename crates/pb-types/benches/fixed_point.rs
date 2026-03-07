use criterion::{criterion_group, criterion_main, Criterion};
use pb_types::{FixedPrice, FixedSize};
use std::hint::black_box;

fn bench_fixed_price_from_f64(c: &mut Criterion) {
    c.bench_function("FixedPrice::from_f64", |b| {
        b.iter(|| FixedPrice::from_f64(black_box(0.5432)))
    });
}

fn bench_fixed_price_from_str(c: &mut Criterion) {
    c.bench_function("FixedPrice::try_from str", |b| {
        b.iter(|| FixedPrice::try_from(black_box("0.5432")))
    });
}

fn bench_fixed_price_comparison(c: &mut Criterion) {
    let a = FixedPrice::from_f64(0.30).unwrap();
    let b = FixedPrice::from_f64(0.70).unwrap();
    c.bench_function("FixedPrice comparison", |bench| {
        bench.iter(|| black_box(a) < black_box(b))
    });
}

fn bench_fixed_size_from_f64(c: &mut Criterion) {
    c.bench_function("FixedSize::from_f64", |b| {
        b.iter(|| FixedSize::from_f64(black_box(12345.6789)))
    });
}

fn bench_fixed_price_serde_roundtrip(c: &mut Criterion) {
    let p = FixedPrice::from_f64(0.5).unwrap();
    c.bench_function("FixedPrice serde roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&p)).unwrap();
            let _: FixedPrice = serde_json::from_str(&json).unwrap();
        })
    });
}

criterion_group!(
    benches,
    bench_fixed_price_from_f64,
    bench_fixed_price_from_str,
    bench_fixed_price_comparison,
    bench_fixed_size_from_f64,
    bench_fixed_price_serde_roundtrip,
);
criterion_main!(benches);
