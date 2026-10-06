//! Opt-in worker inventory. Native probes run off the lease loop and have a deadline.
use serde_json::{Value, json};
#[cfg(any(target_os = "linux", test))]
use std::path::Path;
use std::time::Duration;
#[cfg(any(windows, target_os = "macos"))]
use tokio::process::Command;

pub(crate) async fn detect() -> Value {
    let mut info = json!({
        "hostname": std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default(),
        "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
        "cpu_name": "", "cpu": std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get),
        "ram_mb": null, "gpu": null, "gpu_names": [], "errors": []
    });
    let result = tokio::time::timeout(Duration::from_secs(10), platform()).await;
    match result {
        Ok(Ok(found)) => merge(&mut info, &found),
        Ok(Err(error)) => info["errors"] = json!([format!("hardware detection: {error}")]),
        Err(_) => info["errors"] = json!(["hardware detection timed out"]),
    }
    info
}

fn merge(info: &mut Value, found: &Value) {
    if let Some(fields) = found.as_object() {
        for (key, value) in fields {
            if info.get(key).is_some() && !(key == "cpu" && value.as_u64().unwrap_or(0) == 0) {
                info[key] = value.clone();
            }
        }
    }
}

#[cfg(any(windows, target_os = "macos"))]
async fn output(program: &str, args: &[&str]) -> anyhow::Result<String> {
    let mut command = Command::new(program);
    command.args(args).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let result = command.output().await?;
    anyhow::ensure!(result.status.success(), "{program} failed");
    Ok(String::from_utf8(result.stdout)?)
}

#[cfg(windows)]
async fn platform() -> anyhow::Result<Value> {
    // Each inventory component can fail independently; unknown GPU is not zero.
    let script = r"$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[System.Text.UTF8Encoding]::new();
$r=@{hostname=$env:COMPUTERNAME;cpu_name='';ram_mb=$null;gpu=$null;gpu_names=@();errors=@()};
try { $p=@(Get-CimInstance Win32_Processor); $r.cpu=[int](($p | Measure-Object NumberOfLogicalProcessors -Sum).Sum); $r.cpu_name=($p.Name -join ', ') } catch { $r.errors+= 'CPU detection failed' };
try { $c=Get-CimInstance Win32_ComputerSystem; $r.ram_mb=[long][math]::Floor($c.TotalPhysicalMemory/1MB) } catch { $r.errors+= 'RAM detection failed' };
try { $g=@(Get-CimInstance Win32_VideoController | Where-Object { $_.PNPDeviceID -like 'PCI\*' }); $r.gpu=$g.Count; $r.gpu_names=@($g | ForEach-Object { $_.Name }) } catch { $r.errors+= 'GPU detection failed' };
$r | ConvertTo-Json -Compress -Depth 4";
    Ok(serde_json::from_str(
        &output(
            "powershell.exe",
            &["-NoProfile", "-NonInteractive", "-Command", script],
        )
        .await?,
    )?)
}

#[cfg(target_os = "linux")]
async fn platform() -> anyhow::Result<Value> {
    let cpu = tokio::fs::read_to_string("/proc/cpuinfo").await?;
    let memory = tokio::fs::read_to_string("/proc/meminfo").await?;
    let cpu_name = cpu
        .lines()
        .find_map(|line| {
            line.strip_prefix("model name")
                .and_then(|v| v.split_once(':'))
                .map(|(_, name)| name.trim())
        })
        .unwrap_or("");
    let ram_mb = memory
        .lines()
        .find_map(|line| {
            line.strip_prefix("MemTotal:")
                .and_then(|v| v.split_whitespace().next())
                .and_then(|v| v.parse::<u64>().ok())
        })
        .map(|kb| kb / 1024);
    let (gpu, names, errors) = linux_gpus(Path::new("/sys/bus/pci/devices"));
    Ok(
        json!({"cpu":cpu.lines().filter(|line|line.starts_with("processor\t")).count(),"cpu_name":cpu_name,"ram_mb":ram_mb,"gpu":gpu,"gpu_names":names,"errors":errors}),
    )
}

#[cfg(any(target_os = "linux", test))]
fn linux_gpus(root: &Path) -> (Option<usize>, Vec<String>, Vec<String>) {
    let mut names = Vec::new();
    let Ok(devices) = std::fs::read_dir(root) else {
        return (None, names, vec!["GPU detection unavailable".into()]);
    };
    for device in devices {
        let Ok(device) = device else {
            return (None, names, vec!["GPU detection incomplete".into()]);
        };
        let Ok(class) = std::fs::read_to_string(device.path().join("class")) else {
            return (None, names, vec!["GPU detection incomplete".into()]);
        };
        if class.trim().starts_with("0x03") {
            let vendor = std::fs::read_to_string(device.path().join("vendor")).unwrap_or_default();
            let model = std::fs::read_to_string(device.path().join("device")).unwrap_or_default();
            names.push(format!("PCI GPU {}:{}", vendor.trim(), model.trim()));
        }
    }
    (Some(names.len()), names, Vec::new())
}

#[cfg(target_os = "macos")]
async fn platform() -> anyhow::Result<Value> {
    let cpu = output("sysctl", &["-n", "hw.logicalcpu"])
        .await?
        .trim()
        .parse::<u32>()?;
    let ram = output("sysctl", &["-n", "hw.memsize"])
        .await?
        .trim()
        .parse::<u64>()?
        / 1024
        / 1024;
    let name = output("sysctl", &["-n", "machdep.cpu.brand_string"])
        .await
        .unwrap_or_default();
    let displays = output("system_profiler", &["SPDisplaysDataType", "-json"]).await;
    let mut info = json!({"cpu":cpu,"cpu_name":name.trim(),"ram_mb":ram,"gpu":null,"gpu_names":[],"errors":[]});
    match displays.and_then(|value| Ok(serde_json::from_str::<Value>(&value)?)) {
        Ok(value) if value["SPDisplaysDataType"].is_array() => {
            let adapters = value["SPDisplaysDataType"].as_array().unwrap();
            info["gpu"] = json!(adapters.len());
            info["gpu_names"] = json!(
                adapters
                    .iter()
                    .filter_map(|g| g["sppci_model"].as_str())
                    .collect::<Vec<_>>()
            );
        }
        _ => info["errors"] = json!(["GPU detection unavailable"]),
    }
    Ok(info)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
async fn platform() -> anyhow::Result<Value> {
    anyhow::bail!("hardware inventory is unsupported on this platform")
}

#[cfg(test)]
#[path = "../../../test/distributed_host.rs"]
mod tests;
