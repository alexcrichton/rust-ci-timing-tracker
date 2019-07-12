use failure::{bail, format_err, Error};
use rayon::prelude::*;
use shared::{Commit, Job, Timing};
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};

struct Context {
    azure: HashMap<String, azure::Build>,
    cache: PathBuf,
}

struct Log {
    job_url: String,
    contents: String,
    path: String,
}

const USAGE: &'static str = "
This is some usage

Usage:
    publish-data-to-s3 [options] <rust-repo> <cache-dir>
    publish-data-to-s3 -h | --help

Options:
    -h --help                    Show this screen.
";

#[derive(Debug, serde::Deserialize)]
struct Args {
    arg_rust_repo: PathBuf,
    arg_cache_dir: PathBuf,
}

fn main() {
    env_logger::init();

    let args: Args = docopt::Docopt::new(USAGE)
        .and_then(|d| d.deserialize())
        .unwrap_or_else(|e| e.exit());

    let result = Context {
        azure: HashMap::new(),
        cache: args.arg_cache_dir.clone(),
    }
    .run(&args);
    let err = match result {
        Ok(()) => return,
        Err(e) => e,
    };
    eprintln!("error: {}", err);
    for cause in err.iter_causes() {
        eprintln!("\tcaused by: {}", cause);
    }
    process::exit(1);
}

impl Context {
    fn run(&mut self, args: &Args) -> Result<(), Error> {
        for commit in shared::get_git_commits(&args.arg_rust_repo)? {
            let commit = commit?;
            if self.exists_on_s3(&commit.sha) {
                break;
            }
            self.cache_commit(&commit.sha)?;
            if commit.sha == "3849a5f83b82258fd76a3ff64933b81d7efeffa1" {
                break;
            }
        }
        Ok(())
    }

    fn exists_on_s3(&self, commit: &str) -> bool {
        self.curl_s3()
            .head(true)
            .get(&format!("/commits/{}.json.gz", commit))
            .is_ok()
    }

    fn cache_commit(&mut self, commit: &str) -> Result<(), Error> {
        log::debug!("learning about {}", commit);
        let dir = self.cache.join("commits");
        let dst = dir.join(commit).with_extension("json.gz");
        if dst.exists() {
            return Ok(());
        }
        let logs = self.logs(commit)?;
        fs::create_dir_all(dst.parent().unwrap())?;

        let mut meta = Commit::default();

        for log in logs.iter() {
            let job = match self.identify_job(log) {
                Ok(s) => s,
                Err(_) => continue,
            };
            meta.jobs.insert(
                job,
                Job {
                    url: log.job_url.clone(),
                    path: log.path.clone(),
                    cpu_microarch: self.extract_cpu_microarch(&log.contents),
                    timings: self.extract_timings(&log.contents),
                },
            );
        }
        let json = serde_json::to_string(&meta)?;
        let mut raw = Vec::new();
        let mut gz = flate2::write::GzEncoder::new(&mut raw, flate2::Compression::best());
        gz.write_all(json.as_bytes())?;
        gz.finish()?;
        fs::write(&dst, raw)?;
        Ok(())
    }

    fn extract_timings(&self, contents: &str) -> BTreeMap<String, Timing> {
        let mut ret = BTreeMap::new();
        let mut parts = HashMap::new();
        for line in contents.lines() {
            let line = line.trim();
            if let Some(rest) = find_get_after(line, "[RUSTC-TIMING] ") {
                let mut iter = rest.rsplitn(2, ' ');
                let time = iter.next().unwrap().parse::<f64>().unwrap();
                let name = iter.next().unwrap();
                *parts.entry(name.to_string()).or_insert(0.0) += time;
            }

            if let Some(rest) = find_get_after(line, "[TIMING] ") {
                let pos = match rest.find(" -- ") {
                    Some(i) => i,
                    None => continue,
                };
                let step = &rest[..pos];
                let dur = rest[pos + 4..].parse::<f64>().unwrap();
                let timing = ret.entry(step.to_string()).or_insert_with(Timing::default);
                timing.dur += dur;
                for (k, v) in parts.drain() {
                    *timing.parts.entry(k).or_insert(0.0) += v;
                }
            }
        }
        return ret;
    }

    fn extract_cpu_microarch(&self, contents: &str) -> Option<String> {
        let mut family = None;
        for line in contents.lines() {
            let line = line.trim();
            match family {
                None => {
                    if let Some(family_content) = find_get_after(line, "cpu family\t: ") {
                        family = Some(family_content);
                    }
                }
                Some(family) => {
                    if let Some(model) = find_get_after(line, "model\t\t: ") {
                        return INTEL_CPU_MODEL_TO_MICROARCH
                            .iter()
                            .find(|(f, m, _)| *f == family && *m == model)
                            .map(|(_, _, arch)| arch.to_string());
                    }
                }
            }
        }
        None
    }

    fn identify_job(&self, log: &Log) -> Result<String, Error> {
        let needle = "[CI_JOB_NAME=";
        let line = log
            .contents
            .lines()
            .find(|l| l.contains(needle))
            .ok_or(format_err!("failed to find `{}`", needle))?;
        let pos = line.find(needle).unwrap();
        let contents = &line[pos + needle.len()..];
        let contents = contents.split(']').next().unwrap();

        // azure at one point buggily named everything `JobXX`
        if !contents.starts_with("Job") {
            return Ok(contents.to_string())
        }

        let needle = "AGENT_JOBNAME=";
        let line = log
            .contents
            .lines()
            .find(|l| l.contains(needle))
            .ok_or(format_err!("failed to find `{}`", needle))?;
        let pos = line.find(needle).unwrap();
        let contents = &line[pos + needle.len()..];
        Ok(contents.split_whitespace().skip(1).next().unwrap().to_string())
    }

    fn logs(&mut self, commit: &str) -> Result<Vec<Log>, Error> {
        while self.azure.get(commit).is_none() {
            self.load_more_azure()?;
        }

        let mut logs = Vec::new();
        self.azure_logs(commit, &mut logs)?;

        Ok(logs)
    }

    fn azure_logs(&mut self, commit: &str, logs: &mut Vec<Log>) -> Result<(), Error> {
        let build = &self.azure[commit];
        let response = self.curl_azure().get_json::<azure::Timeline>(&build._links.timeline.href)?;

        let jobs = response
            .records
            .par_iter()
            .filter(|record| {
                if record.r#type != "Job" {
                    return false;
                }

                // TODO: it looks like some logs are just missing from azure? See
                // https://dev.azure.com/rust-lang/rust/_build/results?buildId=3198
                // and dist-i686-apple for example...
                if record.log.is_none() {
                    return false;
                }

                true
            })
            .map(|record| self.get_azure_log(commit, record).map_err(|e| (e, record)))
            .collect::<Vec<_>>();
        for job in jobs {
            match job {
                Ok(s) => logs.push(s),
                // TODO: ignore errors when fetching logs. Apparently some logs
                // seem corrupted and/or azure just 500's whenever we try to
                // fetch them. We're somewhat opportunistic anyway so just
                // ignore it for now I guess?
                Err((e, record)) => {
                    println!("failed to fetch {}/{}", commit, record.id);
                    println!("error: {}", e);
                }
            }
        }
        Ok(())
    }

    fn get_azure_log(&self, commit: &str, record: &azure::TimelineRecord) -> Result<Log, Error> {
        let log = record.log.as_ref().unwrap();
        let path = format!("logs/azure/{}-{}.gz", commit, record.id);
        let dst = self.cache.join(&path);
        let contents = self.get_log(&dst, || {
            self.curl_azure().get(&log.url)
        })?;
        Ok(Log {
            job_url: log.url.clone(),
            contents,
            path,
        })
    }

    fn get_log(
        &self,
        cache: &Path,
        get: impl FnOnce() -> Result<String, Error>,
    ) -> Result<String, Error> {
        if cache.exists() {
            let raw = fs::read(cache)?;
            let mut contents = String::new();
            flate2::read::GzDecoder::new(&raw[..]).read_to_string(&mut contents)?;
            Ok(contents)
        } else {
            let log = get()?;
            fs::create_dir_all(cache.parent().unwrap())?;
            let mut raw = Vec::new();
            let mut gz = flate2::write::GzEncoder::new(&mut raw, flate2::Compression::best());
            gz.write_all(log.as_bytes())?;
            gz.finish()?;
            fs::create_dir_all(cache.parent().unwrap())?;
            fs::write(cache, raw)?;
            Ok(log)
        }
    }

    fn load_more_azure(&mut self) -> Result<(), Error> {
        if self.azure.len() > 0 {
            bail!("never did figure out the continuationToken thing");
        }
        let mut path = format!("/rust-lang/rust/_apis/build/builds");
        path.push_str("?api-version=5.0");
        path.push_str("&branchName=refs/heads/auto");
        path.push_str("&queryOrder=finishTimeDescending");
        let response = self.curl_azure().get_json::<azure::List>(&path)?;

        for build in response.value {
            self.azure.insert(build.source_version.clone(), build);
        }
        Ok(())
    }

    fn curl(&self, host: &str) -> Curl {
        let mut ret = Curl::new(host);
        ret.header("User-Agent", "rustc-ci-timing-tracker");
        return ret;
    }

    fn curl_azure(&self) -> Curl {
        self.curl("https://dev.azure.com")
    }

    fn curl_s3(&self) -> Curl {
        let bucket = env::var("S3_BUCKET").expect("missing environment variable S3_BUCKET");
        self.curl(&format!("https://{}.s3.amazonaws.com", bucket))
    }
}

struct Curl {
    cmd: Command,
    host: String,
}

impl Curl {
    fn new(host: &str) -> Curl {
        let mut cmd = Command::new("curl");
        cmd.arg("-sSf");
        Curl {
            cmd,
            host: host.to_string(),
        }
    }

    fn head(&mut self, head: bool) -> &mut Curl {
        if head {
            self.cmd.arg("-I");
        }
        self
    }

    fn header(&mut self, name: &str, value: &str) -> &mut Curl {
        self.cmd.arg("-H").arg(&format!("{}: {}", name, value));
        self
    }

    fn get_json<T: for<'a> serde::Deserialize<'a>>(&mut self, path: &str) -> Result<T, Error> {
        let json = self.get(path)?;
        let json = if log::log_enabled!(log::Level::Trace) {
            let pretty: serde_json::Value = serde_json::from_str(&json)?;
            let json = serde_json::to_string_pretty(&pretty)?;
            log::trace!("decode {}", json);
            json
        } else {
            json
        };
        Ok(serde_json::from_str(&json)?)
    }

    fn get(&mut self, path: &str) -> Result<String, Error> {
        let url = if path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{}{}", self.host, path)
        };
        log::debug!("GET: {}", url);
        let output = self.cmd.arg(&url).stderr(Stdio::inherit()).output()?;
        if output.status.success() {
            Ok(String::from_utf8(output.stdout)?)
        } else {
            bail!("failed to fetch `{}`: {}", url, output.status)
        }
    }
}

fn find_get_after<'a>(content: &'a str, needle: &str) -> Option<&'a str> {
    content
        .find(needle)
        .map(|pos| &content[pos + needle.len()..])
}

/// Map the CPU family and model to the microarchitecture name
/// Source for the data: https://en.wikichip.org/wiki/intel/cpuid
static INTEL_CPU_MODEL_TO_MICROARCH: &[(&str, &str, &str)] = &[
    ("6", "45", "sandybridge"),
    ("6", "62", "ivybridge"),
    ("6", "63", "haswell"),
    ("6", "79", "broadwell"),
    ("6", "85", "skylake"),
    ("6", "86", "broadwell"),
];

#[allow(dead_code)]
mod azure {
    #[derive(serde::Deserialize)]
    pub struct List {
        pub value: Vec<Build>,
    }

    #[derive(serde::Deserialize)]
    pub struct Build {
        #[serde(rename = "sourceVersion")]
        pub source_version: String,
        pub _links: BuildLinks,
    }

    #[derive(serde::Deserialize)]
    pub struct BuildLinks {
        pub timeline: Link,
    }

    #[derive(serde::Deserialize)]
    pub struct Link {
        pub href: String,
    }

    #[derive(serde::Deserialize)]
    pub struct Timeline {
        pub records: Vec<TimelineRecord>,
    }

    #[derive(serde::Deserialize)]
    pub struct TimelineRecord {
        pub id: String,
        pub r#type: String,
        pub log: Option<TimelineLog>,
    }

    #[derive(serde::Deserialize)]
    pub struct TimelineLog {
        pub url: String,
    }
}
