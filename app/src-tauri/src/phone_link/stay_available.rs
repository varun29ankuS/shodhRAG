//! "Stay available for my phone" (Settings → Phone): while it is on, a phone
//! is paired, Local-only mode is off and the computer runs on mains power,
//! Shodh keeps the computer from going to sleep, so the phone can reach it.
//! On battery the computer sleeps as usual: the setting never drains a
//! laptop away from its charger. Off by default.
//!
//! The decision is [`wanted`], applied by a [`Keeper`] over a power source and
//! a wake hold the tests replace. The operating system's hold belongs to the
//! thread that takes it (Windows' `SetThreadExecutionState`), so one
//! dedicated thread owns it ([`StayAvailable::start`]): it applies every
//! change it is sent, and looks at the power source again each minute,
//! because unplugging the charger sends no event this thread could receive.
//!
//! Windows and macOS keep the computer awake; elsewhere the setting says it
//! is not available.

use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The setting's file in the app data folder.
pub const STAY_FILE: &str = "phone_stay_available.json";

/// Sent to the window when the setting or what it does changed.
pub const STAY_EVENT: &str = "phone_stay_available_changed";

/// How often the power source is looked at again.
const RECHECK: Duration = Duration::from_secs(60);

/// Whether the computer runs on mains power: `None` when it cannot tell.
pub trait PowerSource: Send + 'static {
    fn on_ac(&self) -> Option<bool>;
}

/// The operating system's hold that keeps the computer awake.
pub trait Wakefulness: Send + 'static {
    /// Take the hold; whether the system took it.
    fn hold(&mut self) -> bool;
    fn release(&mut self);
}

/// What decides whether the computer is kept awake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Conditions {
    /// The user's setting.
    pub enabled: bool,
    /// Local-only mode: no phone may reach this computer.
    pub local_only: bool,
    /// A phone is paired.
    pub paired: bool,
}

/// Whether to keep the computer awake: only when the user turned it on, a
/// phone could connect, and the computer is known to be on mains power.
pub fn wanted(conditions: Conditions, on_ac: Option<bool>) -> bool {
    conditions.enabled && !conditions.local_only && conditions.paired && on_ac == Some(true)
}

/// The setting and what it does now, as Settings → Phone shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StayStatus {
    pub enabled: bool,
    /// This system can be kept awake by Shodh.
    pub supported: bool,
    pub on_ac: Option<bool>,
    /// The computer is being kept awake now.
    pub holding: bool,
    pub local_only: bool,
    pub paired: bool,
}

/// Applies [`wanted`] to a wake hold.
pub struct Keeper<P: PowerSource, W: Wakefulness> {
    power: P,
    awake: W,
    holding: bool,
    supported: bool,
}

impl<P: PowerSource, W: Wakefulness> Keeper<P, W> {
    pub fn new(power: P, awake: W, supported: bool) -> Self {
        Self {
            power,
            awake,
            holding: false,
            supported,
        }
    }

    /// Hold or release for `conditions` and the power source now.
    pub fn apply(&mut self, conditions: Conditions) -> StayStatus {
        let on_ac = self.power.on_ac();
        let want = self.supported && wanted(conditions, on_ac);
        if want && !self.holding {
            self.holding = self.awake.hold();
            if !self.holding {
                tracing::warn!(target: "shodh::link", "the computer could not be kept awake for the phone");
            }
        } else if !want && self.holding {
            self.awake.release();
            self.holding = false;
        }
        StayStatus {
            enabled: conditions.enabled,
            supported: self.supported,
            on_ac,
            holding: self.holding,
            local_only: conditions.local_only,
            paired: conditions.paired,
        }
    }

    /// Let the computer sleep again (the app is closing).
    pub fn release(&mut self) {
        if self.holding {
            self.awake.release();
            self.holding = false;
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    enabled: bool,
}

/// The saved setting; off when it was never set or cannot be read.
pub fn load_enabled(data_dir: &Path) -> bool {
    std::fs::read(data_dir.join(STAY_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Saved>(&bytes).ok())
        .is_some_and(|saved| saved.enabled)
}

pub fn save_enabled(data_dir: &Path, enabled: bool) -> Result<(), String> {
    let bytes = serde_json::to_vec(&Saved { enabled }).map_err(|e| e.to_string())?;
    crate::durable_file::write_atomic(&data_dir.join(STAY_FILE), &bytes)
        .map_err(|e| format!("The setting could not be saved: {e}"))
}

/// How long a change waits for the thread to apply it.
const APPLY_WAIT: Duration = Duration::from_secs(5);

enum Message {
    /// Apply these conditions, and say what was done.
    Apply(Conditions, mpsc::Sender<StayStatus>),
    Quit,
}

/// The thread that owns the wake hold, and what it last did.
pub struct StayAvailable {
    sender: Mutex<mpsc::Sender<Message>>,
    status: Arc<Mutex<StayStatus>>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl StayAvailable {
    /// Start the thread with the operating system's power source and hold.
    pub fn start() -> Self {
        Self::start_with(
            platform::OsPower,
            platform::OsAwake::default(),
            platform::SUPPORTED,
        )
    }

    pub fn start_with<P: PowerSource, W: Wakefulness>(power: P, awake: W, supported: bool) -> Self {
        let (sender, receiver) = mpsc::channel::<Message>();
        let status = Arc::new(Mutex::new(StayStatus {
            enabled: false,
            supported,
            on_ac: None,
            holding: false,
            local_only: false,
            paired: false,
        }));
        let shown = status.clone();
        let spawned = std::thread::Builder::new()
            .name("shodh-stay-available".into())
            .spawn(move || {
                let mut keeper = Keeper::new(power, awake, supported);
                let mut conditions = Conditions::default();
                loop {
                    let mut reply = None;
                    match receiver.recv_timeout(RECHECK) {
                        Ok(Message::Apply(next, to)) => {
                            conditions = next;
                            reply = Some(to);
                        }
                        Ok(Message::Quit) | Err(RecvTimeoutError::Disconnected) => {
                            keeper.release();
                            shown.lock().unwrap_or_else(|e| e.into_inner()).holding = false;
                            return;
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                    let now = keeper.apply(conditions);
                    *shown.lock().unwrap_or_else(|e| e.into_inner()) = now.clone();
                    if let Some(reply) = reply {
                        let _ = reply.send(now);
                    }
                }
            });
        let thread = match spawned {
            Ok(thread) => Some(thread),
            Err(e) => {
                tracing::warn!(target: "shodh::link", error = %e, "\"Stay available for my phone\" could not start");
                None
            }
        };
        Self {
            sender: Mutex::new(sender),
            status,
            thread: Mutex::new(thread),
        }
    }

    /// Apply `conditions` now (the thread also rechecks each minute) and
    /// return what was done. Blocking: call it off the async runtime.
    pub fn apply(&self, conditions: Conditions) -> StayStatus {
        let (reply, done) = mpsc::channel();
        let sent = {
            let sender = self.sender.lock().unwrap_or_else(|e| e.into_inner());
            sender.send(Message::Apply(conditions, reply)).is_ok()
        };
        if !sent {
            tracing::warn!(target: "shodh::link", "\"Stay available for my phone\" is not running");
            return self.status();
        }
        done.recv_timeout(APPLY_WAIT)
            .unwrap_or_else(|_| self.status())
    }

    /// What it did last.
    pub fn status(&self) -> StayStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Let the computer sleep again and stop the thread; returns once it has.
    pub fn stop(&self) {
        {
            let sender = self.sender.lock().unwrap_or_else(|e| e.into_inner());
            let _ = sender.send(Message::Quit);
        }
        let thread = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(thread) = thread {
            if thread.join().is_err() {
                tracing::warn!(target: "shodh::link", "\"Stay available for my phone\" stopped with a panic");
            }
        }
    }
}

#[cfg(windows)]
mod platform {
    use windows::Win32::System::Power::{
        GetSystemPowerStatus, SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
        SYSTEM_POWER_STATUS,
    };

    pub const SUPPORTED: bool = true;

    pub struct OsPower;

    impl super::PowerSource for OsPower {
        fn on_ac(&self) -> Option<bool> {
            let mut status = SYSTEM_POWER_STATUS::default();
            // SAFETY: `status` is a valid SYSTEM_POWER_STATUS for the call to fill.
            unsafe { GetSystemPowerStatus(&mut status) }.ok()?;
            match status.ACLineStatus {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            }
        }
    }

    /// `SetThreadExecutionState` on the thread that owns the hold.
    #[derive(Default)]
    pub struct OsAwake;

    impl super::Wakefulness for OsAwake {
        fn hold(&mut self) -> bool {
            // SAFETY: plain flags; the state belongs to this thread until released.
            let previous = unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
            previous.0 != 0
        }

        fn release(&mut self) {
            // SAFETY: plain flags; clears this thread's requirement.
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::{c_char, c_void, CStr, CString};

    type CFTypeRef = *const c_void;
    type CFStringRef = *const c_void;
    type IOPMAssertionID = u32;

    const UTF8: u32 = 0x0800_0100;
    const ASSERTION_LEVEL_ON: u32 = 255;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            text: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFStringGetCString(
            text: CFStringRef,
            buffer: *mut c_char,
            size: isize,
            encoding: u32,
        ) -> u8;
        fn CFRelease(value: CFTypeRef);
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(
            kind: CFStringRef,
            level: u32,
            name: CFStringRef,
            id: *mut IOPMAssertionID,
        ) -> i32;
        fn IOPMAssertionRelease(id: IOPMAssertionID) -> i32;
        fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
        fn IOPSGetProvidingPowerSourceType(blob: CFTypeRef) -> CFStringRef;
    }

    pub const SUPPORTED: bool = true;

    /// A CFString made from `text`, released when dropped.
    struct OwnedString(CFStringRef);

    impl OwnedString {
        fn new(text: &str) -> Option<Self> {
            let text = CString::new(text).ok()?;
            // SAFETY: a NUL-terminated UTF-8 string and the default allocator.
            let made = unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8) };
            (!made.is_null()).then_some(Self(made))
        }
    }

    impl Drop for OwnedString {
        fn drop(&mut self) {
            // SAFETY: created by CFStringCreateWithCString, released once.
            unsafe { CFRelease(self.0) }
        }
    }

    pub struct OsPower;

    impl super::PowerSource for OsPower {
        fn on_ac(&self) -> Option<bool> {
            // SAFETY: the blob is owned (Copy rule) and released below; the
            // type string follows the Get rule and is read before that.
            unsafe {
                let blob = IOPSCopyPowerSourcesInfo();
                if blob.is_null() {
                    return None;
                }
                let kind = IOPSGetProvidingPowerSourceType(blob);
                let mut buffer = [0 as c_char; 64];
                let read = !kind.is_null()
                    && CFStringGetCString(kind, buffer.as_mut_ptr(), buffer.len() as isize, UTF8)
                        != 0;
                CFRelease(blob);
                if !read {
                    return None;
                }
                match CStr::from_ptr(buffer.as_ptr()).to_str().ok()? {
                    "AC Power" => Some(true),
                    "Battery Power" | "UPS Power" => Some(false),
                    _ => None,
                }
            }
        }
    }

    /// An IOKit power assertion that prevents idle system sleep.
    #[derive(Default)]
    pub struct OsAwake {
        id: Option<IOPMAssertionID>,
    }

    impl super::Wakefulness for OsAwake {
        fn hold(&mut self) -> bool {
            if self.id.is_some() {
                return true;
            }
            let (Some(kind), Some(reason)) = (
                OwnedString::new("PreventUserIdleSystemSleep"),
                OwnedString::new("Shodh: stay available for your phone"),
            ) else {
                return false;
            };
            let mut id: IOPMAssertionID = 0;
            // SAFETY: valid CFStrings and an out-pointer to a local.
            let result = unsafe {
                IOPMAssertionCreateWithName(kind.0, ASSERTION_LEVEL_ON, reason.0, &mut id)
            };
            if result == 0 {
                self.id = Some(id);
                true
            } else {
                false
            }
        }

        fn release(&mut self) {
            if let Some(id) = self.id.take() {
                // SAFETY: an assertion this hold created, released once.
                unsafe { IOPMAssertionRelease(id) };
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    pub const SUPPORTED: bool = false;

    pub struct OsPower;

    impl super::PowerSource for OsPower {
        fn on_ac(&self) -> Option<bool> {
            None
        }
    }

    #[derive(Default)]
    pub struct OsAwake;

    impl super::Wakefulness for OsAwake {
        fn hold(&mut self) -> bool {
            false
        }

        fn release(&mut self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

    /// 0: battery, 1: mains, 2: unknown.
    #[derive(Clone, Default)]
    struct FakePower(Arc<AtomicU8>);

    impl PowerSource for FakePower {
        fn on_ac(&self) -> Option<bool> {
            match self.0.load(Ordering::SeqCst) {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            }
        }
    }

    #[derive(Clone, Default)]
    struct FakeAwake {
        holds: Arc<AtomicUsize>,
        releases: Arc<AtomicUsize>,
    }

    impl Wakefulness for FakeAwake {
        fn hold(&mut self) -> bool {
            self.holds.fetch_add(1, Ordering::SeqCst);
            true
        }
        fn release(&mut self) {
            self.releases.fetch_add(1, Ordering::SeqCst);
        }
    }

    const ON: Conditions = Conditions {
        enabled: true,
        local_only: false,
        paired: true,
    };

    #[test]
    fn stay_available_only_on_ac_power_and_when_enabled() {
        let power = FakePower::default();
        let awake = FakeAwake::default();
        let mut keeper = Keeper::new(power.clone(), awake.clone(), true);

        // On battery: never held, even when enabled.
        power.0.store(0, Ordering::SeqCst);
        assert!(!keeper.apply(ON).holding);
        // Plugged in: held, once.
        power.0.store(1, Ordering::SeqCst);
        assert!(keeper.apply(ON).holding);
        assert!(keeper.apply(ON).holding);
        assert_eq!(awake.holds.load(Ordering::SeqCst), 1);
        // Unplugged: released.
        power.0.store(0, Ordering::SeqCst);
        let status = keeper.apply(ON);
        assert!(!status.holding);
        assert_eq!(status.on_ac, Some(false));
        assert_eq!(awake.releases.load(Ordering::SeqCst), 1);
        // Unknown power source: not held.
        power.0.store(2, Ordering::SeqCst);
        assert!(!keeper.apply(ON).holding);

        power.0.store(1, Ordering::SeqCst);
        // Turned off by the user, Local-only mode, or no phone paired: released.
        for off in [
            Conditions {
                enabled: false,
                ..ON
            },
            Conditions {
                local_only: true,
                ..ON
            },
            Conditions {
                paired: false,
                ..ON
            },
        ] {
            assert!(keeper.apply(ON).holding);
            assert!(!keeper.apply(off).holding, "{off:?}");
        }
        assert_eq!(awake.holds.load(Ordering::SeqCst), 4);
        assert_eq!(awake.releases.load(Ordering::SeqCst), 4);
        // Closing the app lets the computer sleep.
        assert!(keeper.apply(ON).holding);
        keeper.release();
        assert_eq!(awake.releases.load(Ordering::SeqCst), 5);
    }

    #[test]
    fn a_system_shodh_cannot_keep_awake_never_holds() {
        let power = FakePower(Arc::new(AtomicU8::new(1)));
        let awake = FakeAwake::default();
        let mut keeper = Keeper::new(power, awake.clone(), false);
        let status = keeper.apply(ON);
        assert!(!status.holding && !status.supported);
        assert_eq!(awake.holds.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_thread_applies_what_it_is_sent_and_releases_when_stopped() {
        let power = FakePower(Arc::new(AtomicU8::new(1)));
        let awake = FakeAwake::default();
        let stay = StayAvailable::start_with(power, awake.clone(), true);
        assert!(stay.apply(ON).holding);
        // Returns once the thread has applied ON (a hold) and then quit (a release).
        stay.stop();
        assert_eq!(awake.holds.load(Ordering::SeqCst), 1);
        assert_eq!(awake.releases.load(Ordering::SeqCst), 1);
        assert!(!stay.status().holding);
    }

    #[test]
    fn the_setting_is_off_until_saved() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!load_enabled(dir.path()));
        save_enabled(dir.path(), true).unwrap();
        assert!(load_enabled(dir.path()));
        std::fs::write(dir.path().join(STAY_FILE), b"not json").unwrap();
        assert!(!load_enabled(dir.path()), "an unreadable setting is off");
    }
}
