use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use axum::{extract::State, response::Json as ResponseJson};
use chrono::{DateTime, Utc};
use serde::Serialize;
use ts_rs::TS;
use utils::response::ApiResponse;
use workspace_manager::WorkspaceManager;

use crate::{DeploymentImpl, error::ApiError};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, TS)]
pub struct LoadAverage {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
pub struct ByteUsage {
    #[ts(type = "number")]
    pub used_bytes: u64,
    #[ts(type = "number")]
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct DiskUsage {
    /// Path that was measured (the first existing ancestor of the requested path).
    pub path: String,
    #[ts(type = "number")]
    pub used_bytes: u64,
    #[ts(type = "number")]
    pub total_bytes: u64,
}

/// Point-in-time host resource snapshot. Fields are `null` when the
/// information is not available on the current OS.
#[derive(Debug, Clone, Serialize, TS)]
pub struct HostStats {
    pub os: String,
    pub cpu_count: usize,
    pub load_average: Option<LoadAverage>,
    pub cpu_utilization_percent: Option<f64>,
    pub memory: Option<ByteUsage>,
    pub swap: Option<ByteUsage>,
    pub workspace_disk: Option<DiskUsage>,
    pub root_disk: Option<DiskUsage>,
    #[ts(type = "number | null")]
    pub uptime_seconds: Option<u64>,
    pub sampled_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CpuTimes {
    pub idle: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MemInfo {
    pub memory: ByteUsage,
    pub swap: ByteUsage,
}

pub(crate) fn parse_loadavg(text: &str) -> Option<LoadAverage> {
    let mut parts = text.split_whitespace();
    let one = parts.next()?.parse().ok()?;
    let five = parts.next()?.parse().ok()?;
    let fifteen = parts.next()?.parse().ok()?;
    Some(LoadAverage { one, five, fifteen })
}

pub(crate) fn parse_uptime(text: &str) -> Option<u64> {
    let secs: f64 = text.split_whitespace().next()?.parse().ok()?;
    if secs.is_finite() && secs >= 0.0 {
        Some(secs as u64)
    } else {
        None
    }
}

/// Parses the aggregate `cpu` line of `/proc/stat`.
pub(crate) fn parse_proc_stat(text: &str) -> Option<CpuTimes> {
    let line = text
        .lines()
        .find(|line| line.split_whitespace().next() == Some("cpu"))?;
    let values: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .map(|v| v.parse().ok())
        .collect::<Option<_>>()?;
    if values.len() < 4 {
        return None;
    }
    // user nice system idle iowait irq softirq steal; guest time is already
    // included in user/nice.
    let total = values.iter().take(8).sum();
    let idle = values[3] + values.get(4).copied().unwrap_or(0);
    Some(CpuTimes { idle, total })
}

pub(crate) fn cpu_utilization(prev: CpuTimes, curr: CpuTimes) -> Option<f64> {
    let total = curr.total.checked_sub(prev.total)?;
    let idle = curr.idle.checked_sub(prev.idle)?;
    if total == 0 || idle > total {
        return None;
    }
    Some(((total - idle) as f64 / total as f64) * 100.0)
}

pub(crate) fn parse_meminfo(text: &str) -> Option<MemInfo> {
    let mut mem_total = None;
    let mut mem_available = None;
    let mut mem_free = None;
    let mut swap_total = None;
    let mut swap_free = None;

    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(key), Some(value)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(kib) = value.parse::<u64>() else {
            continue;
        };
        let bytes = kib.saturating_mul(1024);
        match key {
            "MemTotal:" => mem_total = Some(bytes),
            "MemAvailable:" => mem_available = Some(bytes),
            "MemFree:" => mem_free = Some(bytes),
            "SwapTotal:" => swap_total = Some(bytes),
            "SwapFree:" => swap_free = Some(bytes),
            _ => {}
        }
    }

    let mem_total = mem_total?;
    let mem_available = mem_available.or(mem_free)?;
    let swap_total = swap_total.unwrap_or(0);
    let swap_free = swap_free.unwrap_or(swap_total);

    Some(MemInfo {
        memory: ByteUsage {
            used_bytes: mem_total.saturating_sub(mem_available),
            total_bytes: mem_total,
        },
        swap: ByteUsage {
            used_bytes: swap_total.saturating_sub(swap_free),
            total_bytes: swap_total,
        },
    })
}

fn read_proc(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn existing_ancestor(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|p| p.exists()).map(Path::to_path_buf)
}

#[cfg(unix)]
#[allow(clippy::useless_conversion)] // statvfs field widths differ between platforms
fn disk_usage(path: &Path) -> Option<DiskUsage> {
    let path = existing_ancestor(path)?;
    let stat = nix::sys::statvfs::statvfs(&path).ok()?;
    let fragment = u64::from(stat.fragment_size());
    let total = u64::from(stat.blocks()).saturating_mul(fragment);
    let free = u64::from(stat.blocks_free()).saturating_mul(fragment);
    Some(DiskUsage {
        path: path.to_string_lossy().into_owned(),
        used_bytes: total.saturating_sub(free),
        total_bytes: total,
    })
}

#[cfg(not(unix))]
fn disk_usage(_path: &Path) -> Option<DiskUsage> {
    None
}

static LAST_CPU_SAMPLE: Mutex<Option<(Instant, CpuTimes)>> = Mutex::new(None);

const CPU_SAMPLE_MAX_AGE: Duration = Duration::from_secs(60);
const CPU_FRESH_SAMPLE_WINDOW: Duration = Duration::from_millis(250);

/// Utilisation since the previous request; takes a short fresh sample when
/// there is no recent previous sample.
async fn sample_cpu_utilization() -> Option<f64> {
    let current = parse_proc_stat(&read_proc("/proc/stat")?)?;
    let now = Instant::now();

    let previous = {
        let mut guard = LAST_CPU_SAMPLE.lock().ok()?;
        let previous = guard.replace((now, current));
        previous.filter(|(at, _)| now.duration_since(*at) <= CPU_SAMPLE_MAX_AGE)
    };

    if let Some((_, prev)) = previous
        && current.total > prev.total
    {
        return cpu_utilization(prev, current);
    }

    tokio::time::sleep(CPU_FRESH_SAMPLE_WINDOW).await;
    let later = parse_proc_stat(&read_proc("/proc/stat")?)?;
    if let Ok(mut guard) = LAST_CPU_SAMPLE.lock() {
        *guard = Some((Instant::now(), later));
    }
    cpu_utilization(current, later)
}

pub async fn collect_host_stats() -> HostStats {
    let cpu_utilization_percent = sample_cpu_utilization().await;
    let workspace_dir = WorkspaceManager::get_workspace_base_dir();

    let blocking = tokio::task::spawn_blocking(move || {
        let meminfo = read_proc("/proc/meminfo").and_then(|t| parse_meminfo(&t));
        (
            read_proc("/proc/loadavg").and_then(|t| parse_loadavg(&t)),
            meminfo,
            disk_usage(&workspace_dir),
            disk_usage(Path::new("/")),
            read_proc("/proc/uptime").and_then(|t| parse_uptime(&t)),
        )
    })
    .await;
    let (load_average, meminfo, workspace_disk, root_disk, uptime_seconds) =
        blocking.unwrap_or((None, None, None, None, None));

    HostStats {
        os: std::env::consts::OS.to_string(),
        cpu_count: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
        load_average,
        cpu_utilization_percent,
        memory: meminfo.map(|m| m.memory),
        swap: meminfo.map(|m| m.swap),
        workspace_disk,
        root_disk,
        uptime_seconds,
        sampled_at: Utc::now(),
    }
}

pub async fn get_host_stats(
    State(_deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<HostStats>>, ApiError> {
    Ok(ResponseJson(ApiResponse::success(
        collect_host_stats().await,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROC_STAT: &str = "cpu  4705 356 584 3699176 23060 0 277 0 0 0
cpu0 1393280 32966 572056 13343292 6130 0 17875 0 0 0
intr 114930548 113199788 3 0 5 263 0 4 [... lots more numbers ...]
ctxt 1990473
btime 1062191376
processes 2915
procs_running 1
procs_blocked 0
";

    const MEMINFO: &str = "MemTotal:       65843720 kB
MemFree:         1234567 kB
MemAvailable:   49382790 kB
Buffers:          123456 kB
Cached:         40000000 kB
SwapCached:            0 kB
SwapTotal:       8388604 kB
SwapFree:        8000000 kB
HugePages_Total:       0
";

    #[test]
    fn parses_loadavg() {
        let load = parse_loadavg("3.52 2.10 1.05 4/1234 56789\n").unwrap();
        assert_eq!(
            load,
            LoadAverage {
                one: 3.52,
                five: 2.10,
                fifteen: 1.05
            }
        );
        assert!(parse_loadavg("").is_none());
        assert!(parse_loadavg("abc 1 2").is_none());
    }

    #[test]
    fn parses_uptime() {
        assert_eq!(parse_uptime("350735.47 234388.90\n"), Some(350735));
        assert_eq!(parse_uptime(""), None);
        assert_eq!(parse_uptime("-1 0"), None);
    }

    #[test]
    fn parses_proc_stat_aggregate_line() {
        let times = parse_proc_stat(PROC_STAT).unwrap();
        assert_eq!(times.idle, 3699176 + 23060);
        assert_eq!(times.total, 4705 + 356 + 584 + 3699176 + 23060 + 277);
    }

    #[test]
    fn proc_stat_without_cpu_line_is_none() {
        assert!(parse_proc_stat("intr 1 2 3\nctxt 4\n").is_none());
        assert!(parse_proc_stat("cpu 1 2\n").is_none());
    }

    #[test]
    fn computes_cpu_utilization_from_deltas() {
        let prev = CpuTimes {
            idle: 1000,
            total: 2000,
        };
        let curr = CpuTimes {
            idle: 1300,
            total: 3000,
        };
        let pct = cpu_utilization(prev, curr).unwrap();
        assert!((pct - 70.0).abs() < 1e-9);
        assert!(cpu_utilization(curr, curr).is_none());
        assert!(cpu_utilization(curr, prev).is_none());
    }

    #[test]
    fn parses_meminfo() {
        let info = parse_meminfo(MEMINFO).unwrap();
        assert_eq!(info.memory.total_bytes, 65843720 * 1024);
        assert_eq!(info.memory.used_bytes, (65843720 - 49382790) * 1024);
        assert_eq!(info.swap.total_bytes, 8388604 * 1024);
        assert_eq!(info.swap.used_bytes, (8388604 - 8000000) * 1024);
    }

    #[test]
    fn meminfo_falls_back_to_memfree_and_missing_swap() {
        let info = parse_meminfo("MemTotal: 1000 kB\nMemFree: 400 kB\n").unwrap();
        assert_eq!(info.memory.used_bytes, 600 * 1024);
        assert_eq!(info.swap.total_bytes, 0);
        assert_eq!(info.swap.used_bytes, 0);
        assert!(parse_meminfo("SwapTotal: 10 kB\n").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn disk_usage_walks_up_to_existing_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does/not/exist");
        let usage = disk_usage(&missing).unwrap();
        assert_eq!(usage.path, dir.path().to_string_lossy());
        assert!(usage.total_bytes >= usage.used_bytes);
    }
}
