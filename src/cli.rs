use std::{collections::HashSet, fs::read_dir, path::PathBuf, process::exit, sync::OnceLock, time::Duration};
use clap::{command, Parser};
use base64::prelude::*;

use crate::metrics::{CgroupVersion, DockerCgroupDriver};

#[derive(Parser, Clone, Debug)]
#[command(version, about = "Simple Prometheus exporter for Docker container metrics. Use --help for more info.", long_about = "
This is a simple, lightweight Prometheus exporter for Docker container metrics.
No Docker socket access or special privilege is required; the program will read
from the cgroupfs (\x1b[34m/sys/fs/cgroup/\x1b[0m by default) to read metrics information, and
the Docker containers directory (\x1b[34m/var/lib/docker/containers/\x1b[0m by default) to add
container metadata to those metrics.

Many options below can be configured using environment variables. Some, such as
basicauth credentials, should preferably be configured as such.

Don't expect this tool to be perfect. Use `\x1b[36mcadvisor\x1b[0m` if you need something more
battle-tested and with (much) more metrics.")]
pub struct Cli {
    /// Path to the Docker "containers" directory
    #[arg(short = 'd', long, default_value = "/var/lib/docker/containers/", env)]
    pub containers_dir: PathBuf,

    /// Path to the cgroupfs
    #[arg(short = 'c', long, default_value = "/sys/fs/cgroup/", env)]
    pub cgroupfs_dir: PathBuf,

    /// IP and port to bind the HTTP server to
    /// 
    /// Defaults to localhost only. You must change this to be reachable over the network.
    /// Use [::]:3000 to listen on all IPv4 and IPv6 addresses.
    #[arg(short = 'l', long, default_value = "127.0.0.1:3000", env, verbatim_doc_comment)]
    pub listen_addr: core::net::SocketAddr,

    /// Minimum milliseconds allowed between container metadata refreshes
    /// 
    /// When this program is queried for metrics, it will read the metrics for all Docker containers by container ID.
    /// If it sees a container ID it doesn't recognize, it will re-read the Docker config files in the --containers-dir
    /// directory, unless it already tried doing that less than this many milliseconds ago.
    ///     Set to 0 to ALWAYS try refreshing container metadata.
    #[arg(long, default_value_t = 2000, env, verbatim_doc_comment)]
    pub min_metadata_refresh_ms: u32,
    #[arg(skip)]
    pub min_metadata_refresh: Option<Duration>,

    /// HTTP Basic authentication credentials
    /// 
    /// By default, anyone can query this server for metrics. When this option is set, the client must send an HTTP
    /// Basic authentication header with the provided credentials. Should be in the format of "username:password".
    /// Should not be base64 encoded.
    /// 
    /// We recommend setting this via environment variable:
    #[arg(short = 'B', long, env, verbatim_doc_comment)]
    pub basicauth: Option<String>,
    #[arg(skip)]
    pub basicauth_encoded: Option<String>,

    /// Override cgroup version detection
    /// 
    /// By default, this program will (crudely) analyze the cgroupfs file structure to try to guess whether cgroup
    /// API v1 or v2 is in use. Use this to override that guess.
    #[arg(long, env, verbatim_doc_comment)]
    pub cgroup_version: Option<CgroupVersion>,

    /// Override Docker cgroup driver detection
    /// 
    /// By default, this program will (crudely) analyze the cgroupfs file structure to try to guess which cgroup
    /// driver Docker is using. Use this to override that guess.
    #[arg(long, env, verbatim_doc_comment)]
    pub docker_cgroup_driver: Option<DockerCgroupDriver>,

    /// Docker labels to ignore when labeling metrics
    /// 
    /// By default, all container metrics will be labelled with all the labels of the container (prefixed with
    /// container_label_ and with dots replaced with underscores). This flag will exclude/ignore one or more
    /// container labels during this process. You may provide the flag multiple times, or separate labels with
    /// commas. You cannot provide both this and --include-labels.
    #[arg(long, env, verbatim_doc_comment)]
    pub exclude_labels: Vec<String>,
    #[arg(skip)]
    pub exclude_labels_set: HashSet<String>,

    /// Docker labels to include when labeling metrics
    /// 
    /// See --exclude-labels above. This works the same way but as a whitelist instead of a blacklist - only the
    /// specified container labels will be copied to metric labels.
    /// You cannot provide both this and --exclude-labels.
    #[arg(long, env, verbatim_doc_comment)]
    pub include_labels: Vec<String>,
    #[arg(skip)]
    pub include_labels_set: HashSet<String>,

    /// Increase the log level (default is INFO, one is DEBUG, two is TRACE).
    /// 
    /// You can also use environment variable RUST_LOG={OFF, ERROR, WARN, INFO, DEBUG, TRACE}.
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8
}

static CONFIG: OnceLock<Cli> = OnceLock::new();

#[inline]
pub fn cfg() -> &'static Cli {
    CONFIG.get().unwrap()
}

impl Cli {
    pub fn start() -> Cli {
        let mut out = Cli::parse();

        pretty_env_logger::formatted_builder()
            .filter_level(out.log_filter_level())
            .parse_default_env()
            .init();

        if !out.exclude_labels.is_empty() && !out.include_labels.is_empty() {
            eprintln!("\x1b[1;31mERROR: Cannot pass both --exclude-labels and --include-labels.\x1b[0m");
            exit(1);
        }

        out.exclude_labels_set = process_labels(&out.exclude_labels, "Excluding");
        out.include_labels_set = process_labels(&out.include_labels, "Including");

        check_read_dir(&out.containers_dir, "containers");
        check_read_dir(&out.cgroupfs_dir, "cgroupfs");

        if out.min_metadata_refresh_ms > 0 {
            out.min_metadata_refresh = Some(Duration::from_millis(out.min_metadata_refresh_ms.into()));
        }

        out.basicauth_encoded = out.basicauth.clone().map(|s| {
            info!("HTTP Basic auth will be required.");
            format!("Basic {}", BASE64_STANDARD.encode(s))
        });

        CONFIG.set(out.clone()).unwrap();
        out
    }

    pub fn log_filter_level(&self) -> log::LevelFilter {
        match self.verbose {
            0 => log::LevelFilter::Info,
            1 => log::LevelFilter::Debug,
            2 => log::LevelFilter::Trace,
            _ => { eprintln!("\x1b[1;31mError: Too many -v / --verbose flags supplied. Quitting.\x1b[0m"); exit(1); }
        }
    }
}

fn process_labels(args: &Vec<String>, debug_str: &str) -> HashSet<String> {
    let mut labels: HashSet<String> = HashSet::new();
    let mut lab_vec: Vec<String> = Vec::new();
    for arg in args {
        for label in arg.split(',') {
            let label = label.trim();
            if !label.is_empty() && labels.insert(label.to_owned()) {
                lab_vec.push(label.to_owned());
            }
        }
    }

    if !labels.is_empty() {
        info!("{debug_str} labels: {}", lab_vec.join(", "));
    }

    labels
}

fn check_read_dir(dir: &PathBuf, dir_name: &str) {
    if let Err(e) = read_dir(dir) {
        eprintln!("\x1b[1;31mFATAL ERROR: Unable to read contents of {dir_name} directory {:?}\x1b[0m", dir);
        eprintln!("\x1b[31mError details: {}\x1b[0m", e);
        eprintln!("If you're running this tool within a container, maybe check your volume mounts.");
        exit(e.raw_os_error().unwrap_or(1));
    }
}