//! Which processor runs the search model: a graphics card through the
//! platform's GPU provider (DirectML on Windows, Core ML on macOS), or the
//! CPU, with the reason when it is the CPU.
//!
//! Every GPU attempt reports its own error ([`super::ep`]), never a silent
//! fallback inside ONNX Runtime; [`super::choose`] decides which target runs
//! the model, with the reason kept and shown when it is the CPU.

use serde::{Deserialize, Serialize};

/// Whether the search model may use a graphics card.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComputePreference {
    /// A graphics card when there is a usable one, else the CPU.
    #[default]
    Auto,
    /// The CPU only (chosen by the user; the evals and tests, which measure
    /// the CPU path).
    Cpu,
}

/// What runs the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accelerator {
    /// DirectML on a Windows graphics card (any DirectX 12 GPU).
    DirectMl,
    /// Core ML on Apple silicon (its GPU and Neural Engine).
    CoreMl,
    Cpu,
}

impl Accelerator {
    pub fn is_gpu(self) -> bool {
        !matches!(self, Accelerator::Cpu)
    }

    /// The provider's name, as the status panel's details show it.
    pub fn label(self) -> &'static str {
        match self {
            Accelerator::DirectMl => "DirectML",
            Accelerator::CoreMl => "Core ML",
            Accelerator::Cpu => "CPU",
        }
    }
}

/// A graphics card a GPU provider can run on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub accelerator: Accelerator,
    /// The card's name ("NVIDIA GeForce RTX 4060", "Apple silicon GPU and
    /// Neural Engine").
    pub name: String,
    /// The provider's device index (DirectML: the DXGI adapter index).
    pub index: Option<i32>,
    /// The driver (Windows: the user-mode driver version) or system (macOS)
    /// version: a saved calibration holds only for the same one.
    pub driver: Option<String>,
}

/// What looking for a graphics card found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuProbe {
    /// This build has no GPU provider for the platform.
    Unsupported,
    /// The cards the provider can try, best first (none: only software
    /// adapters, or no card at all).
    Devices(Vec<Device>),
    /// The cards could not be listed.
    Failed(String),
}

/// Why the model runs on the CPU.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CpuReason {
    /// The CPU was chosen in Settings.
    Chosen,
    /// Shodh has no GPU provider on this platform.
    NoProvider,
    /// No graphics card the provider can use (none, or only a software one).
    NoGpu,
    /// The cards could not be listed, or the provider failed to start on
    /// them; `error` is the last failure.
    Failed {
        device: Option<String>,
        error: String,
    },
    /// The graphics card started but was slower than the CPU.
    SlowerThanCpu {
        device: String,
        gpu_tokens_per_second: f64,
        cpu_tokens_per_second: f64,
    },
}

/// Whether a graphics card that embeds at `gpu` tokens a second is kept
/// over the CPU at `cpu`: only when it is at least `margin` times as fast
/// ([`super::choose::GPU_MARGIN`]).
pub fn keep_gpu_with_margin(gpu: f64, cpu: f64, margin: f64) -> bool {
    gpu >= cpu * margin
}

/// The graphics cards of this machine the platform's provider can use.
pub fn probe_gpus() -> GpuProbe {
    platform::probe()
}

#[cfg(windows)]
mod platform {
    use super::{Accelerator, Device, GpuProbe};
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIDevice, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE,
        DXGI_ERROR_NOT_FOUND,
    };

    /// Microsoft's PCI vendor id: the Basic Render Driver (WARP) and the
    /// remote display adapters, which run on the CPU.
    const MICROSOFT_VENDOR: u32 = 0x1414;

    /// The hardware adapters DXGI lists, the one with the most dedicated
    /// memory first (a discrete card before an integrated one). Their index
    /// is DXGI's, which DirectML's device id refers to.
    pub fn probe() -> GpuProbe {
        // SAFETY: creating a DXGI factory has no preconditions.
        let factory: IDXGIFactory1 = match unsafe { CreateDXGIFactory1() } {
            Ok(factory) => factory,
            Err(e) => return GpuProbe::Failed(format!("listing the graphics cards failed: {e}")),
        };
        let mut found = Vec::new();
        for index in 0u32.. {
            // SAFETY: enumerating adapters by index; the end is reported as
            // DXGI_ERROR_NOT_FOUND.
            let adapter = match unsafe { factory.EnumAdapters1(index) } {
                Ok(adapter) => adapter,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(e) => {
                    return GpuProbe::Failed(format!("listing the graphics cards failed: {e}"))
                }
            };
            // SAFETY: reads the adapter's description into a returned struct.
            let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
                continue;
            };
            let software = desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0;
            if software || desc.VendorId == MICROSOFT_VENDOR {
                continue;
            }
            let end = desc
                .Description
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(desc.Description.len());
            let name = String::from_utf16_lossy(&desc.Description[..end])
                .trim()
                .to_string();
            let Ok(index) = i32::try_from(index) else {
                break;
            };
            // SAFETY: asks the adapter for its user-mode driver version.
            let driver = unsafe { adapter.CheckInterfaceSupport(&IDXGIDevice::IID) }
                .ok()
                .map(|v| {
                    let v = v as u64;
                    format!(
                        "{}.{}.{}.{}",
                        v >> 48,
                        (v >> 32) & 0xffff,
                        (v >> 16) & 0xffff,
                        v & 0xffff
                    )
                });
            found.push((
                desc.DedicatedVideoMemory,
                Device {
                    accelerator: Accelerator::DirectMl,
                    name: if name.is_empty() {
                        "Graphics card".to_string()
                    } else {
                        name
                    },
                    index: Some(index),
                    driver,
                },
            ));
        }
        // Stable: equal memory keeps DXGI's order (the primary card first).
        found.sort_by(|a, b| b.0.cmp(&a.0));
        GpuProbe::Devices(found.into_iter().map(|(_, device)| device).collect())
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod platform {
    use super::{Accelerator, Device, GpuProbe};

    /// The macOS build ("24A335"): Core ML comes with the system, so a new
    /// build is a new driver.
    fn os_version() -> Option<String> {
        let name = c"kern.osversion";
        let mut size: libc::size_t = 0;
        // SAFETY: a null buffer asks for the size only.
        let status = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                std::ptr::null_mut(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if status != 0 || size == 0 {
            return None;
        }
        let mut buffer = vec![0u8; size];
        // SAFETY: `buffer` holds `size` writable bytes.
        let status = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                buffer.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if status != 0 {
            return None;
        }
        buffer.truncate(size);
        let text = String::from_utf8_lossy(&buffer)
            .trim_end_matches(char::from(0))
            .trim()
            .to_string();
        (!text.is_empty()).then_some(text)
    }

    /// Every Apple silicon Mac has a GPU and a Neural Engine Core ML can use.
    pub fn probe() -> GpuProbe {
        GpuProbe::Devices(vec![Device {
            accelerator: Accelerator::CoreMl,
            name: "Apple silicon GPU and Neural Engine".to_string(),
            index: None,
            driver: os_version(),
        }])
    }
}

#[cfg(not(any(windows, all(target_os = "macos", target_arch = "aarch64"))))]
mod platform {
    use super::GpuProbe;

    /// No GPU provider is built for this platform (ONNX Runtime's prebuilt
    /// library for it has none).
    pub fn probe() -> GpuProbe {
        GpuProbe::Unsupported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gpu_is_kept_only_when_clearly_faster_than_the_cpu() {
        assert!(keep_gpu_with_margin(9000.0, 1500.0, 1.15));
        assert!(keep_gpu_with_margin(1725.0, 1500.0, 1.15));
        assert!(!keep_gpu_with_margin(1700.0, 1500.0, 1.15));
        assert!(!keep_gpu_with_margin(1200.0, 1500.0, 1.15));
    }

    #[test]
    fn the_reason_serializes_for_the_status_panel() {
        let reason = CpuReason::SlowerThanCpu {
            device: "Intel UHD".to_string(),
            gpu_tokens_per_second: 800.0,
            cpu_tokens_per_second: 1200.0,
        };
        let json = serde_json::to_value(&reason).expect("json");
        assert_eq!(json["kind"], "slowerThanCpu");
        assert_eq!(json["gpuTokensPerSecond"], 800.0);
        assert_eq!(
            serde_json::to_value(CpuReason::NoGpu).expect("json")["kind"],
            "noGpu"
        );
        let back: CpuReason = serde_json::from_value(json).expect("read back");
        assert_eq!(back, reason);
    }
}
