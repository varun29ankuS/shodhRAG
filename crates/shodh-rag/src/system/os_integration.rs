//! OS-specific integrations with cross-platform abstraction
//! Provides unified interface for Windows, macOS, and Linux features

use anyhow::{Result, anyhow, Context};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// System information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub os: String,
    pub os_version: String,
    pub architecture: String,
    pub hostname: String,
    pub cpu_count: usize,
    pub total_memory_mb: u64,
}

/// Running process information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: Option<f32>,
    pub memory_mb: Option<u64>,
}

/// Open path in system file manager
pub fn open_in_file_manager(path: &Path) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("Path does not exist: {:?}", path));
    }

    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        Command::new("explorer")
            .arg(path)
            .spawn()
            .map_err(|e| anyhow!("Failed to open in Explorer: {}", e))?;
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|e| anyhow!("Failed to open in Finder: {}", e))?;
    }

    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        // Try xdg-open first (standard)
        if Command::new("xdg-open").arg(path).spawn().is_ok() {
            return Ok(());
        }
        // Fallback to nautilus (GNOME)
        if Command::new("nautilus").arg(path).spawn().is_ok() {
            return Ok(());
        }
        // Fallback to dolphin (KDE)
        if Command::new("dolphin").arg(path).spawn().is_ok() {
            return Ok(());
        }
        return Err(anyhow!("No file manager found (tried xdg-open, nautilus, dolphin)"));
    }

    Ok(())
}

/// Get system information
pub fn get_system_info() -> Result<SystemInfo> {
    use std::env;

    let os = env::consts::OS.to_string();
    let architecture = env::consts::ARCH.to_string();

    // Get hostname
    let hostname = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string());

    // Get CPU count
    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    // Get OS version (platform-specific)
    let os_version = get_os_version();

    // Get total memory (requires sysinfo crate, simplified here)
    let total_memory_mb = get_total_memory_mb();

    Ok(SystemInfo {
        os,
        os_version,
        architecture,
        hostname,
        cpu_count,
        total_memory_mb,
    })
}

/// Get OS version string (cross-platform)
fn get_os_version() -> String {
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        Command::new("powershell")
            .args(["-Command", "(Get-WmiObject -Class Win32_OperatingSystem).Caption"])
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "Windows (unknown version)".to_string())
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        Command::new("sw_vers")
            .args(["-productVersion"])
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|s| format!("macOS {}", s.trim()))
            .unwrap_or_else(|| "macOS (unknown version)".to_string())
    }

    #[cfg(target_os = "linux")]
    {
        use std::fs;
        // Try to read /etc/os-release
        fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|content| {
                content
                    .lines()
                    .find(|line| line.starts_with("PRETTY_NAME="))
                    .map(|line| line.trim_start_matches("PRETTY_NAME=").trim_matches('"').to_string())
            })
            .unwrap_or_else(|| "Linux (unknown distribution)".to_string())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        "Unknown OS".to_string()
    }
}

/// Get total system memory in MB
fn get_total_memory_mb() -> u64 {
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        Command::new("powershell")
            .args(["-Command", "(Get-WmiObject -Class Win32_ComputerSystem).TotalPhysicalMemory"])
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(|bytes| bytes / 1024 / 1024)
            .unwrap_or(0)
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(|bytes| bytes / 1024 / 1024)
            .unwrap_or(0)
    }

    #[cfg(target_os = "linux")]
    {
        use std::fs;
        // Read /proc/meminfo
        fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|content| {
                content
                    .lines()
                    .find(|line| line.starts_with("MemTotal:"))
                    .and_then(|line| {
                        line.split_whitespace()
                            .nth(1)
                            .and_then(|kb| kb.parse::<u64>().ok())
                            .map(|kb| kb / 1024)
                    })
            })
            .unwrap_or(0)
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        0
    }
}

/// List running processes (simplified, platform-specific)
pub fn list_running_processes() -> Result<Vec<ProcessInfo>> {
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        let output = Command::new("powershell")
            .args(["-Command", "Get-Process | Select-Object Id,Name,CPU,WorkingSet | ConvertTo-Json"])
            .output()
            .context("Failed to list processes")?;

        let json_str = String::from_utf8_lossy(&output.stdout);
        let processes: Vec<serde_json::Value> = serde_json::from_str(&json_str)
            .unwrap_or_default();

        Ok(processes
            .into_iter()
            .filter_map(|p| {
                Some(ProcessInfo {
                    pid: p["Id"].as_u64()? as u32,
                    name: p["Name"].as_str()?.to_string(),
                    cpu_percent: p["CPU"].as_f64().map(|c| c as f32),
                    memory_mb: p["WorkingSet"].as_u64().map(|ws| ws / 1024 / 1024),
                })
            })
            .collect())
    }

    #[cfg(not(target_os = "windows"))]
    {
        use std::process::Command;
        let output = Command::new("ps")
            .args(["aux"])
            .output()
            .context("Failed to list processes")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let processes: Vec<ProcessInfo> = stdout
            .lines()
            .skip(1) // Skip header
            .filter_map(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 11 {
                    Some(ProcessInfo {
                        pid: parts[1].parse().ok()?,
                        name: parts[10].to_string(),
                        cpu_percent: parts[2].parse().ok(),
                        memory_mb: None, // ps doesn't give direct MB
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(processes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_system_info() {
        let info = get_system_info().unwrap();
        println!("System Info: {:?}", info);

        assert!(!info.os.is_empty());
        assert!(!info.hostname.is_empty());
        assert!(info.cpu_count > 0);
    }

    #[test]
    fn test_list_processes() {
        let processes = list_running_processes().unwrap();
        println!("Found {} processes", processes.len());
        assert!(!processes.is_empty());

        // Print first 5 for debugging
        for process in processes.iter().take(5) {
            println!("Process: {:?}", process);
        }
    }
}
