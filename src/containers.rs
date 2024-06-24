use std::{collections::HashMap, fs::{self, File}, io::BufReader, path::PathBuf, sync::Mutex, time::{Duration, Instant}};
use anyhow::Result;
use serde::Deserialize;
use lazy_static::lazy_static;

use crate::cli::cfg;

lazy_static! {
    pub static ref CONTAINERS_MAP: Mutex<HashMap<String,ContainerDetails>> = Mutex::new(HashMap::new());
    static ref LAST_CONTAINER_REFRESH: Mutex<Instant> = Mutex::new(Instant::now() - Duration::from_secs(1000));
}

#[derive(Debug, Deserialize, Clone)]
pub struct ContainerDetails {
    #[serde(rename = "ID")]
    pub id: String,
    
    #[serde(rename = "Name")]
    pub name: String,
    
    #[serde(rename = "Config")]
    pub config: ContainerConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ContainerConfig {
    #[serde(rename = "Image")]
    pub image: String,

    #[serde(rename = "Labels")]
    pub labels: HashMap<String, String>
}

#[inline]
fn container_details_from_config_path(container_config: PathBuf) -> Result<ContainerDetails> {
    let file = File::open(container_config)?;
    let reader = BufReader::new(file);
    Ok(serde_json::from_reader(reader)?)
}

pub fn refresh_containers_map(map: &mut HashMap<String, ContainerDetails>) {
    if let Some(min_interval) = crate::cli::cfg().min_metadata_refresh {
        let now = Instant::now();
        let mut last = LAST_CONTAINER_REFRESH.lock().unwrap();
        if (now - *last) < min_interval {
            return;
        }
        *last = now;
    }
    debug!("Refreshing container metadata.");

    if map.len() > 2000 {
        info!("Container metadata map has grown too large, clearing it out.");
        map.clear(); // crude anti-memory-leak mechanism i guess
    }

    let container_dirs = fs::read_dir(&cfg().containers_dir).expect("Couldn't read container directory.");
    let mut count = 0;
    for container_dir in container_dirs.filter_map(Result::ok) {
        let container_config = container_dir.path().join("config.v2.json");
        match container_details_from_config_path(container_config) {
            Ok(cont) => { count += 1; map.insert(cont.id.clone(), cont); }
            Err(e) => { error!("Container config.v2.json parse error: {e}"); continue; }
        };
    }
    info!("Refreshed container metadata, {count} containers present.")
}