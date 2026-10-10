//! On-demand local system readings for the AX /system panel.
//! AX Crew has its own native collector; this module does not feed its UI.
use nvml_wrapper::Nvml;
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub cpu_usage: Option<f32>,
    pub memory_total_mb: Option<u64>,
    pub memory_used_mb: Option<u64>,
    pub memory_usage: Option<f64>,
    pub cpu_cores: Option<usize>,
    pub gpus: Option<Vec<GpuInfo>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    pub memory_total_mb: Option<u64>,
    pub memory_used_mb: Option<u64>,
    pub usage: Option<u32>,
}

#[allow(clippy::cast_precision_loss)]
pub fn get_system_info() -> SystemInfo {
    // No collectors run on startup. The caller uses the blocking pool.
    let mut system = System::new_with_specifics(
        RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing().with_cpu_usage()),
    );
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    system.refresh_cpu_usage();
    system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
    let total = system.total_memory();
    let used = system.used_memory();
    let has_memory = total > 0 && used <= total;
    let cpu = system.global_cpu_usage();
    SystemInfo {
        cpu_usage: (!system.cpus().is_empty() && cpu.is_finite() && (0.0..=100.0).contains(&cpu))
            .then_some(cpu),
        cpu_cores: (!system.cpus().is_empty()).then_some(system.cpus().len()),
        memory_total_mb: has_memory.then_some(total / 1024 / 1024),
        memory_used_mb: has_memory.then_some(used / 1024 / 1024),
        memory_usage: has_memory.then(|| used as f64 * 100.0 / total as f64),
        gpus: collect_gpus(),
    }
}

fn collect_gpus() -> Option<Vec<GpuInfo>> {
    static DRIVER: OnceLock<Mutex<Option<Nvml>>> = OnceLock::new();
    let mut driver = DRIVER.get_or_init(|| Mutex::new(None)).lock().ok()?;
    if driver.is_none() {
        *driver = Some(Nvml::init().ok()?);
    }
    let nvml = driver.as_ref()?;
    let count = nvml.device_count().ok()?;
    Some(
        (0..count)
            .map(|index| {
                let device = nvml.device_by_index(index).ok();
                let memory = device
                    .as_ref()
                    .and_then(|gpu| gpu.memory_info().ok())
                    .filter(|value| value.total > 0 && value.used <= value.total);
                GpuInfo {
                    name: device
                        .as_ref()
                        .and_then(|gpu| gpu.name().ok())
                        .unwrap_or_else(|| format!("NVIDIA GPU {index}")),
                    memory_total_mb: memory.as_ref().map(|value| value.total / 1024 / 1024),
                    memory_used_mb: memory.as_ref().map(|value| value.used / 1024 / 1024),
                    usage: device
                        .as_ref()
                        .and_then(|gpu| gpu.utilization_rates().ok())
                        .map(|value| value.gpu)
                        .filter(|value| *value <= 100),
                }
            })
            .collect(),
    )
}

#[cfg(test)]
#[path = "../../../test/system_info.rs"]
mod tests;
