//! A budget on how much a single solver step is allowed to allocate.
//!
//! This exists because the solver once spent most of its time on work it threw away: `anyhow`'s
//! `.context(c)` takes its argument *by value*, so `.context(format!(...))` on a hot success path
//! runs the `format!` every time, and two of those sat inside per-cell loops in `skim_line`.
//! Fixing that roughly halved the solver's running time.
//!
//! Allocation count is a cheap, deterministic proxy that catches the whole family of mistakes,
//! and it is far more sensitive than wall-clock (those live in `benches/`). Steps are counted
//! rather than whole solves so that per-solve setup — building the lane table, rendering the
//! answer — doesn't drown out the per-step signal.
//!
//! If this fires, look for an eager `format!` (use `.with_context(|| ...)`) or a nested `Vec`
//! that could be one flat table.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use number_loom::grid_solve::{SolveContext, SolveOptions, SolveState, Step};
use number_loom::import::load_path;
use number_loom::line_solve::Cell;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// A step currently allocates around four: two for `packed_extents`, which `skim_line` runs
/// twice and which returns a fresh `Vec` each time, one for the pre-solve copy of the lane, and
/// `exhaust_line`'s tables on the steps that scrub. Threading caller-owned buffers through to
/// remove them was measured at only 1-6%, and cost more API than it was worth.
///
/// So the budget is set to leave that handful room, not to demand zero. What it is really for is
/// the regression it was written after: the same measurement against the code before the
/// `.context(format!(...))` fix reports 41.7 allocations per step.
const BUDGET_PER_STEP: f64 = 8.0;

#[test]
fn stepping_the_solver_stays_within_its_allocation_budget() {
    for path in [
        "examples/png/tedious_dust_40x40.png",
        "examples/png/fire_submarine.png",
    ] {
        let mut document = load_path(&PathBuf::from(path), None).unwrap();
        let puzzle = document.puzzle().as_square_nono().unwrap().clone();
        let options = SolveOptions::default();
        let mut line_cache = None;

        let mut ctx = SolveContext::new(&puzzle, &mut line_cache, &options);
        let grid = vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()];
        let mut state = SolveState::new(&mut ctx, grid);

        // Untimed warm-up, so the buffers reach their working size before anything is charged.
        for _ in 0..20 {
            let _ = state.step(&mut ctx);
        }

        let mut steps = 0usize;
        let before = ALLOCATIONS.load(Relaxed);
        while let Ok(Step::Attempted) = state.step(&mut ctx) {
            steps += 1;
        }
        let allocations = ALLOCATIONS.load(Relaxed) - before;

        assert!(
            steps > 50,
            "{path}: only {steps} steps, too few to mean much"
        );

        let per_step = allocations as f64 / steps as f64;
        println!("{path}: {allocations} allocations over {steps} steps ({per_step:.3} each)");
        assert!(
            per_step < BUDGET_PER_STEP,
            "{path}: {per_step:.3} allocations per step exceeds the budget of {BUDGET_PER_STEP}"
        );
    }
}
