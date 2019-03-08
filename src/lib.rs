use std::collections::BTreeMap;

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Commit {
    pub jobs: BTreeMap<String, Job>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Job {
    pub url: String,
    pub path: String,
    pub timings: BTreeMap<String, Timing>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Timing {
    pub dur: f64,
    pub parts: BTreeMap<String, f64>,
}

