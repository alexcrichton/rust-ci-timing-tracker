use failure::{bail, format_err, Error, ResultExt};
use std::collections::{HashMap, BTreeMap};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct Context {
    appveyor: HashMap<String, appveyor::Build>,
    travis: HashMap<String, travis::Build>,
    appveyor_start_id: Option<u64>,
    travis_offset: usize,
    cache: PathBuf,
}

struct Log {
    job_url: String,
    contents: String,
    path: String,
}

#[derive(serde::Serialize, Default)]
struct Commit {
    jobs: BTreeMap<String, Job>,
}

#[derive(serde::Serialize)]
struct Job {
    url: String,
    path: String,
    timings: BTreeMap<String, Timing>,
}

#[derive(serde::Serialize, Default)]
struct Timing {
    dur: f64,
    parts: BTreeMap<String, f64>,
}

fn main() {
    env_logger::init();
    let commit = std::env::args().nth(1).unwrap();
    Context {
        travis_offset: 0,
        appveyor_start_id: None,
        appveyor: HashMap::new(),
        travis: HashMap::new(),
        cache: PathBuf::from("cache"),
    }
    .cache_commit(&commit)
    .unwrap();
}

impl Context {
    fn cache_commit(&mut self, commit: &str) -> Result<(), Error> {
        let dir = self.cache.join("commits");
        let dst = dir.join(commit).with_extension("json");
        if dst.exists() {
            return Ok(());
        }
        let logs = self.logs(commit)?;
        fs::create_dir_all(dst.parent().unwrap())?;

        let mut meta = Commit::default();

        for log in logs.iter() {
            let job = self
                .identify_job(log)
                .context(format!("failed to identify {}", log.job_url))?;
            meta.jobs.insert(job, Job {
                url: log.job_url.clone(),
                path: log.path.clone(),
                timings: self.extract_timings(&log.contents),
            });
        }
        let json = serde_json::to_string(&meta)?;
        fs::write(&dst, json)?;
        Ok(())
    }

    fn extract_timings(&self, contents: &str) -> BTreeMap<String, Timing> {
        let mut ret = BTreeMap::new();
        let mut parts = HashMap::new();
        for line in contents.lines() {
            let line = line.trim();
            let needle1 = "[TIMING] ";
            let needle2 = "[RUSTC-TIMING] ";

            if let Some(pos) = line.find(needle2) {
                let rest = &line[pos + needle2.len()..];
                let mut iter = rest.rsplitn(2, ' ');
                let time = iter.next().unwrap().parse::<f64>().unwrap();
                let name = iter.next().unwrap();
                *parts.entry(name.to_string()).or_insert(0.0) += time;
            }

            if let Some(pos) = line.find(needle1) {
                let rest = &line[pos + needle1.len()..];
                let pos = rest.find(" -- ").unwrap();
                let step = &rest[..pos];
                let dur = rest[pos + 4..].parse::<f64>().unwrap();
                let timing = ret.entry(step.to_string()).or_insert_with(Timing::default);
                timing.dur += dur;
                for (k, v) in parts.drain() {
                    *timing.parts.entry(k).or_insert(0.0) += v;
                }
            }
        }
        return ret
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
        Ok(contents.to_string())
    }

    fn logs(&mut self, commit: &str) -> Result<Vec<Log>, Error> {
        while self.travis.get(commit).is_none() {
            self.load_more_travis()?;
        }
        while self.appveyor.get(commit).is_none() {
            self.load_more_appveyor()?;
        }

        let mut logs = Vec::new();
        self.appveyor_logs(commit, &mut logs)?;
        self.travis_logs(commit, &mut logs)?;

        Ok(logs)
    }

    fn travis_logs(&mut self, commit: &str, logs: &mut Vec<Log>) -> Result<(), Error> {
        let build = &self.travis[commit];
        let path = format!("/build/{}?include=build.jobs", build.id);
        let response = self.curl_travis().get_json::<travis::FullBuild>(&path)?;

        for job in response.jobs.iter() {
            logs.push(self.get_travis_log(&job.id.to_string())?);
        }
        Ok(())
    }

    fn get_travis_log(&self, job: &str) -> Result<Log, Error> {
        let path = format!("logs/travis/{}.gz", job);
        let dst = self.cache.join(&path);
        let contents = self.get_log(&dst, || {
            self.curl_travis().get(&format!("/v3/job/{}/log.txt", job))
        })?;
        let job_url = format!("https://travis-ci.com/rust-lang/rust/jobs/{}", job);
        Ok(Log {
            job_url,
            contents,
            path,
        })
    }

    fn appveyor_logs(&mut self, commit: &str, logs: &mut Vec<Log>) -> Result<(), Error> {
        let build = &self.appveyor[commit];
        let path = format!("/api/projects/rust-lang/rust/build/{}", build.version);
        let response = self
            .curl_appveyor()
            .get_json::<appveyor::GetFullBuild>(&path)?;

        for job in response.build.jobs.iter() {
            logs.push(self.get_appveyor_log(build.id, &job.id)?);
        }
        Ok(())
    }

    fn get_appveyor_log(&self, build_id: u64, job: &str) -> Result<Log, Error> {
        let path = format!("logs/appveyor/{}-{}.gz", build_id, job);
        let dst = self.cache.join(&path);
        let contents = self.get_log(&dst, || {
            self.curl_appveyor()
                .get(&format!("/api/buildjobs/{}/log", job))
        })?;
        let job_url = format!(
            "https://ci.appveyor.com/project/rust-lang/rust/builds/{}/job/{}",
            build_id, job
        );
        Ok(Log {
            job_url,
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

    fn load_more_travis(&mut self) -> Result<(), Error> {
        let mut path = format!("/repo/rust-lang%2Frust/builds");
        path.push_str("?branch.name=auto");
        path.push_str("&sort_by=started_at:desc");
        path.push_str("&limit=25");
        path.push_str(&format!("&offset={}", self.travis_offset));
        let response = self.curl_travis().get_json::<travis::Builds>(&path)?;

        self.travis_offset += response.builds.len();
        for build in response.builds {
            assert!(self
                .travis
                .insert(build.commit.sha.clone(), build)
                .is_none());
        }
        Ok(())
    }

    fn load_more_appveyor(&mut self) -> Result<(), Error> {
        let mut path = format!("/api/projects/rust-lang/rust/history");
        path.push_str("?branch=auto");
        path.push_str("&recordsNumber=100");
        if let Some(id) = self.appveyor_start_id.take() {
            path.push_str(&format!("&startBuildId={}", id));
        }
        let response = self.curl_appveyor().get_json::<appveyor::Builds>(&path)?;

        self.appveyor_start_id = Some(response.builds.last().unwrap().id);
        for build in response.builds {
            assert!(self
                .appveyor
                .insert(build.commit_id.clone(), build)
                .is_none());
        }
        Ok(())
    }

    fn curl(&self, host: &str) -> Curl {
        let mut ret = Curl::new(host);
        ret.header("User-Agent", "rustc-ci-timing-tracker");
        return ret;
    }

    fn curl_travis(&self) -> Curl {
        let mut ret = self.curl("https://api.travis-ci.com");
        ret.header("Travis-API-Version", "3");
        return ret;
    }

    fn curl_appveyor(&self) -> Curl {
        self.curl("https://ci.appveyor.com")
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
        let url = format!("{}{}", self.host, path);
        log::debug!("GET: {}", url);
        let output = self.cmd.arg(&url).stderr(Stdio::inherit()).output()?;
        if output.status.success() {
            Ok(String::from_utf8(output.stdout)?)
        } else {
            bail!("failed to fetch `{}`: {}", url, output.status)
        }
    }
}

#[allow(dead_code)]
mod travis {
    #[derive(serde::Deserialize)]
    pub struct Builds {
        pub builds: Vec<Build>,
    }

    #[derive(serde::Deserialize)]
    pub struct Build {
        pub id: u64,
        pub number: String,
        pub started_at: Option<String>,
        pub finished_at: Option<String>,
        pub commit: Commit,
    }

    #[derive(serde::Deserialize)]
    pub struct FullBuild {
        pub jobs: Vec<Job>,
    }

    #[derive(serde::Deserialize)]
    pub struct Job {
        pub id: u64,
        pub number: String,
    }

    #[derive(serde::Deserialize)]
    pub struct Commit {
        pub id: u64,
        pub sha: String,
    }
}

#[allow(dead_code)]
mod appveyor {
    #[derive(serde::Deserialize)]
    pub struct Builds {
        pub builds: Vec<Build>,
    }

    #[derive(serde::Deserialize)]
    pub struct Build {
        #[serde(rename = "buildId")]
        pub id: u64,
        #[serde(rename = "buildNumber")]
        pub build_number: u64,
        pub version: String,
        #[serde(rename = "commitId")]
        pub commit_id: String,
    }

    #[derive(serde::Deserialize)]
    pub struct GetFullBuild {
        pub build: FullBuild,
    }

    #[derive(serde::Deserialize)]
    pub struct FullBuild {
        pub jobs: Vec<Job>,
    }

    #[derive(serde::Deserialize)]
    pub struct Job {
        #[serde(rename = "jobId")]
        pub id: String,
    }
}
