//! Benchmarks `number-loom`'s solver against `pbnsolve`, Jan Wolter's reference implementation
//! from the Survey of Paint-by-Number Puzzle Solvers (<http://webpbn.com/survey/>).
//!
//! ```text
//! cargo run --release --features bench-pbnsolve --bin bench-pbnsolve -- --pbnsolve ~/others/pbnsolve-1.10/pbnsolve
//! ```
//!
//! Both sides are timed on the *solve alone*: `number-loom` runs in-process with an `Instant`
//! around the solve call, and `pbnsolve` reports its own `Processing Time`, which likewise
//! excludes parsing the file. Comparing whole-process wall clock instead would drown a
//! microsecond-scale line solve in a millisecond of process startup — `--verbose` shows those
//! numbers anyway, as a sanity check that the two clocks tell the same story.
//!
//! `--mode backtrack` benchmarks a smaller set than `--mode line` does: puzzles line logic
//! finishes by itself never reach the backtracker's guessing, and the handful in `TOO_DIFFICULT`
//! only ever spend `--loom-timeout` and report that they did. See `for_backtracking`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use clap::Parser;
use number_loom::formats::webpbn::as_webpbn;
use number_loom::puzzle::{DynPuzzle, PuzzleDynOps};
use number_loom::solve::bt_solve::{PickerMix, ScoreKind, backtrack_solve};
use number_loom::solve::grid_solve::SolveOptions;
use number_loom::{import, with_puzzle};

#[derive(clap::ValueEnum, Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// `number-loom`'s line logic head-to-head against `pbnsolve`'s.
    Line,
    /// `pbnsolve`'s search, on its own, as a baseline for the backtracker to beat. Both sides
    /// prove the solution unique here (see `run_pbnsolve`'s `check_unique`).
    Backtrack,
}

#[derive(Parser, Debug)]
#[command(about = "Benchmark number-loom's solver against pbnsolve", long_about = None)]
struct Args {
    /// Path to the `pbnsolve` binary.
    #[arg(long)]
    pbnsolve: PathBuf,

    /// Puzzles to benchmark. Directories are scanned (non-recursively) for puzzle files.
    #[arg(default_value = "examples/wolter")]
    puzzles: Vec<PathBuf>,

    /// Which comparison to run.
    #[arg(long, value_enum, default_value = "line")]
    mode: Mode,

    /// Algorithms to pass to `pbnsolve` as `-a<ALGORITHM>`. Defaults to `LE` in line mode, and in
    /// backtrack mode to leaving the flag off entirely, which gets pbnsolve's own tuned `LHEGP`.
    #[arg(long)]
    pbn_algorithm: Option<String>,

    /// How many times to run each solver; the fastest run of each is the one reported. Defaults to
    /// 5 in line mode and 1 in backtrack mode, where a single run can already take the timeout.
    #[arg(long)]
    reps: Option<u32>,

    /// Seconds of CPU time to allow `pbnsolve` per run (its `-x` flag).
    #[arg(long, default_value_t = 30)]
    timeout: u64,

    /// Show whole-process wall-clock columns alongside the solve-only times.
    #[arg(long)]
    verbose: bool,

    /// Also write the results as CSV, for tracking across commits.
    #[arg(long)]
    csv: Option<PathBuf>,

    /// Seconds of wall clock to allow our own backtracker per puzzle, in backtrack mode. Unlike
    /// pbnsolve's `-x`, this isn't a budget the solver honors: `backtrack_solve` has no deadline
    /// or guess limit to hand it, so the only way to stop one is to kill the process running it.
    /// That is also why it runs out-of-process (see `run_loom_backtrack`), and why this default
    /// is much shorter than `--timeout`: a search that doesn't terminate allocates a fresh copy
    /// of the grid per node the whole time it runs.
    #[arg(long, default_value_t = 10)]
    loom_timeout: u64,

    /// Which guessing heuristic our backtracker uses, in backtrack mode. Takes a rotation as
    /// well as a single name: `disagreement:3,random:1` guesses three times one way and once the
    /// other, over and over. Defaults to whatever `SolveOptions` does, so that the benchmark
    /// measures the solver as shipped.
    #[arg(long)]
    picker: Option<PickerMix>,

    /// In backtrack mode, benchmark the puzzles in `TOO_DIFFICULT` as well. Off by default: each
    /// of them costs a full `--loom-timeout` and reports nothing but that it ran out.
    #[arg(long)]
    include_difficult: bool,

    /// Which node-scoring function orders our backtracker's queue, in backtrack mode. Defaults
    /// to whatever `SolveOptions` does.
    #[arg(long, value_enum)]
    scorer: Option<ScoreKind>,

    /// Not for humans: solve one puzzle with `backtrack_solve` and print a line of counters. The
    /// benchmark re-runs itself this way to bound a search it can't otherwise interrupt.
    #[arg(long, hide = true)]
    solve_backtrack: Option<PathBuf>,
}

impl Args {
    fn algorithm(&self) -> Option<String> {
        match (&self.pbn_algorithm, self.mode) {
            (Some(explicit), _) => Some(explicit.clone()),
            (None, Mode::Line) => Some("LE".to_string()),
            (None, Mode::Backtrack) => None,
        }
    }

    fn reps(&self) -> u32 {
        self.reps.unwrap_or(match self.mode {
            Mode::Line => 5,
            Mode::Backtrack => 1,
        })
    }
}

/// What `pbnsolve -b -t` reports. Every counter but `status` and `seconds` is absent unless the
/// algorithms in play produce it, so they all default to zero.
#[derive(Debug, Default, PartialEq)]
struct PbnReport {
    /// Words from the brief-output line: `unique`, `line`, `stalled`, `timeout`, `contradiction`…
    status: String,
    cells_solved: usize,
    cells_total: usize,
    lines_processed: usize,
    exhaust_cells: usize,
    guesses: usize,
    backtracks: usize,
    /// `pbnsolve`'s own clock. Zero means "faster than it can measure", not "instant".
    seconds: f64,
}

/// Pulls the numbers out of `pbnsolve -b -t` output. Kept separate from running the binary so it
/// can be tested without one installed.
fn parse_pbnsolve(stdout: &str) -> anyhow::Result<PbnReport> {
    let mut report = PbnReport::default();
    let mut saw_cells = false;

    for line in stdout.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Cells Solved:") {
            // "1156 of 1156"
            let mut words = rest.split_whitespace();
            report.cells_solved = parse_at(&mut words, line)?;
            let _of = words.next();
            report.cells_total = parse_at(&mut words, line)?;
            saw_cells = true;
        } else if let Some(rest) = line.strip_prefix("Lines Processed:") {
            report.lines_processed = parse_at(&mut rest.split_whitespace(), line)?;
        } else if let Some(rest) = line.strip_prefix("Exhaustive Search:") {
            report.exhaust_cells = parse_at(&mut rest.split_whitespace(), line)?;
        } else if let Some(rest) = line.strip_prefix("Backtracking:") {
            // "0 guesses, 0 backtracks"
            let mut words = rest.split_whitespace();
            report.guesses = parse_at(&mut words, line)?;
            let _guesses = words.next();
            report.backtracks = parse_at(&mut words, line)?;
        } else if let Some(rest) = line.strip_prefix("Processing Time:") {
            report.seconds = rest
                .split_whitespace()
                .next()
                .and_then(|w| w.parse().ok())
                .with_context(|| format!("couldn't read a time out of {line:?}"))?;
        } else if is_status_line(line) {
            report.status = line.to_string();
        }
    }

    if !saw_cells {
        bail!("no 'Cells Solved:' line in pbnsolve's output; got:\n{stdout}");
    }
    Ok(report)
}

/// The words `-b` can emit. The line before them is the puzzle's title, which can say anything at
/// all, so recognizing the vocabulary is the only way to tell the two apart.
fn is_status_line(line: &str) -> bool {
    const STATUS_WORDS: &[&str] = &[
        "unique",
        "multiple",
        "line",
        "depth-2",
        "trivial",
        "solvable",
        "contradiction",
        "timeout",
        "stalled",
        "logical",
    ];
    !line.is_empty() && line.split_whitespace().all(|w| STATUS_WORDS.contains(&w))
}

fn parse_at<'a>(words: &mut impl Iterator<Item = &'a str>, line: &str) -> anyhow::Result<usize> {
    words
        .next()
        .and_then(|w| w.parse().ok())
        .with_context(|| format!("couldn't read a number out of {line:?}"))
}

/// One `pbnsolve` run that produced no usable report.
enum PbnFailure {
    /// Killed by a signal — `-aE` segfaults on anything past a toy puzzle, for instance.
    Crashed(String),
    /// `-x` spent its budget. pbnsolve sometimes reports this itself, as a clean `timeout` status
    /// line, and sometimes dies to `SIGXCPU` mid-search and tells us nothing at all.
    CpuLimit,
    /// Ran past our wall-clock patience, having failed to stop at its own `-x` limit.
    Killed,
    /// Ran, but said something we couldn't read.
    Unreadable(String),
}

impl PbnFailure {
    fn label(&self) -> String {
        match self {
            PbnFailure::Crashed(how) => format!("CRASH({how})"),
            PbnFailure::CpuLimit => "TIMEOUT(cpu)".to_string(),
            PbnFailure::Killed => "TIMEOUT(wall)".to_string(),
            PbnFailure::Unreadable(why) => {
                format!("UNREADABLE ({})", why.lines().next().unwrap_or(""))
            }
        }
    }
}

/// `-x` is an `RLIMIT_CPU`, so overrunning it arrives as a signal rather than as anything pbnsolve
/// gets to say — and because it sets the hard limit alongside the soft one, what actually lands is
/// usually `SIGKILL` rather than the `SIGXCPU` you'd expect. Telling that apart from a genuine
/// crash is the difference between "this puzzle is too hard for the budget" and "the solver is
/// broken", so `SIGKILL` only counts as a timeout if it arrived when the budget was about spent.
/// (`-aE` segfaulting on its first big puzzle must stay loudly distinguishable.)
#[cfg(unix)]
fn is_cpu_limit(status: &std::process::ExitStatus, elapsed: Duration, timeout: u64) -> bool {
    use std::os::unix::process::ExitStatusExt;
    const SIGXCPU: i32 = 24;
    const SIGKILL: i32 = 9;

    match status.signal() {
        Some(SIGXCPU) => true,
        Some(SIGKILL) => elapsed.as_secs_f64() >= timeout as f64 * 0.9,
        _ => false,
    }
}

#[cfg(not(unix))]
fn is_cpu_limit(_status: &std::process::ExitStatus, _elapsed: Duration, _timeout: u64) -> bool {
    false
}

/// The most likely explanation in a stream pbnsolve wrote, skipping libxml2's four-line lament
/// about the DTD it couldn't fetch — which is present on nearly every run and never the reason
/// anything failed.
fn complaint(text: &str) -> Option<String> {
    const LIBXML_NOISE: &[&str] = &[
        "I/O warning",
        "failed to load external entity",
        "Unknown IO error",
        "<!DOCTYPE",
    ];
    text.lines()
        .map(str::trim)
        .rfind(|line| {
            !line.is_empty()
                && !line.chars().all(|c| c == '^')
                && !LIBXML_NOISE.iter().any(|noise| line.contains(noise))
        })
        .map(str::to_string)
}

/// Runs `pbnsolve` once, returning its report and how long the whole process took.
///
/// `check_unique` is `-u`, and it has to match what our own side is doing or the two aren't being
/// asked the same question. Left off, pbnsolve stops at the first solution it finds and reports
/// `solvable`.
fn run_pbnsolve(
    binary: &Path,
    xml: &Path,
    algorithm: Option<&str>,
    timeout: u64,
    check_unique: bool,
) -> Result<(PbnReport, Duration), PbnFailure> {
    let mut command = Command::new(binary);
    // `-b` for brief output, `-t` for the counters and its own clock.
    command.arg("-b").arg("-t").arg(format!("-x{timeout}"));
    if check_unique {
        command.arg("-u");
    }
    if let Some(algorithm) = algorithm {
        command.arg(format!("-a{algorithm}"));
    }
    command.arg(xml);
    // Keep stderr: on a successful run it's just libxml2 grumbling about not fetching the DTD, but
    // when pbnsolve refuses a puzzle its explanation goes there too.
    command.stderr(std::process::Stdio::piped());
    command.stdout(std::process::Stdio::piped());

    let start = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|e| PbnFailure::Crashed(e.to_string()))?;

    // `-x` bounds pbnsolve's *CPU* time, and it only notices between search nodes, so a run can
    // overshoot the limit by a lot. Stand over it with a wall clock too, or one stubborn puzzle
    // stalls the whole sweep. With `-b` its output is a dozen short lines, far under the pipe
    // buffer, so nothing deadlocks while we wait to read it.
    let deadline = Duration::from_secs(timeout).saturating_mul(2) + Duration::from_secs(5);
    let mut killed = false;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(e) => return Err(PbnFailure::Crashed(e.to_string())),
        }
        if start.elapsed() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            killed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    if killed {
        return Err(PbnFailure::Killed);
    }

    let output = child
        .wait_with_output()
        .map_err(|e| PbnFailure::Crashed(e.to_string()))?;
    let elapsed = start.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout);

    if !output.status.success() {
        if is_cpu_limit(&output.status, elapsed, timeout) {
            return Err(PbnFailure::CpuLimit);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(PbnFailure::Crashed(match output.status.code() {
            // pbnsolve says why it's giving up before exiting — it greets a triddler with
            // "Haven't implemented this yet!", for instance. Repeating that beats "exit 1".
            Some(code) => match complaint(&stdout).or_else(|| complaint(&stderr)) {
                Some(said) => format!("exit {code}: {said}"),
                None => format!("exit {code}"),
            },
            None => "signal".to_string(),
        }));
    }

    match parse_pbnsolve(&stdout) {
        Ok(report) => Ok((report, elapsed)),
        Err(e) => Err(PbnFailure::Unreadable(e.to_string())),
    }
}

/// The best of `reps` runs, plus the report from that run.
fn best_pbnsolve_run(
    binary: &Path,
    xml: &Path,
    algorithm: Option<&str>,
    timeout: u64,
    reps: u32,
    check_unique: bool,
) -> Result<(PbnReport, Duration), PbnFailure> {
    let mut best: Option<(PbnReport, Duration)> = None;
    for _ in 0..reps {
        let (report, wall) = run_pbnsolve(binary, xml, algorithm, timeout, check_unique)?;
        // Compare on pbnsolve's own clock, which is what gets reported; wall clock rides along.
        let better = best
            .as_ref()
            .is_none_or(|(best_report, _)| report.seconds < best_report.seconds);
        if better {
            best = Some((report, wall));
        }
        // A run that hit the CPU limit won't get faster, and each retry costs another timeout.
        if best
            .as_ref()
            .is_some_and(|(r, _)| r.status.contains("timeout"))
        {
            break;
        }
    }
    best.ok_or_else(|| PbnFailure::Unreadable("--reps was 0, so nothing ran".to_string()))
}

/// What `number-loom` did with one puzzle.
struct LoomRun {
    solve: Duration,
    cells_left: usize,
    skims: usize,
    scrubs: usize,
}

fn run_number_loom(puzzle: &DynPuzzle, reps: u32) -> anyhow::Result<LoomRun> {
    // Leave `display_cli_progress` off: the spinner would be inside the measurement.
    let options = SolveOptions::default();

    let mut best: Option<LoomRun> = None;
    for _ in 0..reps {
        let start = Instant::now();
        let report = puzzle.solve(/*backtrack=*/ false, &options)?;
        let solve = start.elapsed();

        if best.as_ref().is_none_or(|b| solve < b.solve) {
            best = Some(LoomRun {
                solve,
                cells_left: report.cells_left,
                skims: report.solve_counts.skim,
                scrubs: report.solve_counts.scrub,
            });
        }
    }
    best.context("no repetitions were run")
}

/// What our own backtracker did with one puzzle.
struct LoomBt {
    /// Wall clock around the `backtrack_solve` call in the child, parsing excluded — the same
    /// thing `pbnsolve`'s `Processing Time` measures.
    seconds: f64,
    /// `unique`, `multiple`, or `contradiction`.
    status: String,
    cells_left: usize,
    skims: usize,
    scrubs: usize,
}

/// The `--solve-backtrack` half of the binary: one puzzle, one `backtrack_solve`, one line of
/// counters on stdout for the parent to read back. Nothing here touches `pbnsolve`.
fn solve_backtrack_child(path: &Path, picker: PickerMix, scorer: ScoreKind) -> anyhow::Result<()> {
    let mut document = import::load_path(&path.to_path_buf(), None)
        .with_context(|| format!("couldn't load {}", path.display()))?;
    let options = SolveOptions {
        guess_picker: picker,
        node_scorer: scorer,
        ..SolveOptions::default()
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let start = Instant::now();
    let outcome = with_puzzle!(document.puzzle(), |p| {
        rt.block_on(backtrack_solve(
            p,
            &options,
            std::sync::mpsc::channel().0,
            std::sync::mpsc::channel().1,
        ))
    });
    let seconds = start.elapsed().as_secs_f64();

    // `LOOM` prefixed so a stray line from anywhere else can't be mistaken for the report.
    // `cells_left == 0` indicates a unique solution
    match outcome {
        Ok(report) if report.cells_left == 0 => println!(
            "LOOM unique {seconds} {} {} {}",
            report.cells_left, report.solve_counts.skim, report.solve_counts.scrub
        ),
        Ok(report) => println!(
            "LOOM multiple {seconds} {} {} {}",
            report.cells_left, report.solve_counts.skim, report.solve_counts.scrub
        ),
        Err(_) => println!("LOOM contradiction {seconds} 0 0 0"),
    }
    Ok(())
}

fn parse_loom_backtrack(stdout: &str) -> anyhow::Result<LoomBt> {
    let line = stdout
        .lines()
        .find(|l| l.starts_with("LOOM "))
        .with_context(|| format!("no LOOM line in the child's output; got:\n{stdout}"))?;
    let mut words = line.split_whitespace().skip(1);
    let status = words
        .next()
        .with_context(|| format!("no status in {line:?}"))?
        .to_string();
    let seconds: f64 = words
        .next()
        .and_then(|w| w.parse().ok())
        .with_context(|| format!("no time in {line:?}"))?;
    Ok(LoomBt {
        seconds,
        status,
        cells_left: parse_at(&mut words, line)?,
        skims: parse_at(&mut words, line)?,
        scrubs: parse_at(&mut words, line)?,
    })
}

/// Runs `backtrack_solve` on one puzzle in a child copy of this binary, killed if it overruns
/// `--loom-timeout`.
///
/// Out-of-process because there is no other way to stop it. `pbnsolve` polices itself with `-x`;
/// `backtrack_solve` takes no deadline and exposes no guess budget, and this call blocks the one
/// thread driving it rather than polling it alongside a timer, so an in-process call that doesn't
/// converge takes the whole sweep down with it — and it allocates a clone of the solve state per
/// search node while it does, so a thread abandoned to run in the background would exhaust memory
/// rather than merely waste a core. A child can just be killed.
fn run_loom_backtrack(
    puzzle: &Path,
    picker: &PickerMix,
    scorer: ScoreKind,
    timeout: u64,
) -> Result<LoomBt, PbnFailure> {
    let exe = std::env::current_exe().map_err(|e| PbnFailure::Crashed(e.to_string()))?;

    let mut command = Command::new(exe);
    // `--pbnsolve` is required by the parser and unused by the child; hand it this binary so the
    // path exists. The child returns before anything would look at it.
    command
        .arg("--pbnsolve")
        .arg(std::env::current_exe().unwrap_or_else(|_| PathBuf::from("/")))
        .arg("--picker")
        .arg(picker.to_string())
        .arg("--scorer")
        .arg(scorer.flag_name())
        .arg("--solve-backtrack")
        .arg(puzzle);
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    let start = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|e| PbnFailure::Crashed(e.to_string()))?;

    let deadline = Duration::from_secs(timeout);
    let mut killed = false;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(e) => return Err(PbnFailure::Crashed(e.to_string())),
        }
        if start.elapsed() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            killed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    if killed {
        return Err(PbnFailure::Killed);
    }

    let output = child
        .wait_with_output()
        .map_err(|e| PbnFailure::Crashed(e.to_string()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // A panic (`unreachable!`, an index out of range) and an out-of-memory kill both land
        // here, and telling them apart matters, so pass along whatever the child said.
        return Err(PbnFailure::Crashed(match output.status.code() {
            Some(code) => match complaint(&stderr).or_else(|| complaint(&stdout)) {
                Some(said) => format!("exit {code}: {said}"),
                None => format!("exit {code}"),
            },
            None => "signal".to_string(),
        }));
    }

    parse_loom_backtrack(&stdout).map_err(|e| PbnFailure::Unreadable(e.to_string()))
}

/// One row of the table: either a measurement or a reason there isn't one.
enum Row {
    Line {
        name: String,
        cells: usize,
        loom: LoomRun,
        pbn: PbnReport,
        loom_wall: Duration,
        pbn_wall: Duration,
    },
    Backtrack {
        name: String,
        cells: usize,
        pbn: PbnReport,
        pbn_wall: Duration,
        /// `Err` is the reason there's no measurement — a timeout, a crash, unreadable output.
        loom: Result<LoomBt, String>,
    },
    Skipped {
        name: String,
        why: String,
    },
}

impl Row {
    fn name(&self) -> &str {
        match self {
            Row::Line { name, .. } | Row::Backtrack { name, .. } | Row::Skipped { name, .. } => {
                name
            }
        }
    }
}

/// Collects every puzzle file named on the command line, expanding directories.
fn collect_puzzles(paths: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    let mut res = vec![];
    for path in paths {
        if path.is_dir() {
            let mut in_dir: Vec<PathBuf> = std::fs::read_dir(path)
                .with_context(|| format!("couldn't read the directory {}", path.display()))?
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .filter(|p| p.is_file() && is_puzzle_file(p))
                .collect();
            in_dir.sort();
            res.extend(in_dir);
        } else {
            res.push(path.clone());
        }
    }
    if res.is_empty() {
        bail!("no puzzle files found");
    }
    Ok(res)
}

/// Directories hold READMEs and licenses next to the puzzles, so scanning one has to be choosy.
/// A file named outright on the command line is taken at its word instead.
fn is_puzzle_file(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    matches!(
        ext,
        "xml" | "pbn" | "g" | "png" | "bmp" | "gif" | "txt" | "woven"
    )
}

/// Puzzles our backtracker cannot finish in any sensible amount of time yet. Backtrack mode leaves
/// them out unless `--include-difficult` asks for them; line mode runs them like anything else.
/// Each pattern is matched against the end of the file stem, so `-09892` catches `webpbn-09892`
/// without also catching a hypothetical `webpbn-color-09892`.
const TOO_DIFFICULT: &[&str] = &[
    "faase",
    "knotty",
    "meow",
    "-09892",
    "-10088",
    "-12548",
    "-18297",
    "-22336",
    "-color-00672",
    "-color-03620",
];

fn is_too_difficult(path: &Path) -> bool {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    TOO_DIFFICULT.iter().any(|hard| stem.ends_with(hard))
}

/// Whether line logic alone finishes the puzzle. A puzzle that won't even load counts as "no", so
/// that `bench_one` gets to reach it and report why it wouldn't.
fn solvable_by_line_logic(path: &Path) -> bool {
    let Ok(mut document) = import::load_path(&path.to_path_buf(), None) else {
        return false;
    };
    document
        .puzzle()
        .solve(/*backtrack=*/ false, &SolveOptions::default())
        .is_ok_and(|report| report.cells_left == 0)
}

/// Narrows the puzzle list down to the ones with a search worth timing, and says on stderr what it
/// dropped. Line logic finishing a puzzle on its own means the backtracker never guesses at all,
/// so timing one measures line logic a second time and pulls the summary toward puzzles that
/// aren't what backtrack mode is asking about. (Both solvers agree on which eight of
/// `examples/wolter` those are, so the test costs a line solve rather than a hardcoded list.)
fn for_backtracking(puzzles: &[PathBuf], include_difficult: bool) -> Vec<PathBuf> {
    let mut kept = vec![];
    let mut line_only = vec![];
    let mut difficult = vec![];

    for path in puzzles {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        // Checked before the line solve, since these are the slow ones to load and skim.
        if !include_difficult && is_too_difficult(path) {
            difficult.push(name);
        } else if solvable_by_line_logic(path) {
            line_only.push(name);
        } else {
            kept.push(path.clone());
        }
    }

    if !line_only.is_empty() {
        eprintln!(
            "skipping {} line-logic-only puzzle(s): {}",
            line_only.len(),
            line_only.join(" ")
        );
    }
    if !difficult.is_empty() {
        eprintln!(
            "skipping {} puzzle(s) too hard for the backtracker (--include-difficult keeps them): {}",
            difficult.len(),
            difficult.join(" ")
        );
    }
    kept
}

/// `pbnsolve` reads webpbn XML, so anything else has to be converted first. Returns the path to
/// feed it, and the temp file keeping that path alive, if we made one.
fn webpbn_path_for(
    path: &Path,
    document: &number_loom::puzzle::Document,
    temp_dir: &Path,
) -> anyhow::Result<PathBuf> {
    if path.extension().and_then(|e| e.to_str()) == Some("xml") {
        return Ok(path.to_path_buf());
    }

    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let converted = temp_dir.join(format!("{stem}.xml"));
    std::fs::write(&converted, as_webpbn(document))
        .with_context(|| format!("couldn't write {}", converted.display()))?;
    Ok(converted)
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // The child half of `run_loom_backtrack`: solve one puzzle and say nothing else.
    if let Some(puzzle) = &args.solve_backtrack {
        return solve_backtrack_child(
            puzzle,
            args.picker.clone().unwrap_or_default(),
            args.scorer.unwrap_or_default(),
        );
    }

    if !args.pbnsolve.is_file() {
        bail!("no pbnsolve binary at {}", args.pbnsolve.display());
    }
    let algorithm = args.algorithm();
    let reps = args.reps();

    let mut puzzles = collect_puzzles(&args.puzzles)?;
    if args.mode == Mode::Backtrack {
        puzzles = for_backtracking(&puzzles, args.include_difficult);
        if puzzles.is_empty() {
            bail!("every puzzle named was excluded from backtrack mode");
        }
    }
    let temp_dir = std::env::temp_dir().join(format!("number-loom-bench-{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir)?;

    eprintln!(
        "{} puzzles, {reps} rep(s), pbnsolve {}, timeout {}s",
        puzzles.len(),
        algorithm
            .as_ref()
            .map_or("(its default algorithms)".to_string(), |a| format!("-a{a}")),
        args.timeout,
    );

    let mut rows = vec![];
    for path in &puzzles {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        match bench_one(path, &args, algorithm.as_deref(), reps, &temp_dir) {
            Ok(row) => rows.push(row),
            Err(why) => rows.push(Row::Skipped {
                name,
                why: why.to_string(),
            }),
        }
    }

    let _ = std::fs::remove_dir_all(&temp_dir);

    match args.mode {
        Mode::Line => print_line_table(&rows, args.verbose),
        Mode::Backtrack => print_backtrack_table(&rows),
    }

    if let Some(csv_path) = &args.csv {
        write_csv(csv_path, &rows)
            .with_context(|| format!("couldn't write {}", csv_path.display()))?;
        eprintln!("wrote {}", csv_path.display());
    }

    Ok(())
}

fn bench_one(
    path: &Path,
    args: &Args,
    algorithm: Option<&str>,
    reps: u32,
    temp_dir: &Path,
) -> anyhow::Result<Row> {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let mut document = import::load_path(&path.to_path_buf(), None)
        .with_context(|| format!("couldn't load {}", path.display()))?;

    // Check the shape before converting: `as_webpbn` panics on Triano clues rather than returning
    // an error, because webpbn genuinely can't express them.
    if matches!(document.puzzle(), DynPuzzle::SquareTriano(_)) {
        bail!("trianogram; webpbn can't represent it");
    }
    let cells = with_puzzle!(document.puzzle(), |p| p.geometry.cell_count());

    let xml = webpbn_path_for(path, &document, temp_dir)?;

    let pbn_run = best_pbnsolve_run(
        &args.pbnsolve,
        &xml,
        algorithm,
        args.timeout,
        reps,
        args.mode == Mode::Backtrack,
    );
    let (pbn, pbn_wall) = match pbn_run {
        Ok(both) => both,
        Err(failure) => bail!("pbnsolve: {}", failure.label()),
    };

    match args.mode {
        Mode::Backtrack => Ok(Row::Backtrack {
            name,
            cells,
            pbn,
            pbn_wall,
            // `path`, not `xml`: our own loader reads every format, and converting first would
            // hand the backtracker a puzzle that had made a round trip through webpbn.
            loom: run_loom_backtrack(
                path,
                &args.picker.clone().unwrap_or_default(),
                args.scorer.unwrap_or_default(),
                args.loom_timeout,
            )
            .map_err(|f| f.label()),
        }),
        Mode::Line => {
            let loom_start = Instant::now();
            let loom = run_number_loom(document.puzzle(), reps)?;
            let loom_wall = loom_start.elapsed() / reps;

            Ok(Row::Line {
                name,
                cells,
                loom,
                pbn,
                loom_wall,
                pbn_wall,
            })
        }
    }
}

/// `pbnsolve` reports `0.000000 sec` for anything below its clock's resolution, which no amount
/// of repetition fixes. Such rows are shown but left out of the summary.
fn micros(seconds: f64) -> Option<f64> {
    (seconds > 0.0).then_some(seconds * 1e6)
}

fn print_line_table(rows: &[Row], verbose: bool) {
    let name_width = rows
        .iter()
        .map(|r| r.name().len())
        .max()
        .unwrap_or(20)
        .max(8);

    print!(
        "{:<name_width$} {:>7} {:>10} {:>10} {:>8} {:>9} {:>9} {:>13} {:>9}",
        "puzzle",
        "cells",
        "loom µs",
        "pbn µs",
        "ratio",
        "loom left",
        "pbn left",
        "skims/scrubs",
        "pbn lines",
    );
    if verbose {
        print!(" {:>10} {:>10}", "loom wall", "pbn wall");
    }
    println!();

    let mut ratios = vec![];
    let mut disagreements = 0;
    let mut measured = 0;

    for row in rows {
        match row {
            Row::Line {
                name,
                cells,
                loom,
                pbn,
                loom_wall,
                pbn_wall,
            } => {
                let pbn_left = pbn.cells_total.saturating_sub(pbn.cells_solved);
                let agrees = pbn_left == loom.cells_left;
                if !agrees {
                    disagreements += 1;
                }

                let loom_us = loom.solve.as_secs_f64() * 1e6;
                let (pbn_us_text, ratio_text) = match micros(pbn.seconds) {
                    Some(pbn_us) => {
                        measured += 1;
                        ratios.push(pbn_us / loom_us);
                        (format!("{pbn_us:.1}"), format!("{:.2}x", pbn_us / loom_us))
                    }
                    None => ("<1".to_string(), "-".to_string()),
                };

                print!(
                    "{name:<name_width$} {cells:>7} {loom_us:>10.1} {pbn_us_text:>10} \
                     {ratio_text:>8} {:>9} {:>9} {:>13} {:>9}",
                    loom.cells_left,
                    format!("{}{}", pbn_left, if agrees { "" } else { " ***" }),
                    format!("{}/{}", loom.skims, loom.scrubs),
                    pbn.lines_processed,
                );
                if verbose {
                    print!(
                        " {:>10.1} {:>10.1}",
                        loom_wall.as_secs_f64() * 1e6,
                        pbn_wall.as_secs_f64() * 1e6
                    );
                }
                println!();
            }
            Row::Skipped { name, why } => println!("{name:<name_width$} {why}"),
            Row::Backtrack { .. } => unreachable!("line mode produces no backtrack rows"),
        }
    }

    println!();
    if ratios.is_empty() {
        println!("nothing was slow enough for pbnsolve's clock to measure.");
    } else {
        // Geometric mean: these are ratios, so averaging them arithmetically would let one
        // lopsided puzzle set the answer.
        let log_sum: f64 = ratios.iter().map(|r| r.ln()).sum();
        println!(
            "geometric mean: number-loom is {:.2}x pbnsolve's speed \
             ({measured} of {} puzzles measurable, {disagreements} disagreement(s))",
            (log_sum / ratios.len() as f64).exp(),
            rows.len(),
        );
    }
    if disagreements > 0 {
        println!(
            "*** marks a puzzle where the two solvers left a different number of cells unsolved, \
             so their times aren't comparable there."
        );
    }
}

fn print_backtrack_table(rows: &[Row]) {
    let name_width = rows
        .iter()
        .map(|r| r.name().len())
        .max()
        .unwrap_or(20)
        .max(8);

    // `backtrack_solve` counts no guesses or backtracks of its own yet, so pbnsolve's two search
    // counters have no column to sit beside; what it does report is skims and scrubs, the same
    // pair line mode shows.
    //
    // There is no "pbn left" column here, though line mode has one: under `-u` pbnsolve backtracks
    // out of the solution it found to go looking for a second, so its `Cells Solved` ends up
    // describing wherever the search stopped rather than the answer — `webpbn-00436` solves
    // uniquely and still reports 725 of 1400. The status word is what says how it went.
    println!(
        "{:<name_width$} {:>7} {:>12} {:>12} {:>8} {:>10} {:>13}  {:<14} {}",
        "puzzle",
        "cells",
        "loom sec",
        "pbn sec",
        "ratio",
        "loom left",
        "skims/scrubs",
        "loom",
        "pbn",
    );

    let mut ratios = vec![];
    let mut solved = 0;

    for row in rows {
        match row {
            Row::Backtrack {
                name,
                cells,
                pbn,
                pbn_wall,
                loom,
            } => {
                let pbn_status = if pbn.status.is_empty() {
                    format!("(wall {:.3}s)", pbn_wall.as_secs_f64())
                } else {
                    pbn.status.clone()
                };

                let (loom_sec, loom_left, loom_lines, loom_status, ratio) = match loom {
                    Ok(loom) => {
                        solved += 1;
                        // Only worth a ratio when both clocks actually measured something.
                        let ratio = match micros(pbn.seconds) {
                            Some(pbn_us) if loom.seconds > 0.0 => {
                                let r = pbn_us / (loom.seconds * 1e6);
                                ratios.push(r);
                                format!("{r:.2}x")
                            }
                            _ => "-".to_string(),
                        };
                        (
                            format!("{:.6}", loom.seconds),
                            loom.cells_left.to_string(),
                            format!("{}/{}", loom.skims, loom.scrubs),
                            loom.status.clone(),
                            ratio,
                        )
                    }
                    Err(why) => (
                        "-".to_string(),
                        "-".to_string(),
                        "-".to_string(),
                        why.clone(),
                        "-".to_string(),
                    ),
                };

                println!(
                    "{name:<name_width$} {cells:>7} {loom_sec:>12} {:>12.6} {ratio:>8} \
                     {loom_left:>10} {loom_lines:>13}  {loom_status:<14} {pbn_status}",
                    pbn.seconds,
                );
            }
            Row::Skipped { name, why } => println!("{name:<name_width$} {why}"),
            Row::Line { .. } => unreachable!("backtrack mode produces no line rows"),
        }
    }

    println!();
    println!(
        "{solved} of {} puzzles fully solved by the backtracker",
        rows.len()
    );
    if !ratios.is_empty() {
        // Geometric mean, for the same reason line mode uses one.
        let log_sum: f64 = ratios.iter().map(|r| r.ln()).sum();
        println!(
            "geometric mean over the {} comparable puzzle(s): number-loom is {:.2}x pbnsolve's speed",
            ratios.len(),
            (log_sum / ratios.len() as f64).exp(),
        );
    }
}

fn write_csv(path: &Path, rows: &[Row]) -> anyhow::Result<()> {
    let mut out = String::from(
        "puzzle,cells,loom_seconds,pbn_seconds,loom_cells_left,pbn_cells_left,\
         loom_skims,loom_scrubs,pbn_lines,pbn_guesses,pbn_backtracks,status\n",
    );
    for row in rows {
        match row {
            Row::Line {
                name,
                cells,
                loom,
                pbn,
                ..
            } => out.push_str(&format!(
                "{name},{cells},{},{},{},{},{},{},{},{},{},{}\n",
                loom.solve.as_secs_f64(),
                pbn.seconds,
                loom.cells_left,
                pbn.cells_total.saturating_sub(pbn.cells_solved),
                loom.skims,
                loom.scrubs,
                pbn.lines_processed,
                pbn.guesses,
                pbn.backtracks,
                pbn.status,
            )),
            Row::Backtrack {
                name,
                cells,
                pbn,
                loom,
                ..
            } => {
                let (loom_seconds, loom_left, skims, scrubs, loom_status) = match loom {
                    Ok(loom) => (
                        loom.seconds.to_string(),
                        loom.cells_left.to_string(),
                        loom.skims.to_string(),
                        loom.scrubs.to_string(),
                        loom.status.clone(),
                    ),
                    Err(why) => (
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        why.clone(),
                    ),
                };
                // `pbn_cells_left` is left empty rather than filled in: see `print_backtrack_table`
                // for why `-u` makes pbnsolve's count of them meaningless here.
                out.push_str(&format!(
                    "{name},{cells},{loom_seconds},{},{loom_left},,{skims},{scrubs},{},{},{},\"loom: {loom_status}; pbn: {}\"\n",
                    pbn.seconds,
                    pbn.lines_processed,
                    pbn.guesses,
                    pbn.backtracks,
                    pbn.status,
                ))
            }
            Row::Skipped { name, why } => {
                out.push_str(&format!("{name},,,,,,,,,,,\"skipped: {why}\"\n"))
            }
        }
    }
    std::fs::write(path, out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real output, captured from `pbnsolve -b -t -aLE examples/wolter/webpbn-00016.xml`.
    const SOLVED: &str = "\
#16 (v.1): Probably Not
unique logical
Cells Solved: 1156 of 1156
Lines in Puzzle: 68
Lines Processed: 298 (400%)
Exhaustive Search: 0 cells in 0 passes
Backtracking: 0 guesses, 0 backtracks
Processing Time: 0.000175 sec
";

    #[test]
    fn reads_a_solved_report() {
        let report = parse_pbnsolve(SOLVED).unwrap();
        assert_eq!(report.status, "unique logical");
        assert_eq!(report.cells_solved, 1156);
        assert_eq!(report.cells_total, 1156);
        assert_eq!(report.lines_processed, 298);
        assert_eq!(report.guesses, 0);
        assert_eq!(report.seconds, 0.000175);
    }

    /// The title line is arbitrary text, and here it is made of status words. Taking the *last*
    /// such line keeps the real status from being overwritten by a lookalike title.
    #[test]
    fn a_title_that_looks_like_a_status_doesnt_win() {
        let tricky = SOLVED.replace("#16 (v.1): Probably Not", "unique solvable");
        assert_eq!(parse_pbnsolve(&tricky).unwrap().status, "unique logical");
    }

    #[test]
    fn reads_a_timeout() {
        let timed_out = "\
Knotty Puzzle
timeout
Cells Solved: 79 of 1600
Lines in Puzzle: 80
Lines Processed: 80 (100%)
Backtracking: 4210 guesses, 4102 backtracks
Processing Time: 5.001922 sec
";
        let report = parse_pbnsolve(timed_out).unwrap();
        assert_eq!(report.status, "timeout");
        assert_eq!(report.cells_solved, 79);
        assert_eq!(report.cells_total, 1600);
        assert_eq!(report.guesses, 4210);
        assert_eq!(report.backtracks, 4102);
        // `-aL` alone reports no exhaustive search; the counter stays at its default.
        assert_eq!(report.exhaust_cells, 0);
    }

    #[test]
    fn a_time_below_the_clocks_resolution_is_not_a_speed() {
        let too_fast = SOLVED.replace("0.000175 sec", "0.000000 sec");
        assert_eq!(parse_pbnsolve(&too_fast).unwrap().seconds, 0.0);
        assert_eq!(micros(0.0), None);
        assert_eq!(micros(0.000175), Some(175.0));
    }

    #[test]
    fn garbage_is_an_error_rather_than_a_zero() {
        // The shape of what pbnsolve prints when it can't open the file at all.
        assert!(parse_pbnsolve("Could not load puzzle from STDIN\n").is_err());
        assert!(parse_pbnsolve("").is_err());
        // Truncated mid-report: the counters we need never arrive.
        assert!(parse_pbnsolve("Knotty Puzzle\nstalled\n").is_err());
    }

    /// Verbatim stderr from a run on `examples/wolter`, where the DTD lament is the only thing
    /// present and so must not be mistaken for an explanation of anything.
    #[test]
    fn the_dtd_lament_is_not_a_complaint() {
        let noise = "\
error : Unknown IO error
examples/wolter/knotty.xml:2: I/O warning : failed to load external entity \"http://webpbn.com/pbn-0.3.dtd\"
<!DOCTYPE pbn SYSTEM \"http://webpbn.com/pbn-0.3.dtd\">
                                                     ^
";
        assert_eq!(complaint(noise), None);
        assert_eq!(complaint(""), None);

        // The real message, when there is one, survives the same filter.
        let refusal = format!("{noise}Haven't implemented this yet!\n");
        assert_eq!(
            complaint(&refusal).as_deref(),
            Some("Haven't implemented this yet!")
        );
    }

    #[test]
    fn the_difficult_list_matches_whole_puzzle_names() {
        assert!(is_too_difficult(Path::new("examples/wolter/knotty.xml")));
        assert!(is_too_difficult(Path::new(
            "examples/wolter/webpbn-09892.xml"
        )));
        assert!(is_too_difficult(Path::new(
            "examples/wolter/webpbn-color-00672.xml"
        )));

        assert!(!is_too_difficult(Path::new("examples/wolter/meow-two.xml")));
        assert!(!is_too_difficult(Path::new(
            "examples/wolter/webpbn-00672.xml"
        )));
        assert!(!is_too_difficult(Path::new(
            "examples/wolter/webpbn-22336.xml"
        )));
    }

    /// The eight puzzles both solvers finish with line logic alone, and one they don't, so that a
    /// line solver that quietly stopped finishing them would be noticed here.
    #[test]
    fn line_logic_only_puzzles_are_recognized() {
        for name in [
            "webpbn-00001",
            "webpbn-00006",
            "webpbn-00016",
            "webpbn-00021",
            "webpbn-00529",
            "webpbn-07604",
            "webpbn-color-00047",
            "webpbn-color-00220",
        ] {
            let path = PathBuf::from(format!("examples/wolter/{name}.xml"));
            assert!(solvable_by_line_logic(&path), "{name} should be line-only");
        }
        assert!(!solvable_by_line_logic(Path::new(
            "examples/wolter/webpbn-00023.xml"
        )));
    }

    #[test]
    fn backtrack_mode_drops_the_line_only_and_the_difficult() {
        let puzzles = collect_puzzles(&[PathBuf::from("examples/wolter")]).unwrap();

        let kept = for_backtracking(&puzzles, /*include_difficult=*/ false);
        assert_eq!(kept.len(), puzzles.len() - 8 - 9);
        assert!(!kept.iter().any(|p| p.ends_with("webpbn-00001.xml")));
        assert!(!kept.iter().any(|p| p.ends_with("knotty.xml")));
        assert!(kept.iter().any(|p| p.ends_with("webpbn-00023.xml")));

        // `--include-difficult` puts back the nine, and only the nine.
        let with_hard = for_backtracking(&puzzles, /*include_difficult=*/ true);
        assert_eq!(with_hard.len(), kept.len() + 9);
        assert!(with_hard.iter().any(|p| p.ends_with("knotty.xml")));
        assert!(!with_hard.iter().any(|p| p.ends_with("webpbn-00001.xml")));
    }

    #[test]
    fn only_puzzle_shaped_files_are_scanned_out_of_a_directory() {
        assert!(is_puzzle_file(Path::new("examples/wolter/knotty.xml")));
        assert!(is_puzzle_file(Path::new("examples/png/tea.png")));
        assert!(is_puzzle_file(Path::new("examples/triddler/blob.g")));
        assert!(!is_puzzle_file(Path::new("examples/wolter/README")));
        assert!(!is_puzzle_file(Path::new("examples/triddler/README.md")));
        assert!(!is_puzzle_file(Path::new("examples/wolter/LICENSE")));
    }
}
