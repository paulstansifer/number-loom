use std::path::PathBuf;

use criterion::{Criterion, criterion_group, criterion_main};

use number_loom::grid_solve::{SolveOptions, solve};
use number_loom::import::load_path;

fn criterion_benchmark(c: &mut Criterion) {
    let mut dust_40_doc =
        load_path(&PathBuf::from("examples/png/tedious_dust_40x40.png"), None).unwrap();
    let dust_40 = dust_40_doc.puzzle().as_square_nono().unwrap();
    let options = SolveOptions::default();

    c.bench_function("tedious_dust_40", |b| {
        b.iter(|| solve(std::hint::black_box(&dust_40.clone()), &mut None, &options));
    });

    let mut fire_sub_doc =
        load_path(&PathBuf::from("examples/png/fire_submarine.png"), None).unwrap();
    let fire_sub = fire_sub_doc.puzzle().as_square_nono().unwrap();

    c.bench_function("fire_sub", |b| {
        b.iter(|| solve(std::hint::black_box(&fire_sub.clone()), &mut None, &options));
    });

    // Triddlers can't load from PNG (out of scope), so this needs a webpbn or Olsak source file.
    let mut hexagon_doc = load_path(&PathBuf::from("examples/triddler/blob.g"), None).unwrap();
    let hexagon = hexagon_doc.puzzle().as_tri_nono().unwrap();

    c.bench_function("hexagon_side_4", |b| {
        b.iter(|| solve(std::hint::black_box(&hexagon.clone()), &mut None, &options));
    });
}

criterion_group!(name=benches;
     config = Criterion::default().sample_size(75);
     targets = criterion_benchmark);
criterion_main!(benches);
