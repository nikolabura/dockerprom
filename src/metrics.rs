use std::{fs, path::PathBuf};
use anyhow::{Error, Result};
use clap::ValueEnum;
use prometheus_exporter_base::{MetricType, PrometheusInstance, PrometheusMetric};
use lazy_static::lazy_static;
use crate::cli::cfg;

use crate::refresh_containers_map;

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq)]
pub enum CgroupVersion { V1, V2 }

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq)]
pub enum DockerCgroupDriver { Cgroupfs, Systemd }

lazy_static! {
    static ref CGROUP_VER: CgroupVersion = figure_out_cgroup_ver();
    static ref DOCKER_CG_DRIVER: DockerCgroupDriver = figure_out_docker_driver();

    static ref MEMORY_DIR: PathBuf = generate_cgroup_dir("memory");
    static ref CPU_DIR: PathBuf = generate_cgroup_dir("cpu");
    static ref BLKIO_DIR: PathBuf = generate_cgroup_dir("blkio");

    static ref EXPECTED_DIR_NAME_LEN: usize = match *DOCKER_CG_DRIVER {
        DockerCgroupDriver::Cgroupfs => 64,
        DockerCgroupDriver::Systemd => 64 + 13 // docker-{64 chars}.scope
    };
}

fn generate_cgroup_dir(resource: &str) -> PathBuf {
    let mut out = cfg().cgroupfs_dir.clone();
    match (*CGROUP_VER, *DOCKER_CG_DRIVER) {
        (CgroupVersion::V1, DockerCgroupDriver::Cgroupfs) => out.push(format!("{resource}/docker")),
        (CgroupVersion::V1, DockerCgroupDriver::Systemd) => out.push(format!("{resource}/system.slice")),
        (CgroupVersion::V2, DockerCgroupDriver::Cgroupfs) => out.push("docker"),
        (CgroupVersion::V2, DockerCgroupDriver::Systemd) => out.push("system.slice")
    }
    out
}

fn figure_out_docker_driver() -> DockerCgroupDriver {
    let cli = cfg();
    let cgver = *CGROUP_VER;
    let mut dir = cli.cgroupfs_dir.clone();
    if cgver == CgroupVersion::V1 { dir.push("memory"); }
    let mut ls = std::fs::read_dir(&dir).unwrap_or_else(|_| panic!("Failed to read {:?} directory.", &dir));
    let guess = if ls.any(|entry| entry.unwrap().file_name() == "docker") {
        DockerCgroupDriver::Cgroupfs
    } else {
        DockerCgroupDriver::Systemd
    };
    debug!("Autodetected Docker cgroup driver {guess:?}.");
    if let Some(force) = cli.docker_cgroup_driver {
        if force != guess { warn!("It looks like this system is using the Docker {guess:?} cgroup driver, but this has been overridden to {force:?}."); }
        return force;
    }
    guess
}

fn figure_out_cgroup_ver() -> CgroupVersion {
    let cli = crate::cli::cfg();
    let dir = &cli.cgroupfs_dir;
    let mut ls = std::fs::read_dir(dir).unwrap_or_else(|_| panic!("Failed to read {:?} directory.", &dir));
    let guess = if ls.any(|entry| entry.unwrap().file_name() == "memory") {
        CgroupVersion::V1
    } else {
        CgroupVersion::V2
    };
    debug!("Autodetected cgroup version {guess:?}.");
    if let Some(force) = cli.cgroup_version {
        if force != guess { warn!("It looks like this system is using cgroup {guess:?}, but this has been overridden to {force:?}."); }
        return force;
    }
    guess
}

pub fn print_cgroup_detection_results() {
    info!("Assuming: cgroup version {:?}, Docker cgroup driver {:?}.",
        *CGROUP_VER, *DOCKER_CG_DRIVER);
}

fn dir_name_to_cont_id(dir_name: &str) -> &str {
    match *DOCKER_CG_DRIVER {
        DockerCgroupDriver::Cgroupfs => dir_name,
        DockerCgroupDriver::Systemd => &dir_name[7..71]
        // slice might panic, but we've already checked appropriate length
    }
}

pub fn get_metrics_string() -> Result<String> {
    let mut output = String::with_capacity(1024);
    output += &get_memory_metric()?;
    output += &get_cpu_metrics()?;
    output += &get_blkio_metrics()?;
    Ok(output)
}

fn get_memory_metric() -> Result<String> {
    let mut metric_rss = PrometheusMetric::build()
        .with_name("container_memory_usage")
        .with_metric_type(MetricType::Gauge)
        .with_help("Memory used by the container, in bytes")
        .build();

    let memory_dirs = fs::read_dir(&*MEMORY_DIR).unwrap_or_else(|_| panic!("Couldn't read memory directory {:?}", *MEMORY_DIR));
    for memory_dir_sub in memory_dirs.filter_map(Result::ok) {
        if !memory_dir_sub.file_type().unwrap().is_dir()
            || memory_dir_sub.file_name().len() != *EXPECTED_DIR_NAME_LEN { continue }

        let memory_usage: u64 = fs::read_to_string(memory_dir_sub.path().join(match *CGROUP_VER {
            CgroupVersion::V1 => "memory.usage_in_bytes",
            CgroupVersion::V2 => "memory.current"
        }))?.trim_end().parse()?;

        let dir_name = memory_dir_sub.file_name().into_string();
        if let Err(ref e) = dir_name { error!("Failed to read dirname {e:?}"); continue };
        let dir_name = dir_name.unwrap();
        let cont_id = dir_name_to_cont_id(&dir_name);
        render_and_append_instance(&mut metric_rss, memory_usage, cont_id);
    }

    Ok(metric_rss.render() + "\n")
}

fn get_cpu_metrics() -> Result<String> {
    let mut metric_user = PrometheusMetric::build()
        .with_name("container_cpu_user_total")
        .with_metric_type(MetricType::Counter)
        .with_help("CPU seconds used by the container in userspace")
        .build();

    let mut metric_sys = PrometheusMetric::build()
        .with_name("container_cpu_system_total")
        .with_metric_type(MetricType::Counter)
        .with_help("CPU seconds used by the container in kernelspace")
        .build();

    let cpu_dirs = fs::read_dir(&*CPU_DIR).unwrap_or_else(|_| panic!("Couldn't read CPU directory {:?}", *CPU_DIR));
    for cpu_dir_sub in cpu_dirs.filter_map(Result::ok) {
        if !cpu_dir_sub.file_type().unwrap().is_dir()
            || cpu_dir_sub.file_name().len() != *EXPECTED_DIR_NAME_LEN { continue }

        fn get_metrics(dir: PathBuf) -> Result<(f64, f64, String)> {
            let dir_name = dir.file_name().unwrap().to_owned().into_string()
                .map_err(|x| Error::msg(format!("Failed to read dirname {:?}", x)))?;
            if *CGROUP_VER == CgroupVersion::V1 {
                let usage_user_ns: f64 = fs::read_to_string(dir.join("cpuacct.usage_user"))?.trim_end().parse()?;
                let usage_sys_ns:  f64 = fs::read_to_string(dir.join("cpuacct.usage_sys" ))?.trim_end().parse()?;
                Ok((usage_user_ns / 1_000_000_000.0, usage_sys_ns / 1_000_000_000.0, dir_name))
            } else {
                let cpu_stat_file = dir.join("cpu.stat");
                let cpu_stat = fs::read_to_string(&cpu_stat_file)?;
                let mut user_us: Option<f64> = None;
                let mut sys_us: Option<f64> = None;
                for line in cpu_stat.lines() {
                    if line.starts_with("user_usec") {
                        user_us = Some(line.split_ascii_whitespace().last()
                            .ok_or(Error::msg("Couldn't split user_usec line in cpu.stat"))?.parse()?);
                    } else if line.starts_with("system_usec") {
                        sys_us = Some(line.split_ascii_whitespace().last()
                            .ok_or(Error::msg("Couldn't split system_usec line in cpu.stat"))?.parse()?);
                    }
                }
                if let (Some(user_us), Some(sys_us)) = (user_us, sys_us) {
                    Ok((user_us / 1_000_000.0, sys_us / 1_000_000.0, dir_name))
                } else {
                    Err(anyhow::anyhow!("Couldn't find one of user_usec or system_usec in {cpu_stat_file:?}"))
                }
            }
        }

        match get_metrics(cpu_dir_sub.path()) {
            Ok((usage_user_sec, usage_sys_sec, dir_name)) => {
                let cont_id = dir_name_to_cont_id(&dir_name);
                render_and_append_instance(&mut metric_user, usage_user_sec, cont_id);
                render_and_append_instance(&mut metric_sys,  usage_sys_sec,  cont_id);
            }
            Err(e) => error!("Metrics parsing error: {e}")
        }
    }

    let mut out = metric_user.render() + "\n";
    out += &metric_sys.render();
    Ok(out + "\n")
}

fn get_blkio_metrics() -> Result<String> {
    let mut metric_read = PrometheusMetric::build()
        .with_name("container_blkio_read_total")
        .with_metric_type(MetricType::Counter)
        .with_help("Bytes read from disk by the container")
        .build();

    let mut metric_write = PrometheusMetric::build()
        .with_name("container_blkio_write_total")
        .with_metric_type(MetricType::Counter)
        .with_help("Bytes written to disk by the container")
        .build();

    let blkio_dirs = fs::read_dir(&*BLKIO_DIR).unwrap_or_else(|_| panic!("Couldn't read blkio directory {:?}", *BLKIO_DIR));
    for blkio_dir_sub in blkio_dirs.filter_map(Result::ok) {
        if !blkio_dir_sub.file_type().unwrap().is_dir()
            || blkio_dir_sub.file_name().len() != *EXPECTED_DIR_NAME_LEN { continue }

        fn get_metrics(dir: PathBuf) -> Result<(u64, u64, String)> {
            let dir_name = dir.file_name().unwrap().to_owned().into_string()
                .map_err(|x| Error::msg(format!("Failed to read dirname {:?}", x)))?;

            let mut total_read:  u64 = 0;
            let mut total_write: u64 = 0;

            if *CGROUP_VER == CgroupVersion::V1 {
                let io_service_bytes = fs::read_to_string(dir.join("blkio.throttle.io_service_bytes"))?;
                for line in io_service_bytes.lines() {
                    if line.contains("Read") {
                        total_read += line.split_ascii_whitespace().last()
                            .ok_or(Error::msg("Couldn't split Read line in blkio.throttle.io_service_bytes"))?.parse::<u64>()?;
                    } else if line.contains("Write") {
                        total_write += line.split_ascii_whitespace().last()
                            .ok_or(Error::msg("Couldn't split Write line in blkio.throttle.io_service_bytes"))?.parse::<u64>()?;
                    }
                }
            } else {
                let io_stat = fs::read_to_string(dir.join("io.stat"))?;
                for line in io_stat.lines() {
                    for kv in line.split_ascii_whitespace() {
                        if kv.contains('=') {
                            let mut spl = kv.split('=');
                            let first = spl.next().ok_or(Error::msg("Couldn't split kv pair in io.stat"))?;
                            let last: u64 = spl.last().ok_or(Error::msg("Couldn't split kv pair in io.stat"))?.parse()?;
                            match first {
                                "rbytes" => total_read  += last,
                                "wbytes" => total_write += last,
                                _ => ()
                            }
                        }
                    }
                }
            }

            Ok((total_read, total_write, dir_name))
        }

        match get_metrics(blkio_dir_sub.path()) {
            Ok((total_read, total_write, dir_name)) => {
                let cont_id = dir_name_to_cont_id(&dir_name);
                render_and_append_instance(&mut metric_read, total_read, cont_id);
                render_and_append_instance(&mut metric_write, total_write, cont_id);
            }
            Err(e) => error!("Metrics parsing error: {e}")
        }
    }

    let mut out = metric_read.render() + "\n";
    out += &metric_write.render();
    Ok(out + "\n")
}

fn render_and_append_instance<N: num::Num + std::fmt::Display + core::fmt::Debug>(metric: &mut PrometheusMetric<'_>, value: N, cont_id: &str) {
    let mut prom = PrometheusInstance::new()
        .with_value(value)
        .with_label("id", cont_id)
        .with_current_timestamp()
        .expect("error getting UNIX time for timestamp");

    let mut map = crate::containers::CONTAINERS_MAP.lock().unwrap();
    let label_keys: append_only_vec::AppendOnlyVec<String> = append_only_vec::AppendOnlyVec::new();

    if !map.contains_key(cont_id) {
        refresh_containers_map(&mut map);
    }

    let include_labels = &cfg().include_labels_set;
    let exclude_labels = &cfg().exclude_labels_set;
    
    if let Some(cont) = map.get(cont_id) {
        prom = prom
            .with_label("name", &*cont.name)
            .with_label("image", &*cont.config.image);

        for (label_key, label_val) in &cont.config.labels {
            trace!("Inserting label {} ...", label_key);
            if !include_labels.is_empty() {
                if !include_labels.contains(label_key) { trace!("Not included."); continue; }
            } else if !exclude_labels.is_empty() && exclude_labels.contains(label_key) {
                trace!("Excluded.");
                continue;
            }
            let key = format!("container_label_{}", label_key).replace('.', "_").replace('-', "_");
            let idx = label_keys.push(key);
            prom = prom.with_label(&*label_keys[idx], label_val.as_str());
        }
    } else {
        warn!("Couldn't find details for container ID {cont_id}");
    }

    metric.render_and_append_instance(&prom);
}