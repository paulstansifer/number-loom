//! A budget on heap allocations per solver step.
//!
//! This exists because the solver was once spending most of its time formatting error messages
//! that were immediately discarded: `anyhow`'s `.context(c)` takes its argument *by value*, so a
//! `.context(format!(...))` on a hot success path runs the `format!` every time. Two of those sat
//! inside per-cell loops in `skim_line`. Allocation count is a cheap, deterministic proxy that
//! catches the whole family of mistakes; wall-clock benchmarks live in `benches/`.
//!
//! If this fires, look for eager `format!` (use `.with_context(|| ...)`), a `Vec` built per call
//! where a reusable buffer would do, or a nested `Vec` that could be one flat table.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use number_loom::grid_solve::{SolveOptions, solve};
use number_loom::import::load_path;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Generous enough not to trip on incidental churn, tight enough to catch the ~43-per-step
/// regression this was written for. Measured at ~5 (dust) and ~9 (submarine).
const BUDGET_PER_STEP: f64 = 20.0;

#[test]
fn solving_stays_within_its_allocation_budget() {
    for path in [
        "examples/png/tedious_dust_40x40.png",
        "examples/png/fire_submarine.png",
    ] {
        let mut document = load_path(&PathBuf::from(path), None).unwrap();
        let puzzle = document.puzzle().as_square_nono().unwrap().clone();
        let options = SolveOptions::default();

        // An untimed warm-up, so lazily-initialized odds and ends aren't charged to the count.
        solve(&puzzle, &mut None, &options).unwrap();

        let before = ALLOCS.load(Relaxed);
        let report = solve(&puzzle, &mut None, &options).unwrap();
        let allocations = ALLOCS.load(Relaxed) - before;

        let steps = report.solve_counts.skim + report.solve_counts.scrub;
        let per_step = allocations as f64 / steps as f64;
        println!("{path}: {allocations} allocations over {steps} steps ({per_step:.1} each)");

        assert!(
            per_step < BUDGET_PER_STEP,
            "{path}: {per_step:.1} allocations per step exceeds the budget of {BUDGET_PER_STEP}"
        );
    }
}
