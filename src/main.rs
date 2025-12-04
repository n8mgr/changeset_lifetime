use chrono::{DateTime, Duration, Utc};
use clap::Parser;
use std::collections::HashSet;
use std::path::Path;
use std::io::{self, Write};
use std::process::{Command, Stdio};

fn eprintln_exit(msg: &str, code: i32) -> ! {
    let _ = writeln!(io::stderr(), "{msg}");
    std::process::exit(code);
}

fn run_git(dir: &str, args: &[&str]) -> Vec<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .unwrap_or_else(|e| eprintln_exit(&format!("failed to run git: {e}"), 1));

    if !output.status.success() {
        eprintln_exit(
            &format!("git {:?} failed with status {}", args, output.status),
            1,
        );
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

struct ChangesetLifetime {
    name: String,
    commit_added: String,
    commit_removed: Option<String>,
    age: Duration,
}

/// Oldest add commit for path (first time file was added).
fn commit_created(dir: &str, branch: &str, path: &str) -> (String, DateTime<Utc>) {
    let lines = run_git(dir, &[
        "log",
        branch,
        "--diff-filter=A",
        "--follow",
        "--format=%H %aI",
        "--",
        path,
    ]);
    let mut parts = lines[0].split_whitespace();
    let hash = parts.next().expect("two parts").trim();
    let ts = parts.next().expect("two parts").trim();
    match DateTime::parse_from_rfc3339(ts) {
        Ok(dt) => (hash.to_string(), dt.with_timezone(&Utc)),
        Err(_) => panic!("failed to parse date from git log"),
    }
}

/// Newest delete commit for path (last time file was deleted).
fn commit_deleted(dir: &str, branch: &str, path: &str) -> Option<(String, DateTime<Utc>)> {
    let lines = run_git(dir, &[
        "log",
        branch,
        "--diff-filter=D",
        "--follow",
        "--format=%H %aI",
        "--",
        path,
    ]);
    if lines.is_empty() {
        return None;
    }
    // newest delete = first line
    let mut parts = lines[0].split_whitespace();
    let hash = parts.next().expect("two parts").trim();
    let ts = parts.next().expect("two parts").trim();
    match DateTime::parse_from_rfc3339(ts) {
        Ok(dt) => Some((hash.to_string(), dt.with_timezone(&Utc))),
        Err(_) => None,
    }
}

fn parse_duration(s: &str) -> Result<Duration, humantime::DurationError> {
    let dur = humantime::parse_duration(s)?;
    Ok(Duration::from_std(dur).unwrap())
}

#[derive(clap::Parser)]
struct Args {
    #[clap(short, long, default_value = ".")]
    dir: String,
    #[clap(long, default_value = "master")]
    branch: String,
    #[clap(long)]
    start: DateTime<Utc>,
    #[clap(long)]
    end: DateTime<Utc>,
    #[clap(long="days", default_value = "30days", value_parser = parse_duration)]
    min_days: Duration,
}

fn main() {
    let args = Args::parse();

    if args.end <= args.start {
        eprintln_exit("end must be after start", 1);
    }

    // All paths ever added/deleted under dir (includes deleted files)
    let files_raw = run_git(&args.dir, &[
        "log",
        &args.branch,
        "--diff-filter=AD",
        "--name-only",
        "--pretty=format:",
        "--",
        ".changeset",
    ]);

    let mut changesets = Vec::new();
    let files: HashSet<String> = files_raw.into_iter().collect();
    for fp in files {
        let (created_hash, created_dt) = commit_created(&args.dir, &args.branch, &fp);

        if created_dt > args.end {
            continue;
        }

        let meta = commit_deleted(&args.dir, &args.branch, &fp);
        if let Some((_, deleted_dt)) = meta {
            if deleted_dt < args.start {
                continue;
            }
        }

        let age: Duration = match meta {
            Some((_, deleted_dt)) => {
                deleted_dt - created_dt
            }
            None => {
                Utc::now() - created_dt
            }
        };
        // truncate to minutes. No need for nanosecond precision.
        let age = Duration::minutes(age.num_minutes());
        if age.is_zero() || age < args.min_days {
            continue;
        }

        
        let changeset = ChangesetLifetime{
            name: Path::new(&fp).file_name().unwrap().to_string_lossy().to_string(),
            commit_added: created_hash,
            commit_removed: meta.as_ref().map(|(h, _)| h.clone()),
            age,
        };
        changesets.push(changeset);
    }

    // Sort by age descending
    changesets.sort_by(|a, b| b.age.cmp(&a.age));

    let mut avg = Duration::zero();
    let n = changesets.len();
    for cs in changesets {
        avg = avg + cs.age;
        println!("{} {} - {}  ({})", cs.name, cs.commit_added, cs.commit_removed.unwrap_or("".into()), humantime::format_duration(cs.age.to_std().unwrap()));
    }
    let avg = Duration::minutes(avg.num_minutes() / n.max(1) as i64).to_std().unwrap();
    println!("Total: {} changesets ({})", n, humantime::format_duration(avg));
}
