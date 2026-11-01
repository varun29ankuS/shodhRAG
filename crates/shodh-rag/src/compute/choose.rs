//! Choosing what runs the search model, and staying on a working target.
//!
//! [`choose`] tries each graphics card the probe offers and then the CPU. A
//! card counts only once it has *run* the model: a provider that builds but
//! fails at its first run (a contrib operator it rejects, a removed device,
//! a Core ML compile failure) is a failed attempt with its reason, and the
//! CPU takes over. A card that runs is kept only when it is clearly faster
//! than the CPU ([`GPU_MARGIN`]). What a saved calibration already knows is
//! used before anything is built: a card known to fail or to be slower is not
//! built again, and a known speed is not measured again.
//!
//! [`run_or_fall_back`] keeps search working when a card fails later, in the
//! middle of a run (a driver reset, a removed device): the CPU takes over and
//! the run goes on.
//!
//! Search is never left without a model: the CPU session is always built,
//! and a CPU whose speed cannot be measured is used unmeasured.

use super::provider::{
    keep_gpu_with_margin, Accelerator, ComputePreference, CpuReason, Device, GpuProbe,
};

/// A card must embed at least this many times the CPU's speed to be kept:
/// timing on a shared machine varies by about a tenth, and a card near
/// parity gains nothing for its memory and start-up.
pub const GPU_MARGIN: f64 = 1.15;

/// Builds and times sessions of the model.
pub trait Candidate {
    type Session;
    /// A session on `device`, or the CPU (`None`).
    fn build(&mut self, device: Option<&Device>) -> Result<Self::Session, String>;
    /// Tokens a second `session` embeds at; an error when it cannot run.
    fn time(&mut self, session: &mut Self::Session) -> Result<f64, String>;
}

/// What a saved calibration knows about a target.
#[derive(Debug, Clone, PartialEq)]
pub enum Known {
    /// The card was kept, at this speed.
    Kept { gpu: f64 },
    /// The card was not kept, for this reason.
    Rejected(CpuReason),
    /// The CPU's speed.
    Cpu { cpu: f64 },
}

/// A measurement worth saving.
#[derive(Debug, Clone, PartialEq)]
pub enum Learned {
    Kept { device: Device, gpu: f64 },
    Rejected { device: Device, reason: CpuReason },
    Cpu { cpu: f64 },
}

/// The session chosen and what runs it.
#[derive(Debug)]
pub struct Chosen<S> {
    pub session: S,
    pub accelerator: Accelerator,
    pub device: Option<Device>,
    pub cpu_reason: Option<CpuReason>,
    /// Its speed; `None` when it could not be measured.
    pub tokens_per_second: Option<f64>,
    /// What was measured now (to save).
    pub learned: Vec<Learned>,
}

/// Chooses the session: each card of `probe` in turn (see the module docs),
/// then the CPU. Fails only when the CPU session cannot be built.
pub fn choose<C: Candidate>(
    candidate: &mut C,
    preference: ComputePreference,
    probe: impl FnOnce() -> GpuProbe,
    known: &dyn Fn(Option<&Device>) -> Option<Known>,
) -> Result<Chosen<C::Session>, String> {
    let mut learned = Vec::new();
    // The CPU session, once built for a comparison, is the one kept.
    let mut cpu: Option<(C::Session, Option<f64>)> = None;
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
                let mut last = CpuReason::NoGpu;
                for device in devices {
                    if let Some(Known::Rejected(reason)) = known(Some(&device)) {
                        last = reason;
                        continue;
                    }
                    let mut session = match candidate.build(Some(&device)) {
                        Ok(session) => session,
                        Err(error) => {
                            let reason = failed(&device, error);
                            learned.push(Learned::Rejected {
                                device: device.clone(),
                                reason: reason.clone(),
                            });
                            last = reason;
                            continue;
                        }
                    };
                    if let Some(Known::Kept { gpu }) = known(Some(&device)) {
                        return Ok(Chosen {
                            session,
                            accelerator: device.accelerator,
                            device: Some(device),
                            cpu_reason: None,
                            tokens_per_second: Some(gpu),
                            learned,
                        });
                    }
                    // Its first run is part of the attempt.
                    let gpu = match candidate.time(&mut session) {
                        Ok(gpu) => gpu,
                        Err(error) => {
                            drop(session);
                            let reason = failed(&device, format!("its first run failed: {error}"));
                            learned.push(Learned::Rejected {
                                device: device.clone(),
                                reason: reason.clone(),
                            });
                            last = reason;
                            continue;
                        }
                    };
                    let cpu_rate = if cpu.is_some() {
                        cpu.as_ref().and_then(|(_, speed)| *speed)
                    } else {
                        match candidate.build(None) {
                            Ok(mut session) => {
                                let speed = cpu_speed(candidate, &mut session, known, &mut learned);
                                cpu = Some((session, speed));
                                speed
                            }
                            // The card runs and the CPU does not: keep the card.
                            Err(_) => None,
                        }
                    };
                    let keep = cpu_rate.is_none_or(|c| keep_gpu_with_margin(gpu, c, GPU_MARGIN));
                    if keep {
                        learned.push(Learned::Kept {
                            device: device.clone(),
                            gpu,
                        });
                        return Ok(Chosen {
                            session,
                            accelerator: device.accelerator,
                            device: Some(device),
                            cpu_reason: None,
                            tokens_per_second: Some(gpu),
                            learned,
                        });
                    }
                    drop(session);
                    let reason = CpuReason::SlowerThanCpu {
                        device: device.name.clone(),
                        gpu_tokens_per_second: gpu,
                        cpu_tokens_per_second: cpu_rate.unwrap_or(0.0),
                    };
                    learned.push(Learned::Rejected {
                        device: device.clone(),
                        reason: reason.clone(),
                    });
                    last = reason;
                }
                last
            }
        },
    };
    let (session, speed) = match cpu {
        Some(built) => built,
        None => {
            let mut session = candidate.build(None)?;
            let speed = cpu_speed(candidate, &mut session, known, &mut learned);
            (session, speed)
        }
    };
    Ok(Chosen {
        session,
        accelerator: Accelerator::Cpu,
        device: None,
        cpu_reason: Some(reason),
        tokens_per_second: speed,
        learned,
    })
}

fn failed(device: &Device, error: String) -> CpuReason {
    tracing::warn!(
        provider = device.accelerator.label(),
        device = %device.name,
        %error,
        "The graphics card could not run the search model"
    );
    CpuReason::Failed {
        device: Some(device.name.clone()),
        error,
    }
}

/// The CPU's speed: known, else measured; `None` when it cannot be measured
/// (the session is used all the same).
fn cpu_speed<C: Candidate>(
    candidate: &mut C,
    session: &mut C::Session,
    known: &dyn Fn(Option<&Device>) -> Option<Known>,
    learned: &mut Vec<Learned>,
) -> Option<f64> {
    if let Some(Known::Cpu { cpu }) = known(None) {
        return Some(cpu);
    }
    match candidate.time(session) {
        Ok(cpu) => {
            learned.push(Learned::Cpu { cpu });
            Some(cpu)
        }
        Err(error) => {
            tracing::warn!(%error, "The search model's speed could not be measured; it is used unmeasured");
            None
        }
    }
}

/// Runs `run` on `session`. When it fails on a graphics card (`on_gpu`),
/// a CPU session from `cpu` takes its place and `run` is tried on it: the
/// returned error text is the card's failure, for the status. A failure on
/// the CPU is the caller's.
pub fn run_or_fall_back<S, R>(
    session: &mut S,
    on_gpu: &mut bool,
    mut run: impl FnMut(&mut S) -> Result<R, String>,
    cpu: impl FnOnce() -> Result<S, String>,
) -> Result<(R, Option<String>), String> {
    match run(session) {
        Ok(value) => Ok((value, None)),
        Err(error) if *on_gpu => {
            tracing::warn!(%error, "The graphics card failed while embedding; the CPU takes over");
            let mut fresh = cpu().map_err(|e| {
                format!("the graphics card failed ({error}) and the CPU could not take over: {e}")
            })?;
            let value = run(&mut fresh)?;
            *session = fresh;
            *on_gpu = false;
            Ok((value, Some(error)))
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A target's scripted outcomes: whether it builds and how its timed
    /// runs go.
    #[derive(Clone)]
    struct Script {
        builds: Result<(), String>,
        times: Result<f64, String>,
    }

    #[derive(Debug, PartialEq)]
    struct Fake(&'static str);

    struct Scripted {
        targets: HashMap<&'static str, Script>,
        built: Vec<&'static str>,
        timed: Vec<&'static str>,
    }

    fn key(device: Option<&Device>) -> &'static str {
        match device.map(|d| d.name.as_str()) {
            Some("RTX 4060") => "RTX 4060",
            Some("Intel UHD") => "Intel UHD",
            _ => "cpu",
        }
    }

    impl Candidate for Scripted {
        type Session = Fake;
        fn build(&mut self, device: Option<&Device>) -> Result<Fake, String> {
            let k = key(device);
            self.built.push(k);
            self.targets[k].builds.clone().map(|()| Fake(k))
        }
        fn time(&mut self, session: &mut Fake) -> Result<f64, String> {
            self.timed.push(session.0);
            self.targets[session.0].times.clone()
        }
    }

    fn card(name: &str, index: i32) -> Device {
        Device {
            accelerator: Accelerator::DirectMl,
            name: name.to_string(),
            index: Some(index),
            driver: Some("31.0.15.5222".to_string()),
        }
    }

    fn scripted(targets: &[(&'static str, Result<(), &str>, Result<f64, &str>)]) -> Scripted {
        Scripted {
            targets: targets
                .iter()
                .map(|(k, b, t)| {
                    (
                        *k,
                        Script {
                            builds: b.clone().map_err(str::to_string),
                            times: t.clone().map_err(str::to_string),
                        },
                    )
                })
                .collect(),
            built: Vec::new(),
            timed: Vec::new(),
        }
    }

    fn one_card() -> GpuProbe {
        GpuProbe::Devices(vec![card("RTX 4060", 0)])
    }

    fn nothing_known(_: Option<&Device>) -> Option<Known> {
        None
    }

    #[test]
    fn a_card_clearly_faster_than_the_cpu_runs_the_model_and_the_cpu_session_goes() {
        let mut c = scripted(&[
            ("RTX 4060", Ok(()), Ok(9000.0)),
            ("cpu", Ok(()), Ok(1500.0)),
        ]);
        let chosen = choose(&mut c, ComputePreference::Auto, one_card, &nothing_known).unwrap();
        assert_eq!(chosen.session, Fake("RTX 4060"));
        assert_eq!(chosen.accelerator, Accelerator::DirectMl);
        assert_eq!(chosen.tokens_per_second, Some(9000.0));
        assert_eq!(chosen.cpu_reason, None);
        assert_eq!(c.built, ["RTX 4060", "cpu"]);
        assert!(chosen.learned.contains(&Learned::Cpu { cpu: 1500.0 }));
        assert!(chosen.learned.contains(&Learned::Kept {
            device: card("RTX 4060", 0),
            gpu: 9000.0
        }));
    }

    #[test]
    fn a_card_that_builds_but_fails_its_first_run_falls_back_to_the_cpu_with_the_reason() {
        let mut c = scripted(&[
            ("RTX 4060", Ok(()), Err("887A0005 device removed")),
            ("cpu", Ok(()), Ok(1500.0)),
        ]);
        let chosen = choose(&mut c, ComputePreference::Auto, one_card, &nothing_known).unwrap();
        assert_eq!(chosen.session, Fake("cpu"));
        assert_eq!(chosen.tokens_per_second, Some(1500.0));
        match &chosen.cpu_reason {
            Some(CpuReason::Failed { device, error }) => {
                assert_eq!(device.as_deref(), Some("RTX 4060"));
                assert!(error.contains("887A0005"), "{error}");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(chosen.learned[0], Learned::Rejected { .. }));
    }

    #[test]
    fn a_card_that_does_not_build_is_reported_and_the_next_card_is_tried() {
        let mut c = scripted(&[
            ("RTX 4060", Err("out of memory"), Ok(0.0)),
            ("Intel UHD", Ok(()), Ok(6000.0)),
            ("cpu", Ok(()), Ok(1500.0)),
        ]);
        let chosen = choose(
            &mut c,
            ComputePreference::Auto,
            || GpuProbe::Devices(vec![card("RTX 4060", 0), card("Intel UHD", 1)]),
            &nothing_known,
        )
        .unwrap();
        assert_eq!(chosen.session, Fake("Intel UHD"));
    }

    #[test]
    fn a_card_near_the_cpus_speed_is_not_kept() {
        // 10% faster is within the noise: not kept.
        let mut c = scripted(&[
            ("RTX 4060", Ok(()), Ok(1650.0)),
            ("cpu", Ok(()), Ok(1500.0)),
        ]);
        let chosen = choose(&mut c, ComputePreference::Auto, one_card, &nothing_known).unwrap();
        assert_eq!(chosen.session, Fake("cpu"));
        assert!(matches!(
            chosen.cpu_reason,
            Some(CpuReason::SlowerThanCpu { .. })
        ));
        // The CPU session built for the comparison is the one kept.
        assert_eq!(c.built, ["RTX 4060", "cpu"]);
        assert_eq!(chosen.tokens_per_second, Some(1500.0));
    }

    #[test]
    fn a_known_verdict_is_used_before_anything_is_built() {
        let rejected = |device: Option<&Device>| match device {
            Some(_) => Some(Known::Rejected(CpuReason::Failed {
                device: Some("RTX 4060".to_string()),
                error: "driver error".to_string(),
            })),
            None => Some(Known::Cpu { cpu: 1400.0 }),
        };
        let mut c = scripted(&[
            ("RTX 4060", Ok(()), Ok(9000.0)),
            ("cpu", Ok(()), Ok(1500.0)),
        ]);
        let chosen = choose(&mut c, ComputePreference::Auto, one_card, &rejected).unwrap();
        assert_eq!(c.built, ["cpu"], "the rejected card is not built");
        assert!(c.timed.is_empty(), "the known CPU speed is not measured");
        assert_eq!(chosen.tokens_per_second, Some(1400.0));
        assert!(chosen.learned.is_empty());

        let kept = |device: Option<&Device>| device.map(|_| Known::Kept { gpu: 8800.0 });
        let mut c = scripted(&[
            ("RTX 4060", Ok(()), Ok(9000.0)),
            ("cpu", Ok(()), Ok(1500.0)),
        ]);
        let chosen = choose(&mut c, ComputePreference::Auto, one_card, &kept).unwrap();
        assert_eq!(c.built, ["RTX 4060"], "no CPU session for a known card");
        assert!(c.timed.is_empty());
        assert_eq!(chosen.tokens_per_second, Some(8800.0));
    }

    #[test]
    fn a_cpu_that_cannot_be_timed_is_used_unmeasured() {
        let mut c = scripted(&[("cpu", Ok(()), Err("timer failed"))]);
        let chosen = choose(
            &mut c,
            ComputePreference::Cpu,
            || panic!("no probe"),
            &nothing_known,
        )
        .unwrap();
        assert_eq!(chosen.session, Fake("cpu"));
        assert_eq!(chosen.tokens_per_second, None);
        assert_eq!(chosen.cpu_reason, Some(CpuReason::Chosen));
    }

    #[test]
    fn only_a_cpu_that_cannot_be_built_is_an_error() {
        let mut c = scripted(&[
            ("RTX 4060", Err("no"), Ok(0.0)),
            ("cpu", Err("no memory"), Ok(0.0)),
        ]);
        let result = choose(&mut c, ComputePreference::Auto, one_card, &nothing_known);
        assert_eq!(result.err(), Some("no memory".to_string()));
    }

    #[test]
    fn a_card_failing_mid_run_hands_over_to_the_cpu_and_the_run_goes_on() {
        let mut session = Fake("RTX 4060");
        let mut on_gpu = true;
        let run = |s: &mut Fake| -> Result<&'static str, String> {
            match s.0 {
                "RTX 4060" => Err("887A0006 device hung".to_string()),
                other => Ok(other),
            }
        };
        let (value, failure) =
            run_or_fall_back(&mut session, &mut on_gpu, run, || Ok(Fake("cpu"))).unwrap();
        assert_eq!(value, "cpu");
        assert_eq!(failure.as_deref(), Some("887A0006 device hung"));
        assert_eq!(session, Fake("cpu"));
        assert!(!on_gpu);
        // On the CPU a failure is the caller's; nothing is swapped.
        let failing = |_: &mut Fake| -> Result<(), String> { Err("bad input".to_string()) };
        let result = run_or_fall_back(&mut session, &mut on_gpu, failing, || {
            panic!("no second CPU")
        });
        assert_eq!(result.err(), Some("bad input".to_string()));
        // The card fails and so does the CPU's build: both are said.
        let mut gpu = Fake("RTX 4060");
        let mut on_gpu = true;
        let result = run_or_fall_back(&mut gpu, &mut on_gpu, run, || Err("no memory".to_string()));
        let error = result.err().unwrap();
        assert!(
            error.contains("887A0006") && error.contains("no memory"),
            "{error}"
        );
    }
}
