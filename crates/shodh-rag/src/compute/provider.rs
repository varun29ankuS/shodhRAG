//! Which processor runs the search model: a graphics card through the
//! platform's GPU provider (DirectML on Windows, Core ML on macOS), or the
//! CPU, with the reason when it is the CPU.
//!
//! Every GPU attempt is registered with `error_on_failure`, so a provider
//! that does not start is an error here, never a silent fallback inside ONNX
//! Runtime: the reason is kept and shown. A GPU that starts is kept only if
//! it is faster than the CPU in the calibration ([`keep_gpu`]): a provider
//! that runs few of the model's operators, or a weak integrated GPU, can be
//! slower than the processor it was meant to relieve.

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

/// A session and what it runs on.
#[derive(Debug)]
pub struct Selection<S> {
    pub session: S,
    pub accelerator: Accelerator,
    /// The graphics card, when one runs the session.
    pub device: Option<Device>,
    /// Why the CPU runs the session, when it does.
    pub cpu_reason: Option<CpuReason>,
}

/// Builds a session on the best target: each card of `probe` in turn, then
/// the CPU. `build(Some(device))` builds on a card and must fail when its
/// provider does not register; `build(None)` builds on the CPU. Fails only
/// when the CPU session fails too.
pub fn select<S>(
    preference: ComputePreference,
    probe: impl FnOnce() -> GpuProbe,
    mut build: impl FnMut(Option<&Device>) -> Result<S, String>,
) -> Result<Selection<S>, String> {
    let reason = match preference {
        ComputePreference::Cpu => CpuReason::Chosen,
        ComputePreference::Auto => match probe() {
            GpuProbe::Unsupported => CpuReason::NoProvider,
            GpuProbe::Failed(error) => CpuReason::Failed {
                device: None,
                error,
            },
            GpuProbe::Devices(devices) if devices.is_empty() => CpuReason::NoGpu,
            GpuProbe::Devices(devices) => {
                let mut last = None;
                for device in devices {
                    match build(Some(&device)) {
                        Ok(session) => {
                            return Ok(Selection {
                                session,
                                accelerator: device.accelerator,
                                device: Some(device),
                                cpu_reason: None,
                            })
                        }
                        Err(error) => {
                            tracing::warn!(
                                provider = device.accelerator.label(),
                                device = %device.name,
                                %error,
                                "The graphics card could not run the search model"
                            );
                            last = Some(CpuReason::Failed {
                                device: Some(device.name.clone()),
                                error,
                            });
                        }
                    }
                }
                last.unwrap_or(CpuReason::NoGpu)
            }
        },
    };
    let session = build(None)?;
    Ok(Selection {
        session,
        accelerator: Accelerator::Cpu,
        device: None,
        cpu_reason: Some(reason),
    })
}

/// Whether a graphics card that embeds at `gpu` tokens a second is kept
/// over the CPU at `cpu`: only when it is faster.
pub fn keep_gpu(gpu: f64, cpu: f64) -> bool {
    gpu > cpu
}

/// The graphics cards of this machine the platform's provider can use.
pub fn probe_gpus() -> GpuProbe {
    platform::probe()
}

#[cfg(windows)]
mod platform {
    use super::{Accelerator, Device, GpuProbe};
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_NOT_FOUND,
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

    /// Every Apple silicon Mac has a GPU and a Neural Engine Core ML can use.
    pub fn probe() -> GpuProbe {
        GpuProbe::Devices(vec![Device {
            accelerator: Accelerator::CoreMl,
            name: "Apple silicon GPU and Neural Engine".to_string(),
            index: None,
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

    fn card(name: &str, index: i32) -> Device {
        Device {
            accelerator: Accelerator::DirectMl,
            name: name.to_string(),
            index: Some(index),
        }
    }

    /// A session label: which target `build` was called for.
    fn label(device: Option<&Device>) -> String {
        device.map_or("cpu".to_string(), |d| d.name.clone())
    }

    #[test]
    fn a_working_gpu_runs_the_model() {
        let mut tried = Vec::new();
        let selection = select(
            ComputePreference::Auto,
            || GpuProbe::Devices(vec![card("RTX 4060", 0)]),
            |device| {
                tried.push(label(device));
                Ok(label(device))
            },
        )
        .expect("selected");
        assert_eq!(selection.session, "RTX 4060");
        assert_eq!(selection.accelerator, Accelerator::DirectMl);
        assert_eq!(selection.cpu_reason, None);
        assert_eq!(tried, ["RTX 4060"], "the CPU session is not built");
    }

    #[test]
    fn a_gpu_that_fails_falls_back_to_the_cpu_with_its_error() {
        let selection = select(
            ComputePreference::Auto,
            || GpuProbe::Devices(vec![card("Radeon 780M", 0)]),
            |device| match device {
                Some(_) => Err("DirectML: device removed (887A0005)".to_string()),
                None => Ok("cpu"),
            },
        )
        .expect("selected");
        assert_eq!(selection.session, "cpu");
        assert_eq!(selection.accelerator, Accelerator::Cpu);
        assert_eq!(
            selection.cpu_reason,
            Some(CpuReason::Failed {
                device: Some("Radeon 780M".to_string()),
                error: "DirectML: device removed (887A0005)".to_string(),
            })
        );
    }

    #[test]
    fn the_next_card_is_tried_before_the_cpu() {
        let selection = select(
            ComputePreference::Auto,
            || GpuProbe::Devices(vec![card("RTX 4060", 0), card("Intel UHD", 1)]),
            |device| match device.map(|d| d.index) {
                Some(Some(0)) => Err("out of memory".to_string()),
                _ => Ok(label(device)),
            },
        )
        .expect("selected");
        assert_eq!(selection.session, "Intel UHD");
        assert_eq!(selection.device.map(|d| d.index), Some(Some(1)));
    }

    #[test]
    fn the_cpu_setting_never_tries_a_gpu() {
        let selection = select(
            ComputePreference::Cpu,
            || panic!("no card is looked for"),
            |device| {
                assert!(device.is_none(), "only the CPU is built");
                Ok("cpu")
            },
        )
        .expect("selected");
        assert_eq!(selection.cpu_reason, Some(CpuReason::Chosen));
    }

    #[test]
    fn no_card_and_no_provider_are_reported_apart() {
        let no_card = select(
            ComputePreference::Auto,
            || GpuProbe::Devices(Vec::new()),
            |_| Ok("cpu"),
        )
        .expect("selected");
        assert_eq!(no_card.cpu_reason, Some(CpuReason::NoGpu));
        let no_provider = select(
            ComputePreference::Auto,
            || GpuProbe::Unsupported,
            |_| Ok("cpu"),
        )
        .expect("selected");
        assert_eq!(no_provider.cpu_reason, Some(CpuReason::NoProvider));
        let unlisted = select(
            ComputePreference::Auto,
            || GpuProbe::Failed("DXGI unavailable".to_string()),
            |_| Ok("cpu"),
        )
        .expect("selected");
        assert_eq!(
            unlisted.cpu_reason,
            Some(CpuReason::Failed {
                device: None,
                error: "DXGI unavailable".to_string()
            })
        );
    }

    #[test]
    fn a_cpu_that_fails_too_is_an_error() {
        let result = select(
            ComputePreference::Auto,
            || GpuProbe::Devices(vec![card("RTX 4060", 0)]),
            |device| -> Result<(), String> { Err(format!("{} failed", label(device))) },
        );
        assert_eq!(result.err(), Some("cpu failed".to_string()));
    }

    #[test]
    fn a_gpu_is_kept_only_when_faster_than_the_cpu() {
        assert!(keep_gpu(9000.0, 1500.0));
        assert!(!keep_gpu(1200.0, 1500.0));
        assert!(!keep_gpu(1500.0, 1500.0));
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
    }

    /// This machine's probe never offers a software adapter: CI's runners
    /// have only the Basic Render Driver, which must not count as a GPU.
    #[test]
    fn the_probe_lists_no_software_adapter() {
        if let GpuProbe::Devices(devices) = probe_gpus() {
            for device in devices {
                assert!(device.accelerator.is_gpu());
                assert!(
                    !device.name.contains("Basic Render"),
                    "{} is a software adapter",
                    device.name
                );
            }
        }
    }
}
