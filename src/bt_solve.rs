

use priority_queue::PriorityQueue;


#[derive(PartialEq, Eq, Hash, Debug, Clone)]
struct HypoCoord<G: GridKind> {
    guesses: Vec<(G::Coord, Color)>
}


struct BtSearchState<G: GridKind> {
    /// Need interior mutability so the parent references can be `&` 
    knowledge: RefCell<solveState>,

    depth: u8
}

pub fn backtrack_solve<C: Clue, K: GridKind>(
    puzzle: &Puzzle<C, K>,
    options: &SolveOptions,
    grid: &mut PartialSolution,
) -> anyhow::Result<Report> {

    let mut grid = vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()];
    let mut line_cache: LineCache<C> = LineCache::new();

    let mut ctx = SolveContext::new(puzzle, line_cache, options);
    let mut state = SolveState::new(&mut ctx, std::mem::take(grid));

    let outcome = state.run(&mut ctx);
    ctx.progress.finish_and_clear();

    let report = outcome.map(|_| state.report(puzzle));
    // Hand the caller's grid back even on failure, so it still sees how far we got.
    *grid = std::mem::take(&mut state.grid);
    report
}
