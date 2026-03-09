use super::tool_schema::{RiskLevel, ToolDefinition};
use serde::Deserialize;
use sysinfo::{Pid, System};

#[derive(Debug, Deserialize)]
#[serde(tag = "action")]
enum ProcessInput {
    /// List all running processes.
    #[serde(rename = "list")]
    List {
        /// Optional filter by process name (case-insensitive substring match).
        #[serde(default)]
        filter: Option<String>,
        /// Maximum number of processes to return (default 100).
        #[serde(default)]
        limit: Option<usize>,
    },
    /// Get detailed info about a specific process by PID.
    #[serde(rename = "info")]
    Info { pid: u32 },
    /// Get overall system information.
    #[serde(rename = "system_info")]
    SystemInfo,
    /// Get disk usage information.
    #[serde(rename = "disk_info")]
    DiskInfo,
}

/// Process listing and system info tool using the sysinfo crate.
pub struct ProcessTool;

impl ProcessTool {
    pub fn new() -> Self {
        Self
    }

    /// Execute a process/system info query.
    pub async fn execute(&self, input: &serde_json::Value) -> Result<serde_json::Value, String> {
        let params: ProcessInput = serde_json::from_value(input.clone())
            .map_err(|e| format!("Invalid process input: {}", e))?;

        // sysinfo operations are CPU-bound, run in a blocking thread
        tokio::task::spawn_blocking(move || match params {
            ProcessInput::List { filter, limit } => list_processes(filter, limit),
            ProcessInput::Info { pid } => process_info(pid),
            ProcessInput::SystemInfo => system_info(),
            ProcessInput::DiskInfo => disk_info(),
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))?
    }

    /// Return the tool definition for capability reporting.
    pub fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "process".to_string(),
            description: "List running processes, get process details, and query system information (CPU, memory, disks, uptime).".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "info", "system_info", "disk_info"],
                        "description": "The action to perform"
                    },
                    "pid": {
                        "type": "integer",
                        "description": "Process ID (for 'info' action)"
                    },
                    "filter": {
                        "type": "string",
                        "description": "Filter processes by name substring (for 'list' action)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of processes to return"
                    }
                }
            }),
            requires_approval: false,
            risk_level: RiskLevel::Low,
        }
    }
}

impl Default for ProcessTool {
    fn default() -> Self {
        Self::new()
    }
}

/// List running processes, optionally filtered by name.
fn list_processes(
    filter: Option<String>,
    limit: Option<usize>,
) -> Result<serde_json::Value, String> {
    let mut sys = System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let limit = limit.unwrap_or(100);
    let filter_lower = filter.as_ref().map(|f| f.to_lowercase());

    let mut processes: Vec<serde_json::Value> = sys
        .processes()
        .iter()
        .filter(|(_, proc_info)| {
            if let Some(ref f) = filter_lower {
                proc_info
                    .name()
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(f)
            } else {
                true
            }
        })
        .take(limit)
        .map(|(pid, proc_info)| {
            serde_json::json!({
                "pid": pid.as_u32(),
                "name": proc_info.name().to_string_lossy(),
                "cpu_usage": proc_info.cpu_usage(),
                "memory_bytes": proc_info.memory(),
                "status": format!("{:?}", proc_info.status()),
                "parent_pid": proc_info.parent().map(|p| p.as_u32()),
            })
        })
        .collect();

    // Sort by CPU usage descending
    processes.sort_by(|a, b| {
        let cpu_a = a.get("cpu_usage").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let cpu_b = b.get("cpu_usage").and_then(|v| v.as_f64()).unwrap_or(0.0);
        cpu_b
            .partial_cmp(&cpu_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(serde_json::json!({
        "processes": processes,
        "count": processes.len(),
        "total_processes": sys.processes().len(),
    }))
}

/// Get detailed info about a specific process.
fn process_info(pid: u32) -> Result<serde_json::Value, String> {
    let mut sys = System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let sysinfo_pid = Pid::from_u32(pid);
    let proc_info = sys
        .process(sysinfo_pid)
        .ok_or_else(|| format!("Process {} not found", pid))?;

    Ok(serde_json::json!({
        "pid": pid,
        "name": proc_info.name().to_string_lossy(),
        "exe": proc_info.exe().map(|p| p.to_string_lossy().to_string()),
        "cmd": proc_info.cmd().iter().map(|s| s.to_string_lossy().to_string()).collect::<Vec<_>>(),
        "cwd": proc_info.cwd().map(|p| p.to_string_lossy().to_string()),
        "environ": proc_info.environ().iter().take(50).map(|s| s.to_string_lossy().to_string()).collect::<Vec<_>>(),
        "cpu_usage": proc_info.cpu_usage(),
        "memory_bytes": proc_info.memory(),
        "virtual_memory_bytes": proc_info.virtual_memory(),
        "status": format!("{:?}", proc_info.status()),
        "parent_pid": proc_info.parent().map(|p| p.as_u32()),
        "start_time": proc_info.start_time(),
        "run_time": proc_info.run_time(),
    }))
}

/// Get overall system information.
fn system_info() -> Result<serde_json::Value, String> {
    let mut sys = System::new_all();
    sys.refresh_all();

    Ok(serde_json::json!({
        "system": {
            "name": System::name(),
            "os_version": System::os_version(),
            "kernel_version": System::kernel_version(),
            "host_name": System::host_name(),
            "uptime_seconds": System::uptime(),
        },
        "cpu": {
            "global_usage_percent": sys.global_cpu_usage(),
            "physical_core_count": sys.physical_core_count(),
            "cpus": sys.cpus().iter().map(|cpu| {
                serde_json::json!({
                    "name": cpu.name(),
                    "usage_percent": cpu.cpu_usage(),
                    "frequency_mhz": cpu.frequency(),
                })
            }).collect::<Vec<_>>(),
        },
        "memory": {
            "total_bytes": sys.total_memory(),
            "used_bytes": sys.used_memory(),
            "free_bytes": sys.free_memory(),
            "available_bytes": sys.available_memory(),
            "total_swap_bytes": sys.total_swap(),
            "used_swap_bytes": sys.used_swap(),
        },
    }))
}

/// Get disk usage information.
fn disk_info() -> Result<serde_json::Value, String> {
    let disks = sysinfo::Disks::new_with_refreshed_list();

    let disk_list: Vec<serde_json::Value> = disks
        .iter()
        .map(|disk| {
            serde_json::json!({
                "name": disk.name().to_string_lossy(),
                "mount_point": disk.mount_point().to_string_lossy(),
                "file_system": String::from_utf8_lossy(disk.file_system().as_encoded_bytes()),
                "total_bytes": disk.total_space(),
                "available_bytes": disk.available_space(),
                "used_bytes": disk.total_space() - disk.available_space(),
                "is_removable": disk.is_removable(),
            })
        })
        .collect();

    Ok(serde_json::json!({
        "disks": disk_list,
        "count": disk_list.len(),
    }))
}
