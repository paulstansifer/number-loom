//! A Reddit bot (only a skeleton, so far) that reads the clues out of puzzles posted to
//! r/nonograms. It watches for new posts, downloads the first picture in each, and runs
//! `ocr-clues` on it. It doesn't write anything to Reddit yet: what it would reply goes into the
//! post's directory instead, beside everything else it found.
//!
//! ```text
//! cargo build --release --features ocr --bins
//! REDDIT_CLIENT_ID=... REDDIT_CLIENT_SECRET=... target/release/reddit-bot bot/
//! ```
//!
//! Reddit doesn't answer API requests without OAuth, so it needs the id and secret of an app
//! registered at <https://www.reddit.com/prefs/apps>. Since it only reads, it logs in as the app
//! itself, not as any user. (`--listing` reads a saved listing instead, for trying it out
//! without credentials.)

use std::collections::HashSet;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use clap::Parser;
use number_loom::import;
use number_loom::puzzle::{DynPuzzle, PuzzleDynOps as _};
use reqwest::blocking::Client;
use serde_json::Value;

#[derive(clap::Parser, Debug)]
#[command(about = "Watch r/nonograms, and read the clues out of new posts' pictures")]
struct Args {
    /// Where to keep everything: which posts have been seen, and for each post, its picture, what
    /// was read from it, and what the bot would reply
    dir: PathBuf,

    /// Check for new posts once, and stop
    #[arg(long)]
    once: bool,

    /// How long to wait between checks, in seconds
    #[arg(long, default_value_t = 120)]
    interval: u64,

    #[arg(long, default_value = "nonograms")]
    subreddit: String,

    /// The `ocr-clues` binary (by default, the one beside this one)
    #[arg(long)]
    ocr_clues: Option<PathBuf>,

    /// Process every post in this listing (as saved from `/r/nonograms/new.json`), whether it's
    /// been seen or not, instead of asking Reddit
    #[arg(long)]
    listing: Option<PathBuf>,
}

const USER_AGENT: &str = concat!(
    "rust:number-loom-ocr-bot:v",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/paulstansifer/number-loom)"
);

/// Logged in as the app itself (so it can read, but not post).
struct Reddit {
    http: Client,
    client_id: String,
    secret: String,
    /// And when to get a new one.
    token: Option<(String, Instant)>,
}

impl Reddit {
    fn from_env(http: Client) -> anyhow::Result<Reddit> {
        let var = |name: &str| {
            std::env::var(name).with_context(|| {
                format!("${name} isn't set (see https://www.reddit.com/prefs/apps)")
            })
        };
        Ok(Reddit {
            http,
            client_id: var("REDDIT_CLIENT_ID")?,
            secret: var("REDDIT_CLIENT_SECRET")?,
            token: None,
        })
    }

    fn token(&mut self) -> anyhow::Result<String> {
        if let Some((token, expires)) = &self.token
            && Instant::now() < *expires
        {
            return Ok(token.clone());
        }
        let response: Value = serde_json::from_str(
            &self
                .http
                .post("https://www.reddit.com/api/v1/access_token")
                .basic_auth(&self.client_id, Some(&self.secret))
                .form(&[("grant_type", "client_credentials")])
                .send()?
                .error_for_status()
                .context("logging in to Reddit")?
                .text()?,
        )?;
        let token = response["access_token"]
            .as_str()
            .with_context(|| format!("no token from Reddit: {response}"))?
            .to_string();
        let lifetime = response["expires_in"].as_u64().unwrap_or(3600);
        // (Renewed a little early, so it doesn't run out mid-request.)
        let expires = Instant::now() + Duration::from_secs(lifetime.saturating_sub(60));
        self.token = Some((token.clone(), expires));
        Ok(token)
    }

    /// The newest posts in the subreddit, as Reddit's listing JSON.
    fn new_posts(&mut self, subreddit: &str) -> anyhow::Result<Value> {
        let token = self.token()?;
        // (`raw_json` stops it from HTML-escaping the URLs in it.)
        let url = format!("https://oauth.reddit.com/r/{subreddit}/new?limit=25&raw_json=1");
        let text = self
            .http
            .get(&url)
            .bearer_auth(token)
            .send()?
            .error_for_status()
            .with_context(|| format!("fetching {url}"))?
            .text()?;
        Ok(serde_json::from_str(&text)?)
    }
}

/// The posts in a listing, newest first (as Reddit lists them).
fn posts(listing: &Value) -> Vec<&Value> {
    listing["data"]["children"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|child| child["kind"] == "t3")
        .map(|child| &child["data"])
        .collect()
}

const PICTURES: &[&str] = &["png", "jpg", "jpeg", "webp"];

/// The file extension of the picture at `url`, if it looks like a picture.
fn picture_extension(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let ext = path.rsplit_once('.')?.1.to_lowercase();
    PICTURES.contains(&ext.as_str()).then_some(ext)
}

/// Where to download the first picture in a post, if there is one.
fn first_picture(post: &Value) -> Option<String> {
    // A crosspost's pictures are in the post it's a copy of.
    if let Some(original) = post["crosspost_parent_list"].get(0) {
        return first_picture(original);
    }
    let metadata = &post["media_metadata"];
    let media = |id: &str| -> Option<String> {
        let m = &metadata[id];
        if m["status"] != "valid" || m["e"] != "Image" {
            return None;
        }
        // The original, not the resized preview that `s.u` points to.
        let ext = m["m"].as_str()?.strip_prefix("image/")?;
        Some(format!("https://i.redd.it/{id}.{ext}"))
    };
    if post["is_gallery"] == true {
        return post["gallery_data"]["items"]
            .as_array()?
            .iter()
            .find_map(|item| media(item["media_id"].as_str()?));
    }
    // A link straight to a picture (which is how Reddit stores a post of a single picture).
    let url = post["url_overridden_by_dest"]
        .as_str()
        .or(post["url"].as_str())
        .unwrap_or("");
    if picture_extension(url).is_some() {
        return Some(url.to_string());
    }
    // Pictures in the text of a post. `media_metadata` lists them in no particular order, so
    // it's the text that says which is first.
    let text = post["selftext"].as_str().unwrap_or("");
    let mut inline: Vec<(usize, &String)> = metadata
        .as_object()?
        .keys()
        .filter_map(|id| Some((text.find(id.as_str())?, id)))
        .collect();
    inline.sort();
    inline.into_iter().find_map(|(_, id)| media(id))
}

/// What came of reading a post's picture.
enum Outcome {
    /// `ocr-clues` couldn't: the last thing it said, which says why.
    Unreadable(String),
    /// Too few lanes with clues for a puzzle; likely not a picture of one at all.
    TooSmall { rows: usize, cols: usize },
    /// The clues read contradict each other, so some must be misread.
    Contradictory { width: usize, height: usize },
    /// `message` is the advice `ocr-clues` wrote for the person solving it, if it had any.
    Read {
        width: usize,
        height: usize,
        cells_left: usize,
        message: Option<String>,
    },
}

/// Fewer lanes than this with clues in them, in either direction, isn't worth replying to.
const MIN_LANES: usize = 4;

impl Outcome {
    /// One line, saying how well it went.
    fn status(&self) -> String {
        match self {
            Outcome::Unreadable(why) => format!("unreadable: {why}"),
            Outcome::TooSmall { rows, cols } => {
                format!("too small for a puzzle: {rows} rows and {cols} columns with clues")
            }
            Outcome::Contradictory { width, height } => {
                format!("read {width}x{height}, but the clues contradict each other")
            }
            Outcome::Read {
                width,
                height,
                cells_left: 0,
                ..
            } => format!("read {width}x{height}; solvable with line logic"),
            Outcome::Read {
                width,
                height,
                cells_left,
                ..
            } => format!("read {width}x{height}; line logic leaves {cells_left} cells"),
        }
    }
}

/// Download the picture at `url`, and read it, keeping everything in `dir`. Returns where the
/// picture went.
fn read_picture(
    http: &Client,
    ocr_clues: &Path,
    url: &str,
    dir: &Path,
) -> anyhow::Result<(PathBuf, Outcome)> {
    let ext = picture_extension(url).unwrap_or_else(|| "png".into());
    let picture = dir.join(format!("picture.{ext}"));
    let bytes = http
        .get(url)
        .send()?
        .error_for_status()
        .with_context(|| format!("downloading {url}"))?
        .bytes()?;
    fs::write(&picture, bytes)?;

    let puzzle_path = dir.join("puzzle.xml");
    let message_path = dir.join("message.md");
    // (So a failed rerun can't leave an old one behind.)
    let _ = fs::remove_file(&message_path);
    let output = Command::new(ocr_clues)
        .arg(&picture)
        .arg(&puzzle_path)
        .arg("--debug-image")
        .arg(dir.join("debug.png"))
        .arg("--message")
        .arg(&message_path)
        .output()
        .with_context(|| format!("running {ocr_clues:?}"))?;
    let report = String::from_utf8_lossy(&output.stderr).into_owned();
    fs::write(dir.join("ocr.txt"), &report)?;
    if !output.status.success() {
        let why = report.lines().last().unwrap_or("").to_string();
        return Ok((picture, Outcome::Unreadable(why)));
    }
    let message = fs::read_to_string(&message_path).ok();
    Ok((picture, check(&puzzle_path, message)?))
}

/// Whether the clues `ocr-clues` wrote to `puzzle_path` make sense.
fn check(puzzle_path: &Path, message: Option<String>) -> anyhow::Result<Outcome> {
    let document = import::load_path(&puzzle_path.to_path_buf(), None)?;
    let puzzle = document
        .try_puzzle()
        .and_then(DynPuzzle::as_square_nono)
        .context("ocr-clues wrote something other than a black-and-white puzzle")?;
    let lane_map = &puzzle.geometry.lane_map;
    // (Rows, then columns.)
    let [rows, cols] = [0, 1].map(|family| {
        lane_map
            .family(family.into())
            .filter(|&lane| !puzzle.lines[lane].is_empty())
            .count()
    });
    if rows < MIN_LANES || cols < MIN_LANES {
        return Ok(Outcome::TooSmall { rows, cols });
    }
    let [height, width] = [0, 1].map(|family| lane_map.family(family.into()).count());
    Ok(match puzzle.line_solve() {
        Err(_) => Outcome::Contradictory { width, height },
        Ok(solved) => Outcome::Read {
            width,
            height,
            cells_left: solved.cells_left,
            message,
        },
    })
}

/// What to say in reply to the post, if anything.
fn draft_reply(outcome: &Outcome) -> Option<String> {
    let Outcome::Read {
        message: Some(message),
        ..
    } = outcome
    else {
        return None; // Can't read it, or nothing to say about it; don't reply.
    };
    Some(message.clone())
}

/// Where replying to the post will go. For now, the reply is only saved.
fn reply(post: &Value, dir: &Path, text: &str) -> anyhow::Result<()> {
    fs::write(dir.join("reply.md"), text)?;
    eprintln!(
        "    would reply to https://www.reddit.com{} (see {})",
        post["permalink"].as_str().unwrap_or(""),
        dir.join("reply.md").display()
    );
    Ok(())
}

/// Read the post's first picture (if it has one), keeping everything in a directory of its own
/// in `root`, and noting how it went in `root/log.txt`.
fn handle(http: &Client, ocr_clues: &Path, post: &Value, root: &Path) -> anyhow::Result<()> {
    let Some(url) = first_picture(post) else {
        return Ok(());
    };
    let id = post["id"].as_str().context("a post without an id")?;
    eprintln!("{id}: {}", post["title"].as_str().unwrap_or(""));
    let dir = root.join(id);
    fs::create_dir_all(&dir).with_context(|| format!("creating {dir:?}"))?;
    fs::write(dir.join("post.json"), serde_json::to_string_pretty(post)?)?;

    let (picture, outcome) = read_picture(http, ocr_clues, &url, &dir)?;
    let status = outcome.status();
    eprintln!("    {status}");
    let picture = fs::canonicalize(&picture).unwrap_or(picture);
    let log = root.join("log.txt");
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .with_context(|| format!("writing {log:?}"))?;
    writeln!(log, "{}\t{status}", picture.display())?;

    if let Some(text) = draft_reply(&outcome) {
        reply(post, &dir, &text)?;
    } else {
        // (So an earlier run's draft doesn't look like this one's.)
        let _ = fs::remove_file(dir.join("reply.md"));
    }
    Ok(())
}

/// The ids of the posts already handled, kept one per line in a file.
struct Seen {
    path: PathBuf,
    ids: HashSet<String>,
}

impl Seen {
    fn load(path: PathBuf) -> anyhow::Result<Seen> {
        let ids = match fs::read_to_string(&path) {
            Ok(text) => text.lines().map(str::to_string).collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashSet::new(),
            Err(e) => return Err(e).with_context(|| format!("reading {path:?}")),
        };
        Ok(Seen { path, ids })
    }

    fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    fn add(&mut self, id: &str) -> anyhow::Result<()> {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("writing {:?}", self.path))?;
        writeln!(file, "{id}")?;
        self.ids.insert(id.to_string());
        Ok(())
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let ocr_clues = match &args.ocr_clues {
        Some(path) => path.clone(),
        None => std::env::current_exe()?.with_file_name("ocr-clues"),
    };
    if !ocr_clues.exists() {
        bail!(
            "no {ocr_clues:?}; build it with `cargo build --release --features ocr --bins`, \
             or say where it is with --ocr-clues"
        );
    }
    fs::create_dir_all(&args.dir).with_context(|| format!("creating {:?}", args.dir))?;
    let http = Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(60))
        .build()?;

    if let Some(path) = &args.listing {
        let text = fs::read_to_string(path).with_context(|| format!("reading {path:?}"))?;
        let listing: Value = serde_json::from_str(&text)?;
        for post in posts(&listing) {
            handle(&http, &ocr_clues, post, &args.dir)?;
        }
        return Ok(());
    }

    let mut reddit = Reddit::from_env(http.clone())?;
    let mut seen = Seen::load(args.dir.join("seen.txt"))?;
    loop {
        match reddit.new_posts(&args.subreddit) {
            // Oldest first, as they were posted.
            Ok(listing) => {
                for post in posts(&listing).into_iter().rev() {
                    let Some(id) = post["id"].as_str() else {
                        continue;
                    };
                    if seen.contains(id) {
                        continue;
                    }
                    // A post that goes wrong is reported and skipped, not retried: it would
                    // likely only go wrong again.
                    if let Err(e) = handle(&http, &ocr_clues, post, &args.dir) {
                        eprintln!("    {e:#}");
                    }
                    seen.add(id)?;
                }
            }
            Err(e) => eprintln!("Couldn't get the new posts: {e:#}"),
        }
        if args.once {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(args.interval));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn image(id: &str, mime: &str) -> Value {
        json!({ "status": "valid", "e": "Image", "m": mime, "s": { "u": format!("https://preview.redd.it/{id}") } })
    }

    #[test]
    fn single_picture() {
        let post = json!({ "url": "https://i.redd.it/abc123.png", "selftext": "" });
        assert_eq!(
            first_picture(&post).as_deref(),
            Some("https://i.redd.it/abc123.png")
        );
    }

    #[test]
    fn gallery() {
        let post = json!({
            "is_gallery": true,
            "url": "https://www.reddit.com/gallery/xyz",
            "gallery_data": { "items": [{ "media_id": "first" }, { "media_id": "second" }] },
            "media_metadata": { "second": image("second", "image/png"), "first": image("first", "image/jpg") },
        });
        assert_eq!(
            first_picture(&post).as_deref(),
            Some("https://i.redd.it/first.jpg")
        );
    }

    #[test]
    fn pictures_in_text() {
        let post = json!({
            "url": "https://www.reddit.com/r/nonograms/comments/xyz/help/",
            "selftext": "Stuck here:\n\nhttps://preview.redd.it/early.png?width=640\n\nand here:\n\nhttps://preview.redd.it/late.png?width=640",
            "media_metadata": { "late": image("late", "image/png"), "early": image("early", "image/png") },
        });
        assert_eq!(
            first_picture(&post).as_deref(),
            Some("https://i.redd.it/early.png")
        );
    }

    #[test]
    fn crosspost() {
        let post = json!({
            "url": "/r/nonograms/comments/abc/original/",
            "crosspost_parent_list": [{ "url": "https://i.redd.it/original.webp" }],
        });
        assert_eq!(
            first_picture(&post).as_deref(),
            Some("https://i.redd.it/original.webp")
        );
    }

    #[test]
    fn no_picture() {
        for post in [
            json!({ "url": "https://www.reddit.com/r/nonograms/comments/xyz/question/", "selftext": "How do you say it?" }),
            json!({ "url": "https://v.redd.it/video", "media_metadata": { "gif": { "status": "valid", "e": "AnimatedImage" } } }),
            json!({ "url": "https://www.youtube.com/watch?v=abc.png" }),
        ] {
            assert_eq!(first_picture(&post), None, "{post}");
        }
    }
}
