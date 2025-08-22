use std::path::PathBuf;

use criterion::{Criterion, criterion_group, criterion_main};

use number_loom::grid_solve::solve;
use number_loom::import::load_path;

fn criterion_benchmark(c: &mut Criterion) {
    let mut dust_40_doc = load_path(&PathBuf::from("examples/png/tedious_dust_40x40.png"), None);
    let dust_40 = dust_40_doc.puzzle().assume_nono();

    c.bench_function("tedious_dust_40", |b| {
        b.iter(|| solve(std::hint::black_box(&dust_40.clone()), &mut None, false));
    });

    let mut fire_sub_doc = load_path(&PathBuf::from("examples/png/fire_submarine.png"), None);
    let fire_sub = fire_sub_doc.puzzle().assume_nono();

    c.bench_function("tedious_fire_sub", |b| {
        b.iter(|| solve(std::hint::black_box(&fire_sub.clone()), &mut None, false));
    });
}

criterion_group!(name=benches;
     config = Criterion::default().sample_size(75);
     targets = criterion_benchmark);
criterion_main!(benches);
