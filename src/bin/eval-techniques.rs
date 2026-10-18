//! Compares line-solving techniques against each other, on random lines that `exhaust_line` can
//! make progress on: how often does each one get anywhere, and which ones make which others
//! redundant?
//!
//! ```text
//! cargo run --release --features eval-techniques --bin eval-techniques -- --examples 3
//! ```
//!
//! The lines are meant to look like what a line solver sees in the middle of a puzzle. Each one
//! starts as a random solved line with some of its cells revealed, and gets solved as far as line
//! logic can take it. Then a few more cells are revealed, as though the crossing lines had made
//! some progress since this one was last looked at. Lines where that doesn't tell `exhaust_line`
//! anything new are thrown out.
//!
//! `rule_out_finished_colors` is free: nobody needs a technique to stop painting once they've run
//! out of clues. So it happens before a line gets evaluated at all, and after every step when
//! techniques are taking turns.
//!
//! "Progress" always means ruling out a color for a cell. In a black-and-white puzzle, that's the
//! same thing as solving the cell.

use anyhow::bail;
use clap::Parser;
use number_loom::puzzle::{BACKGROUND, Color, Nono};
use number_loom::solve::human_line_solver::{Technique, rule_out_finished_colors};
use number_loom::solve::line_solve::{Cell, exhaust_line, scrub_line, settle_line, skim_line};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{RngExt, SeedableRng};

#[derive(clap::Parser, Debug)]
#[command(about = "Compares line-solving techniques on random partly-solved lines")]
struct Args {
    /// How many lines to evaluate (lines that `exhaust` can't progress don't count)
    #[arg(long, default_value_t = 100_000)]
    lines: usize,

    #[arg(long, default_value_t = 0)]
    seed: u64,

    /// Shortest line to generate
    #[arg(long, default_value_t = 10)]
    min_len: usize,

    /// Longest line to generate
    #[arg(long, default_value_t = 30)]
    max_len: usize,

    /// How many foreground colors the lines have
    #[arg(long, default_value_t = 1)]
    colors: u8,

    /// The most cells that get revealed between solving the line and evaluating it
    #[arg(long, default_value_t = 3)]
    max_new_cells: usize,

    /// Show this many example lines of each kind of shortfall
    #[arg(long, default_value_t = 0)]
    examples: usize,

    /// Techniques grouped by difficulty, easiest first: `/` between tranches, `,` inside them
    #[arg(long, default_value_t = default_tranches())]
    tranches: String,
}

/// The tranches that the GUI's line analysis grades lines by.
fn default_tranches() -> String {
    let tranches: Vec<String> = Technique::tranches()
        .iter()
        .map(|tranche| {
            let names: Vec<&str> = tranche.iter().map(|technique| technique.name()).collect();
            names.join(",")
        })
        .collect();
    tranches.join("/")
}

type LineSolver = Box<dyn Fn(&[Nono], &mut [Cell]) -> anyhow::Result<()>>;

enum Kind {
    /// Gets applied once.
    Single(LineSolver),
    /// These singles (indices into the list of entries) take turns until none can do any more.
    Fixpoint(Vec<usize>),
}

struct Entry {
    name: String,
    kind: Kind,
}

/// Techniques that are sitting the evaluation out, for now: between them, they're most of a
/// `skim`, which makes the easier techniques look like they don't matter.
const SHELVED: &[Technique] = &[Technique::BackgroundSkim, Technique::ForegroundSkim];

const OLD_SCRUB: &str = "old-scrub";
const EXHAUST: &str = "exhaust";
const HUMAN: &str = "human*";
const ALL: &str = "all*";

struct Roster {
    /// Singles come first, in (loose!) order from easiest to hardest.
    entries: Vec<Entry>,
    singles: usize,
    /// For each of the human techniques, the fixpoint of all the others.
    without: Vec<Without>,
    tranches: Vec<Tranche>,
    /// What the fixpoints are, for people.
    legend: Vec<String>,
}

struct Without {
    entry: usize,
    technique: usize,
}

/// Some techniques of about the same difficulty.
struct Tranche {
    /// The fixpoint of this tranche and all the easier ones.
    entry: usize,
    /// For each of its techniques, that fixpoint without it.
    without: Vec<Without>,
}

impl Roster {
    fn new(tranche_spec: &str) -> anyhow::Result<Roster> {
        let mut entries = vec![];

        let mut single = |name: &str, solver: LineSolver| {
            entries.push(Entry {
                name: name.to_string(),
                kind: Kind::Single(solver),
            })
        };

        let evaluated = || Technique::all().iter().filter(|t| !SHELVED.contains(t));
        let human: Vec<&str> = evaluated().map(|t| t.name()).collect();
        for technique in evaluated() {
            single(
                technique.name(),
                Box::new(|clues, lane| technique.apply(clues, lane).map(|_| ())),
            );
        }
        single(
            "settle",
            Box::new(|clues, lane| settle_line(clues, lane).map(|_| ())),
        );
        single(
            "skim",
            Box::new(|clues, lane| skim_line(clues, lane).map(|_| ())),
        );
        single(
            OLD_SCRUB,
            Box::new(|clues, lane| scrub_line(clues, lane).map(|_| ())),
        );
        single(
            EXHAUST,
            Box::new(|clues, lane| exhaust_line(clues, lane).map(|_| ())),
        );

        let singles = entries.len();
        let single_names: Vec<String> = entries.iter().map(|entry| entry.name.clone()).collect();
        let single = |name: &str| single_names.iter().position(|single| single == name);
        let mut fixpoint = |name: &str, members: &[&str]| {
            let members = members
                .iter()
                .map(|member| single(member).unwrap())
                .collect();
            entries.push(Entry {
                name: name.to_string(),
                kind: Kind::Fixpoint(members),
            });
            entries.len() - 1
        };

        fixpoint("skim*", &["skim"]);
        fixpoint(HUMAN, &human);
        let mut without = vec![];
        for (technique, name) in human.iter().enumerate() {
            let others: Vec<&str> = human.iter().filter(|n| *n != name).copied().collect();
            without.push(Without {
                entry: fixpoint(&format!("w/o {name}*"), &others),
                technique,
            });
        }
        let mut everything = human.clone();
        everything.extend(["settle", "skim"]);
        fixpoint(ALL, &everything);

        let mut tranches = vec![];
        let mut so_far: Vec<&str> = vec![];
        for spec in tranche_spec
            .split('/')
            .filter(|spec| !spec.trim().is_empty())
        {
            let names: Vec<&str> = spec.split(',').map(str::trim).collect();
            for name in &names {
                if single(name).is_none() {
                    bail!(
                        "--tranches: no technique called `{name}` (there's {})",
                        single_names.join(", ")
                    );
                }
                if so_far.contains(name) {
                    bail!("--tranches: `{name}` is there twice");
                }
                so_far.push(name);
            }

            let label = tranche_label(tranches.len());
            let entry = fixpoint(&label, &so_far);
            let without = names
                .iter()
                .map(|name| {
                    let others: Vec<&str> = so_far.iter().filter(|n| *n != name).copied().collect();
                    Without {
                        entry: fixpoint(&format!("{label} w/o {name}"), &others),
                        technique: single(name).unwrap(),
                    }
                })
                .collect();
            tranches.push(Tranche { entry, without });
        }

        let legend = vec![
            format!(
                "{HUMAN}: everything in `human_line_solver.rs`{}",
                if SHELVED.is_empty() {
                    "".to_string()
                } else {
                    let shelved: Vec<&str> = SHELVED.iter().map(|t| t.name()).collect();
                    format!(" (except {}, which are shelved)", shelved.join(" and "))
                }
            ),
            format!("w/o ...*: `{HUMAN}`, leaving one out"),
            format!("{ALL}: everything but `{OLD_SCRUB}` and `{EXHAUST}`"),
        ];

        Ok(Roster {
            entries,
            singles,
            without,
            tranches,
            legend,
        })
    }

    /// Whether an entry is only there to show what a tranche would be like without something.
    fn is_tranche_without(&self, idx: usize) -> bool {
        self.tranches
            .iter()
            .flat_map(|tranche| &tranche.without)
            .any(|without| without.entry == idx)
    }

    fn find(&self, name: &str) -> usize {
        self.entries
            .iter()
            .position(|entry| entry.name == name)
            .unwrap()
    }

    fn run(&self, idx: usize, clues: &[Nono], lane: &mut [Cell]) -> anyhow::Result<()> {
        match &self.entries[idx].kind {
            Kind::Single(solver) => solver(clues, lane),
            Kind::Fixpoint(members) => loop {
                let before = lane.to_vec();
                for member in members {
                    self.run(*member, clues, lane)?;
                    rule_out_finished_colors(clues, lane)?;
                }
                if lane == before.as_slice() {
                    return Ok(());
                }
            },
        }
    }
}

/// What to call a tranche taking turns with all of the easier ones.
fn tranche_label(idx: usize) -> String {
    if idx == 0 {
        "tranche 1*".to_string()
    } else {
        format!("tranches 1-{}*", idx + 1)
    }
}

// ---- Generating lines ----

struct TestLine {
    solution: Vec<Color>,
    clues: Vec<Nono>,
    lane: Vec<Cell>,
}

/// Runs of background and foreground, with a different taste in run lengths for each line.
fn random_solution(rng: &mut StdRng, len: usize, colors: u8) -> Vec<Color> {
    let stay_background = rng.random_range(0.3..0.9);
    let stay_foreground = rng.random_range(0.3..0.9);

    let random_foreground = |rng: &mut StdRng| Color(rng.random_range(1..=colors));

    let mut cur = if rng.random_bool(0.5) {
        BACKGROUND
    } else {
        random_foreground(rng)
    };
    let mut res = Vec::with_capacity(len);
    for _ in 0..len {
        res.push(cur);
        if cur == BACKGROUND {
            if !rng.random_bool(stay_background) {
                cur = random_foreground(rng);
            }
        } else if !rng.random_bool(stay_foreground) {
            // Sometimes, differently-colored runs touch.
            let prev = cur;
            cur = BACKGROUND;
            if colors > 1 && rng.random_bool(0.3) {
                while cur == BACKGROUND || cur == prev {
                    cur = random_foreground(rng);
                }
            }
        }
    }
    res
}

fn clues_of(solution: &[Color]) -> Vec<Nono> {
    let mut res: Vec<Nono> = vec![];
    let mut prev = BACKGROUND;
    for &color in solution {
        if color != BACKGROUND {
            match res.last_mut() {
                Some(clue) if prev == color => clue.count += 1,
                _ => res.push(Nono { color, count: 1 }),
            }
        }
        prev = color;
    }
    res
}

/// `None` if the line didn't work out (generally, because it got solved too early).
fn random_test_line(rng: &mut StdRng, args: &Args) -> Option<TestLine> {
    let len = rng.random_range(args.min_len..=args.max_len);
    let solution = random_solution(rng, len, args.colors);
    let clues = clues_of(&solution);

    let mut unknown = Cell::new_impossible();
    for color in 0..=args.colors {
        unknown.actually_could_be(Color(color));
    }

    // What the crossing lines had worked out, the last time anyone looked at this one...
    let reveal = rng.random_range(0.0..0.5);
    let mut lane: Vec<Cell> = solution
        .iter()
        .map(|color| {
            if rng.random_bool(reveal) {
                Cell::from_color(*color)
            } else {
                unknown
            }
        })
        .collect();
    // ...which was a thorough look...
    exhaust_line(&clues, &mut lane).expect("a line can't contradict its own clues");

    // ...and what they've worked out since.
    let mut unsolved: Vec<usize> = (0..len).filter(|i| !lane[*i].is_known()).collect();
    unsolved.shuffle(rng);
    let new_cells = rng.random_range(1..=args.max_new_cells);
    if unsolved.len() <= new_cells {
        return None; // Not a line anymore, just some leftovers.
    }
    for i in &unsolved[..new_cells] {
        lane[*i] = Cell::from_color(solution[*i]);
    }
    rule_out_finished_colors(&clues, &mut lane).expect("a line can't contradict its own clues");

    Some(TestLine {
        solution,
        clues,
        lane,
    })
}

// ---- Measuring ----

fn possibilities(lane: &[Cell]) -> u64 {
    lane.iter().map(|cell| cell.raw().count_ones() as u64).sum()
}

/// `a` knows everything `b` does (and maybe more).
fn covers(a: &[Cell], b: &[Cell]) -> bool {
    a.iter().zip(b).all(|(a, b)| a.is_within(*b))
}

/// Is `result` something that could legitimately have been deduced?
fn check(result: &[Cell], line: &TestLine, exhausted: &[Cell]) -> anyhow::Result<()> {
    for (i, cell) in result.iter().enumerate() {
        if !cell.can_be(line.solution[i]) {
            bail!("ruled out the truth at cell {i}");
        }
        if !exhausted[i].is_within(*cell) {
            bail!("knows more than `exhaust` at cell {i}");
        }
    }
    Ok(())
}

#[derive(Default, Clone)]
struct Stats {
    progressed: usize,
    /// Possibilities eliminated, over all lines.
    eliminated: u64,
    matched_exhaust: usize,
    /// Lines where it ruled out something that no other single did (the scrubs aside).
    unique: usize,
    /// `covered[i]`: lines where single `i` progressed, and this learned at least as much.
    covered: Vec<usize>,
    problems: usize,
    first_problem: Option<String>,
}

/// Lines where one entry learned something that another one didn't.
#[derive(Default)]
struct Shortfall {
    lines: usize,
    examples: Vec<String>,
}

impl Shortfall {
    /// `rows` are what to show for it: the line before anything happened to it, then the one
    /// that fell short, then the one that didn't.
    fn note(&mut self, args: &Args, line: &TestLine, rows: &[(&str, &[Cell])]) {
        if covers(rows[1].1, rows[2].1) {
            return;
        }
        self.lines += 1;
        if self.examples.len() < args.examples {
            self.examples.push(show_example(line, rows));
        }
    }

    fn show(&self, title: &str) {
        if self.examples.is_empty() {
            return;
        }
        println!();
        println!("{title}:");
        for example in &self.examples {
            print!("{example}");
        }
    }
}

fn show_clues(clues: &[Nono]) -> String {
    let colorful = clues.iter().any(|clue| clue.color != Color(1));
    clues
        .iter()
        .map(|clue| {
            if colorful {
                format!("{}{}", show_color(clue.color), clue.count)
            } else {
                clue.count.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn show_color(color: Color) -> char {
    match color.0 {
        0 => 'x',
        1 => '#',
        n => (b'a' + n - 2) as char,
    }
}

const LEGEND: &str = "`?` is unknown, `*` is some foreground color or other, and `~` is still \
                      unknown, but less so than in `before`";

/// See `LEGEND`. (Only `?` ever turns up in black-and-white lines.)
fn show_lane(lane: &[Cell], before: &[Cell]) -> String {
    lane.iter()
        .zip(before)
        .map(|(cell, before)| match cell.known_or() {
            Some(color) => show_color(color),
            None if cell != before => '~',
            None if !cell.can_be(BACKGROUND) => '*',
            None => '?',
        })
        .collect()
}

/// The first row is taken to be the line as it was before anything happened to it.
fn show_example(line: &TestLine, rows: &[(&str, &[Cell])]) -> String {
    let width = rows.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    let mut res = format!("    {}\n", show_clues(&line.clues));
    for (name, lane) in rows {
        res.push_str(&format!(
            "      {name:>width$}  {}\n",
            show_lane(lane, rows[0].1)
        ));
    }
    res
}

fn percent(count: usize, out_of: usize) -> String {
    if out_of == 0 {
        "-".to_string()
    } else {
        format!("{:.1}%", 100.0 * count as f64 / out_of as f64)
    }
}

/// Technique names are too long to head a column, but they'll go on two lines.
fn column_heading(name: &str) -> (&str, &str) {
    match name.rfind('-') {
        Some(dash) => name.split_at(dash + 1),
        None => ("", name),
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if args.min_len < 2 || args.min_len > args.max_len {
        bail!("need 2 <= --min-len <= --max-len");
    }
    if args.colors == 0 || args.colors > 26 {
        bail!("--colors must be between 1 and 26");
    }
    if args.max_new_cells == 0 {
        bail!("--max-new-cells must be at least 1");
    }

    let roster = Roster::new(&args.tranches)?;
    let entries = &roster.entries;
    let singles = roster.singles;
    let old_scrub = roster.find(OLD_SCRUB);
    let exhaust = roster.find(EXHAUST);
    let human = roster.find(HUMAN);
    let all = roster.find(ALL);

    let mut stats = vec![
        Stats {
            covered: vec![0; singles],
            ..Default::default()
        };
        entries.len()
    ];
    let mut needs: Vec<Shortfall> = roster.without.iter().map(|_| Default::default()).collect();
    let mut tranche_needs: Vec<Vec<Shortfall>> = roster
        .tranches
        .iter()
        .map(|tranche| tranche.without.iter().map(|_| Default::default()).collect())
        .collect();
    let mut human_shortfall = Shortfall::default();
    let mut all_shortfall = Shortfall::default();

    let mut rng = StdRng::seed_from_u64(args.seed);
    let mut generated = 0_usize;
    let mut evaluated = 0_usize;

    while evaluated < args.lines {
        generated += 1;
        if generated > 1000 * args.lines.max(1000) {
            bail!("almost none of the generated lines are any use; giving up");
        }
        let Some(line) = random_test_line(&mut rng, &args) else {
            continue;
        };

        let mut exhausted = line.lane.clone();
        exhaust_line(&line.clues, &mut exhausted).expect("a line can't contradict its own clues");
        if exhausted == line.lane {
            continue;
        }
        evaluated += 1;

        let before = possibilities(&line.lane);
        let results: Vec<Vec<Cell>> = (0..entries.len())
            .map(|idx| {
                let mut lane = line.lane.clone();
                let outcome = roster
                    .run(idx, &line.clues, &mut lane)
                    .and_then(|()| check(&lane, &line, &exhausted));
                if let Err(problem) = outcome {
                    stats[idx].problems += 1;
                    stats[idx].first_problem.get_or_insert_with(|| {
                        format!(
                            "{problem}\n{}",
                            show_example(&line, &[("before", &line.lane), ("after", &lane)])
                        )
                    });
                    // Don't let nonsense into the statistics.
                    return line.lane.clone();
                }
                lane
            })
            .collect();

        for (idx, result) in results.iter().enumerate() {
            let stat = &mut stats[idx];
            if *result != line.lane {
                stat.progressed += 1;
            }
            stat.eliminated += before - possibilities(result);
            if *result == exhausted {
                stat.matched_exhaust += 1;
            }
            for other in 0..singles {
                if results[other] != line.lane && covers(result, &results[other]) {
                    stat.covered[other] += 1;
                }
            }
        }

        for idx in 0..singles {
            // Everything the other singles know between them (but the scrubs know too much).
            let is_unique = (0..line.lane.len()).any(|cell| {
                let others = (0..singles)
                    .filter(|other| *other != idx && *other != exhaust && *other != old_scrub)
                    .fold(line.lane[cell].raw(), |known, other| {
                        known & results[other][cell].raw()
                    });
                others & !results[idx][cell].raw() != 0
            });
            if is_unique {
                stats[idx].unique += 1;
            }
        }

        for (need, without) in needs.iter_mut().zip(&roster.without) {
            need.note(
                &args,
                &line,
                &[
                    ("before", &line.lane),
                    ("without", &results[without.entry]),
                    ("with", &results[human]),
                    (EXHAUST, &exhausted),
                ],
            );
        }
        for (needs, tranche) in tranche_needs.iter_mut().zip(&roster.tranches) {
            for (need, without) in needs.iter_mut().zip(&tranche.without) {
                need.note(
                    &args,
                    &line,
                    &[
                        ("before", &line.lane),
                        ("without", &results[without.entry]),
                        ("with", &results[tranche.entry]),
                        (EXHAUST, &exhausted),
                    ],
                );
            }
        }
        human_shortfall.note(
            &args,
            &line,
            &[
                ("before", &line.lane),
                (HUMAN, &results[human]),
                (ALL, &results[all]),
                (EXHAUST, &exhausted),
            ],
        );
        all_shortfall.note(
            &args,
            &line,
            &[
                ("before", &line.lane),
                (ALL, &results[all]),
                (EXHAUST, &exhausted),
            ],
        );
    }

    // ---- Reporting ----

    println!(
        "{evaluated} lines of length {}..={}, with {} foreground color{}, at most {} new cell{} \
         (seed {}).",
        args.min_len,
        args.max_len,
        args.colors,
        if args.colors == 1 { "" } else { "s" },
        args.max_new_cells,
        if args.max_new_cells == 1 { "" } else { "s" },
        args.seed
    );
    println!(
        "{} of generated lines were ones that `{EXHAUST}` could make progress on.",
        percent(evaluated, generated)
    );

    let name_width = (0..entries.len())
        .map(|idx| {
            if roster.is_tranche_without(idx) {
                0
            } else {
                entries[idx].name.len()
            }
        })
        .chain(
            roster
                .tranches
                .iter()
                .flat_map(|tranche| &tranche.without)
                .map(|without| "  w/o ".len() + entries[without.technique].name.len()),
        )
        .max()
        .unwrap();
    let total = stats[exhaust].eliminated;
    let summary_row_called = |name: &str, idx: usize, extra: Option<usize>| {
        let stat = &stats[idx];
        print!(
            "  {name:<name_width$}  {:>8}  {:>8}  {:>8}",
            percent(stat.progressed, evaluated),
            format!("{:.1}%", 100.0 * stat.eliminated as f64 / total as f64),
            percent(stat.matched_exhaust, evaluated),
        );
        if let Some(extra) = extra {
            print!("  {:>8}", percent(extra, evaluated));
        }
        println!();
    };
    let summary_row =
        |idx: usize, extra: Option<usize>| summary_row_called(&entries[idx].name, idx, extra);

    println!();
    println!("Applied once:");
    println!("  progress: lines where it learns anything");
    println!("  share:    how much of everything `{EXHAUST}` learns (over all lines) it learns");
    println!("  all:      lines where it learns everything `{EXHAUST}` does");
    println!(
        "  unique:   lines where it learns something that none of the others here do \
         (not counting `{OLD_SCRUB}` and `{EXHAUST}`)"
    );
    println!();
    println!(
        "  {:<name_width$}  {:>8}  {:>8}  {:>8}  {:>8}",
        "", "progress", "share", "all", "unique"
    );
    for idx in 0..singles {
        summary_row(idx, Some(stats[idx].unique));
    }

    println!();
    println!("Taking turns until nothing more happens:");
    for line in &roster.legend {
        println!("  {line}");
    }
    println!();
    println!(
        "  {:<name_width$}  {:>8}  {:>8}  {:>8}",
        "", "progress", "share", "all"
    );
    for idx in singles..=all {
        if roster.without.iter().all(|without| without.entry != idx) {
            summary_row(idx, None);
        }
    }
    println!();
    println!(
        "  {} of lines have something that `{ALL}` finds and `{HUMAN}` doesn't.",
        percent(human_shortfall.lines, evaluated)
    );
    println!(
        "  {} of lines have something that only the scrubs find.",
        percent(all_shortfall.lines, evaluated)
    );

    println!();
    println!("What each of the human techniques adds to the rest of them:");
    println!("  needed: lines where `{HUMAN}` learns something that it doesn't without this one");
    println!();
    println!(
        "  {:<name_width$}  {:>8}  {:>8}  {:>8}  {:>8}",
        "", "progress", "share", "all", "needed"
    );
    for (need, without) in needs.iter().zip(&roster.without) {
        summary_row(without.entry, Some(need.lines));
    }

    if !roster.tranches.is_empty() {
        println!();
        println!("Tranches, each one taking turns with all of the easier ones:");
        println!(
            "  needed: lines where the tranches so far learn something that they don't without \
             this one"
        );
        println!();
        println!(
            "  {:<name_width$}  {:>8}  {:>8}  {:>8}  {:>8}",
            "", "progress", "share", "all", "needed"
        );
        for (needs, tranche) in tranche_needs.iter().zip(&roster.tranches) {
            summary_row(tranche.entry, None);
            for (need, without) in needs.iter().zip(&tranche.without) {
                summary_row_called(
                    &format!("  w/o {}", entries[without.technique].name),
                    without.entry,
                    Some(need.lines),
                );
            }
        }
    }

    let headings: Vec<(&str, &str)> = (0..singles)
        .map(|idx| column_heading(&entries[idx].name))
        .collect();
    let column_width = headings
        .iter()
        .map(|(top, bottom)| top.len().max(bottom.len()))
        .max()
        .unwrap()
        .max(6);
    println!();
    println!("Subsumption: of the lines where the column makes progress (applied once), how");
    println!("often does the row learn everything the column did?");
    println!();
    print!("  {:<name_width$}", "");
    for (top, _) in &headings {
        print!(" {top:>column_width$}");
    }
    println!();
    print!("  {:<name_width$}", "");
    for (_, bottom) in &headings {
        print!(" {bottom:>column_width$}");
    }
    println!();
    for (row, stat) in stats.iter().enumerate() {
        if roster.is_tranche_without(row) {
            continue;
        }
        if row == singles {
            println!();
        }
        print!("  {:<name_width$}", entries[row].name);
        for column in 0..singles {
            let cell = if row == column {
                "".to_string()
            } else {
                percent(stat.covered[column], stats[column].progressed)
            };
            print!(" {cell:>column_width$}");
        }
        println!();
    }

    // The matrix is a lot to take in, so pick out the punchlines.
    let mut near_misses: Vec<(f64, String)> = vec![];
    let mut near_miss = |row: usize, column: usize, row_name: &str| {
        let lines = stats[column].progressed;
        if lines == 0 {
            return;
        }
        let rate = stats[row].covered[column] as f64 / lines as f64;
        if rate >= 0.9 {
            let missed = lines - stats[row].covered[column];
            near_misses.push((
                rate,
                format!(
                    "  {row_name} {}subsume{} {} ({}; misses {missed} of {lines} lines)",
                    if missed == 0 { "" } else { "nearly " },
                    if row < singles { "s" } else { "" },
                    entries[column].name,
                    percent(stats[row].covered[column], lines),
                ),
            ));
        }
    };
    for row in 0..singles {
        for column in 0..singles {
            if row != column && row != exhaust && row != old_scrub {
                near_miss(row, column, &entries[row].name);
            }
        }
    }
    for without in &roster.without {
        near_miss(
            without.entry,
            without.technique,
            "the other human techniques",
        );
    }
    near_misses.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!();
    if near_misses.is_empty() {
        println!("Nothing comes close (90%) to subsuming anything else.");
    } else {
        println!("Closest to subsuming (90% and up):");
        for (_, description) in near_misses {
            println!("{description}");
        }
    }

    if args.examples > 0 && args.colors > 1 {
        println!();
        println!("In the examples, {LEGEND}.");
    }
    for (need, without) in needs.iter().zip(&roster.without) {
        need.show(&format!(
            "Lines where the other human techniques need `{}`",
            entries[without.technique].name
        ));
    }
    for (idx, (needs, tranche)) in tranche_needs.iter().zip(&roster.tranches).enumerate() {
        for (need, without) in needs.iter().zip(&tranche.without) {
            need.show(&format!(
                "Lines where `{}` needs `{}`",
                tranche_label(idx),
                entries[without.technique].name
            ));
        }
    }
    human_shortfall.show(&format!("Lines where `{ALL}` beats `{HUMAN}`"));
    all_shortfall.show(&format!("Lines where `{EXHAUST}` beats `{ALL}`"));

    let mut any_problems = false;
    for (entry, stat) in entries.iter().zip(&stats) {
        if let Some(problem) = &stat.first_problem {
            any_problems = true;
            println!();
            println!(
                "BUG: `{}` went wrong on {} lines! The first one: {problem}",
                entry.name, stat.problems
            );
        }
    }
    if any_problems {
        bail!("some techniques are broken, so don't trust the numbers");
    }

    Ok(())
}
