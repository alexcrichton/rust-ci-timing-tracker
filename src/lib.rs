use failure::Error;
use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Commit {
    pub jobs: BTreeMap<String, Job>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Job {
    pub url: String,
    pub path: String,
    pub cpu_microarch: Option<String>,
    pub timings: BTreeMap<String, Timing>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Timing {
    pub dur: f64,
    pub parts: BTreeMap<String, f64>,
}

pub struct GitCommit {
    pub sha: String,
    pub date: String,
}

pub fn get_git_commits(
    repo: &Path,
) -> Result<impl Iterator<Item = Result<GitCommit, Error>>, Error> {
    let mut child = Command::new("git")
        .arg("log")
        .arg("--author=bors")
        .arg("--pretty=%H %aI")
        .current_dir(repo)
        .stdout(Stdio::piped())
        .spawn()?;
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());

    Ok(std::iter::repeat(()).filter_map(move |()| {
        let mut line = String::new();
        match stdout.read_line(&mut line) {
            Ok(0) => return None,
            Ok(_) => {}
            Err(e) => return Some(Err(e.into())),
        }
        let mut parts = line.split_whitespace();
        Some(Ok(GitCommit {
            sha: parts.next().unwrap().to_string(),
            date: parts.next().unwrap().to_string(),
        }))
    }))
}
