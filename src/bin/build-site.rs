use failure::Error;
use shared::{Commit, GitCommit};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

const USAGE: &'static str = "
This is some usage

Usage:
    build-site <rust-repo> <cache-dir> <out-dir>
    build-site -h | --help

Options:
    -h --help                    Show this screen.
";

#[derive(Debug, serde::Deserialize)]
struct Args {
    arg_rust_repo: PathBuf,
    arg_cache_dir: PathBuf,
    arg_out_dir: PathBuf,
}

fn main() {
    env_logger::init();

    let args: Args = docopt::Docopt::new(USAGE)
        .and_then(|d| d.deserialize())
        .unwrap_or_else(|e| e.exit());

    let err = match run(&args) {
        Ok(()) => return,
        Err(e) => e,
    };
    eprintln!("error: {}", err);
    for cause in err.iter_causes() {
        eprintln!("\tcaused by: {}", cause);
    }
    process::exit(1);
}

fn run(args: &Args) -> Result<(), Error> {
    let commits = get_commits(&args.arg_rust_repo, &args.arg_cache_dir)?;

    if !args.arg_out_dir.exists() {
        std::fs::create_dir_all(&args.arg_out_dir)?;
    }
    write_overall(&commits, &args.arg_out_dir)?;
    write_each_commit(&commits, &args.arg_out_dir)?;
    Ok(())
}

fn write_overall(commits: &[(GitCommit, Commit)], out_dir: &Path) -> Result<(), Error> {
    let mut jobs = BTreeMap::new();
    for (_sha, commit) in commits.iter() {
        for (name, data) in commit.jobs.iter() {
            let (count, total) = jobs.entry(name).or_insert((0, 0.0));
            *count += 1;
            for (_name, timing) in data.timings.iter() {
                *total += timing.dur;
            }
        }
    }

    let mut slowest_jobs = jobs.keys().cloned().collect::<Vec<_>>();
    slowest_jobs.sort_by_key(|name| {
        let (count, total) = jobs[name];
        (-total / (count as f64)) as i64
    });

    #[derive(serde::Serialize, Default)]
    struct Data<'a> {
        commits: Vec<Commit<'a>>,
        series: Vec<Series<'a>>,
    }
    #[derive(serde::Serialize)]
    struct Series<'a> {
        name: &'a str,
        data: Vec<f64>,
    }
    #[derive(serde::Serialize)]
    struct Commit<'a> {
        sha: &'a str,
        date: &'a str,
    }
    let mut data = Data::default();
    for job in slowest_jobs {
        let mut series = Series {
            name: job,
            data: Vec::new(),
        };
        for (_sha, commit) in commits.iter() {
            match commit.jobs.get(job) {
                Some(data) => {
                    series.data.push(
                        data.timings
                            .iter()
                            .filter(|(k, _)| *k != "Distcheck")
                            .map(|(_, v)| v.dur)
                            .sum(),
                    );
                }
                None => series.data.push(0.0),
            }
        }
        data.series.push(series);
    }
    for (git, _commit) in commits.iter() {
        data.commits.push(Commit {
            sha: &git.sha,
            date: &git.date,
        });
    }
    data.commits.reverse();
    for data in data.series.iter_mut() {
        data.data.reverse();
    }
    let json = serde_json::to_string(&data)?;
    fs::write(out_dir.join("overall.json"), json)?;
    Ok(())
}

fn write_each_commit(commits: &[(GitCommit, Commit)], out_dir: &Path) -> Result<(), Error> {
    for (git, commit) in commits {
        let dst = out_dir.join(&git.sha).with_extension("json");
        let json = serde_json::to_string(&commit)?;
        fs::write(&dst, json)?;
    }
    Ok(())
}

fn get_commits(rust: &Path, cache: &Path) -> Result<Vec<(GitCommit, Commit)>, Error> {
    let commits = shared::get_git_commits(rust)?
        .take(100)
        .collect::<Result<Vec<_>, Error>>()?;

    let mut urls = Vec::new();
    let commits_dir = cache.join("commits");
    let mut paths = Vec::new();
    for commit in commits.iter() {
        let path = commits_dir.join(&commit.sha).with_extension("json.gz");
        if !path.exists() {
            let url = format!(
                "https://s3-{}.amazonaws.com/{}/commits/{}.json.gz",
                env::var("S3_REGION").expect("missing environment variable S3_REGION"),
                env::var("S3_BUCKET").expect("missing environment variable S3_BUCKET"),
                commit.sha
            );
            urls.push(url);
        }
        paths.push(path);
    }

    if urls.len() > 0 {
        println!("downloading {:#?}", urls);
        fs::create_dir_all(&commits_dir)?;
        let status = Command::new("curl")
            .arg("--remote-name-all")
            .arg("-f")
            .args(&urls)
            .current_dir(commits_dir)
            .status()?;
        assert!(status.success());
    }

    let mut ret = Vec::new();
    for (commit, path) in commits.into_iter().zip(&paths) {
        log::debug!("reading {:?}", path);
        let raw = fs::read(path)?;
        let mut json = String::new();
        flate2::read::GzDecoder::new(&raw[..]).read_to_string(&mut json)?;
        let json: shared::Commit = serde_json::from_str(&json)?;
        ret.push((commit, json));
    }
    Ok(ret)
}
