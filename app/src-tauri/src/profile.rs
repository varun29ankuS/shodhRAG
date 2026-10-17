//! Which data folder this instance uses.
//!
//! Normally Shodh keeps everything in the platform app data folder. When the
//! `SHODH_DATA_DIR` environment variable names an absolute folder, this
//! instance is a separate profile (a demo or tester profile): its data,
//! WebView storage and credential-store entries all live apart from the
//! normal profile, and it runs beside a normal instance instead of handing
//! over to it. Every app data path goes through [`app_data_dir`]; the
//! variable is read once, by [`init`].

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use sha2::{Digest, Sha256};
use tauri::webview::WebviewWindowBuilder;
use tauri::{Manager, Runtime};

/// Environment variable naming a separate profile's data folder.
pub const DATA_DIR_ENV: &str = "SHODH_DATA_DIR";

/// Credential-store service of the normal profile. Matches the Tauri bundle
/// identifier so the entries are easy to identify in the OS credential manager.
pub const BASE_KEYRING_SERVICE: &str = "com.shodh.rag-app";

/// The profile this instance runs as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// `None`: the normal profile in the platform app data folder.
    data_dir: Option<PathBuf>,
}

static NORMAL: Profile = Profile { data_dir: None };
static ACTIVE: OnceLock<Profile> = OnceLock::new();

impl Profile {
    /// The profile named by the value of [`DATA_DIR_ENV`]. Unset or empty is
    /// the normal profile. A relative path is an error: guessing what it is
    /// relative to could open a folder the user did not mean.
    pub fn from_env_value(value: Option<&OsStr>) -> Result<Self, String> {
        let Some(value) = value.filter(|v| !v.is_empty()) else {
            return Ok(NORMAL.clone());
        };
        let path = Path::new(value);
        if !path.is_absolute() {
            return Err(format!(
                "{DATA_DIR_ENV} must be an absolute folder path, got {}",
                path.display()
            ));
        }
        // Rebuilt from its components so `C:\demo\` and `C:\demo` are one profile.
        Ok(Self {
            data_dir: Some(path.components().collect()),
        })
    }

    /// The separate profile's data folder, or `None` for the normal profile.
    pub fn data_dir(&self) -> Option<&Path> {
        self.data_dir.as_deref()
    }

    /// Whether this is a separate profile rather than the normal one.
    pub fn is_separate(&self) -> bool {
        self.data_dir.is_some()
    }

    /// Only the normal profile is single-instance: a separate profile must be
    /// able to run beside it (the single-instance lock is per app, not per
    /// folder, so it would hand the launch over to the normal instance).
    pub fn uses_single_instance(&self) -> bool {
        !self.is_separate()
    }

    /// The credential-store service holding this profile's secrets. The
    /// normal profile keeps the original name so existing keys stay readable;
    /// a separate profile gets its own, so it never reads the normal keys.
    pub fn keyring_service(&self) -> String {
        match self.data_dir.as_deref() {
            None => BASE_KEYRING_SERVICE.to_string(),
            Some(_) => format!(
                "{BASE_KEYRING_SERVICE}.profile-{}",
                hex::encode(self.folder_id())
            ),
        }
    }

    /// WebView storage folder of a separate profile (`None`: the platform
    /// default of the normal profile).
    pub fn webview_data_dir(&self) -> Option<PathBuf> {
        self.data_dir.as_deref().map(|dir| dir.join("webview"))
    }

    /// A stable 16-byte id of the profile folder.
    fn folder_id(&self) -> [u8; 16] {
        let mut id = [0u8; 16];
        if let Some(dir) = self.data_dir.as_deref() {
            let digest = Sha256::digest(dir.as_os_str().as_encoded_bytes());
            id.copy_from_slice(&digest[..16]);
        }
        id
    }
}

/// Read [`DATA_DIR_ENV`] once and fix this instance's profile. Call before
/// anything resolves a path.
pub fn init() -> Result<&'static Profile, String> {
    let profile = Profile::from_env_value(std::env::var_os(DATA_DIR_ENV).as_deref())?;
    Ok(ACTIVE.get_or_init(|| profile))
}

/// This instance's profile.
pub fn active() -> &'static Profile {
    ACTIVE.get().unwrap_or(&NORMAL)
}

/// The folder all app data lives in: the separate profile's folder, else the
/// platform app data folder.
pub fn app_data_dir<R: Runtime, M: Manager<R>>(app: &M) -> tauri::Result<PathBuf> {
    match active().data_dir() {
        Some(dir) => Ok(dir.to_path_buf()),
        None => app.path().app_data_dir(),
    }
}

/// WebView2 browser flags of every Shodh window (Windows; ignored elsewhere).
/// All windows share one WebView2 environment, so they must all use the same
/// flags; [`webview_defaults`] applies them.
///
/// - wry's defaults, repeated because setting flags replaces them.
/// - `--disable-gpu`: pages are composited in software. The GPU process held
///   ~140 MB at idle (more than the page itself); Shodh draws text, SVG and
///   2D canvases (no WebGL), which software rendering handles.
pub const WEBVIEW_BROWSER_ARGS: &str =
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --disable-gpu";

/// WebView2's own variable for extra browser flags (for example a DevTools
/// port when measuring the app).
pub const WEBVIEW_ARGS_ENV: &str = "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS";

/// The flags passed to WebView2: [`WEBVIEW_BROWSER_ARGS`], then the
/// regional-format flag ([`regional_lang_flag`]) if any, then the value of
/// [`WEBVIEW_ARGS_ENV`], if any (last, so a flag given there wins). Which of
/// the app's flags and the variable WebView2 keeps differs between runtime
/// versions (154 keeps the variable, 153 the app's flags), so the app passes
/// both and every runtime starts the same browser.
pub fn webview_browser_args(env_value: Option<&OsStr>, lang_flag: Option<&str>) -> String {
    let mut args = WEBVIEW_BROWSER_ARGS.to_string();
    if let Some(flag) = lang_flag {
        args.push(' ');
        args.push_str(flag);
    }
    if let Some(extra) = env_value.map(|v| v.to_string_lossy()) {
        if !extra.trim().is_empty() {
            args.push(' ');
            args.push_str(extra.trim());
        }
    }
    args
}

/// This process's WebView2 flags, worked out once.
static BROWSER_ARGS: OnceLock<String> = OnceLock::new();

/// The value in `cell`, computed by `compute` the first time only.
fn once_args(cell: &OnceLock<String>, compute: impl FnOnce() -> String) -> &str {
    cell.get_or_init(compute)
}

/// The flags of every window of this process: [`webview_browser_args`] with
/// the regional-format flag and [`WEBVIEW_ARGS_ENV`], read on the first
/// call. wry creates a WebView2 environment per window on the shared data
/// folder, and WebView2 refuses one whose options differ from the running
/// browser's, so a regional format or variable changed while Shodh runs
/// must not reach a later window (a PDF export's).
fn process_browser_args() -> &'static str {
    once_args(&BROWSER_ARGS, || {
        let lang_flag =
            os_languages().and_then(|(ui, regional)| regional_lang_flag(&ui, &regional));
        webview_browser_args(
            std::env::var_os(WEBVIEW_ARGS_ENV).as_deref(),
            lang_flag.as_deref(),
        )
    })
}

/// Whether `tag` is a plain locale name (`en`, `en-IN`, `zh-Hant-TW`): ASCII
/// letters, digits and hyphens, starting with a letter and not ending with a
/// hyphen. Anything else (an empty name, a Windows sort suffix such as
/// `de-DE_phoneb`, spaces) is never put on the command line.
fn is_locale_name(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 85
        && tag.starts_with(|c: char| c.is_ascii_alphabetic())
        && !tag.ends_with('-')
        && tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// `--lang=<regional format>` when the Windows regional format (dates,
/// numbers) is another region of the display language, e.g. `en-IN` or
/// `en-GB` under an `en-US` display.
///
/// wry sets WebView2's language to the display language
/// (`GetUserDefaultUILanguage`, wry 0.53 `src/webview2/mod.rs`), which the
/// browser gets as `--lang` and uses for `<input type="date">`, so the date
/// fields show `mm/dd/yyyy` to every user with an English display. No flag
/// when the format is in another language (`de-DE` under `en-US`): the flag
/// also sets the language of the context menus and of the
/// `Accept-Language` header, which would then stop following the display
/// language.
pub fn regional_lang_flag(ui_language: &str, regional_format: &str) -> Option<String> {
    let (ui, regional) = (ui_language.trim(), regional_format.trim());
    if !is_locale_name(ui) || !is_locale_name(regional) || ui.eq_ignore_ascii_case(regional) {
        return None;
    }
    let primary = |tag: &str| {
        tag.split('-')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    (primary(ui) == primary(regional)).then(|| format!("--lang={regional}"))
}

/// The user's display language and regional format, as Windows names them.
#[cfg(windows)]
fn os_languages() -> Option<(String, String)> {
    use windows::Win32::Globalization::{
        GetUserDefaultLocaleName, GetUserDefaultUILanguage, LCIDToLocaleName,
        LOCALE_ALLOW_NEUTRAL_NAMES,
    };
    /// `LOCALE_NAME_MAX_LENGTH` (winnls.h): the longest locale name with its NUL.
    const LOCALE_NAME_MAX_LENGTH: usize = 85;
    let mut ui = [0u16; LOCALE_NAME_MAX_LENGTH];
    let mut regional = [0u16; LOCALE_NAME_MAX_LENGTH];
    // SAFETY: the bindings pass each buffer with its length, so the calls
    // never write past it (a name that does not fit makes them return 0).
    // The display language is read the way wry reads it.
    let (ui_len, regional_len) = unsafe {
        let lcid = u32::from(GetUserDefaultUILanguage());
        (
            LCIDToLocaleName(lcid, Some(&mut ui[..]), LOCALE_ALLOW_NEUTRAL_NAMES),
            GetUserDefaultLocaleName(&mut regional[..]),
        )
    };
    // Each call returns the length including the terminating NUL, or 0 on failure.
    let text = |buffer: &[u16], len: i32| -> Option<String> {
        let len = usize::try_from(len)
            .ok()
            .filter(|&n| n > 1 && n <= buffer.len())?;
        Some(String::from_utf16_lossy(&buffer[..len - 1]))
    };
    Some((text(&ui[..], ui_len)?, text(&regional[..], regional_len)?))
}

#[cfg(not(windows))]
fn os_languages() -> Option<(String, String)> {
    None
}

/// Settings every window's WebView gets: [`webview_browser_args`], and for a
/// separate profile, storage inside its folder (Windows, Linux) or its own
/// data store (macOS 14+).
pub fn webview_defaults<'a, R: Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    let builder = builder.additional_browser_args(process_browser_args());
    let profile = active();
    match profile.webview_data_dir() {
        Some(dir) => builder
            .data_directory(dir)
            .data_store_identifier(profile.folder_id()),
        None => builder,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn absolute(name: &str) -> PathBuf {
        std::env::temp_dir().join(name)
    }

    #[test]
    fn unset_or_empty_is_the_normal_profile() {
        for value in [None, Some(OsString::new())] {
            let profile = Profile::from_env_value(value.as_deref()).unwrap();
            assert_eq!(profile.data_dir(), None);
            assert!(!profile.is_separate());
            assert_eq!(profile.webview_data_dir(), None);
        }
    }

    #[test]
    fn absolute_folder_is_a_separate_profile() {
        let dir = absolute("shodh-demo");
        let profile = Profile::from_env_value(Some(dir.as_os_str())).unwrap();
        assert_eq!(profile.data_dir(), Some(dir.as_path()));
        assert_eq!(profile.webview_data_dir(), Some(dir.join("webview")));
    }

    #[test]
    fn relative_folder_is_rejected() {
        let err = Profile::from_env_value(Some(OsStr::new("demo-data"))).unwrap_err();
        assert!(err.contains(DATA_DIR_ENV), "{err}");
    }

    #[test]
    fn trailing_separator_names_the_same_profile() {
        let dir = absolute("shodh-demo");
        let mut with_slash = dir.as_os_str().to_owned();
        with_slash.push(std::path::MAIN_SEPARATOR_STR);
        let a = Profile::from_env_value(Some(dir.as_os_str())).unwrap();
        let b = Profile::from_env_value(Some(&with_slash)).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.keyring_service(), b.keyring_service());
    }

    #[test]
    fn normal_profile_keeps_the_original_vault_service() {
        assert_eq!(NORMAL.keyring_service(), "com.shodh.rag-app");
    }

    #[test]
    fn separate_profiles_get_their_own_vault_service() {
        let a = Profile::from_env_value(Some(absolute("shodh-demo-a").as_os_str())).unwrap();
        let b = Profile::from_env_value(Some(absolute("shodh-demo-b").as_os_str())).unwrap();
        let service_a = a.keyring_service();
        assert!(service_a.starts_with("com.shodh.rag-app.profile-"));
        assert_ne!(service_a, BASE_KEYRING_SERVICE);
        assert_ne!(service_a, b.keyring_service());
        // Stable across runs: the same folder always finds its keys again.
        assert_eq!(service_a, a.clone().keyring_service());
    }

    #[test]
    fn browser_flags_keep_the_apps_and_add_the_variables() {
        assert_eq!(webview_browser_args(None, None), WEBVIEW_BROWSER_ARGS);
        assert_eq!(
            webview_browser_args(Some(OsStr::new("  ")), None),
            WEBVIEW_BROWSER_ARGS
        );
        let args = webview_browser_args(Some(OsStr::new(" --remote-debugging-port=9339 ")), None);
        assert_eq!(
            args,
            format!("{WEBVIEW_BROWSER_ARGS} --remote-debugging-port=9339")
        );
    }

    #[test]
    fn every_window_gets_the_flags_worked_out_first() {
        let cell = OnceLock::new();
        let first = once_args(&cell, || format!("{WEBVIEW_BROWSER_ARGS} --lang=en-IN")).to_string();
        // The regional format changed while the app ran: a later window keeps the first flags.
        let later = once_args(&cell, || format!("{WEBVIEW_BROWSER_ARGS} --lang=en-GB"));
        assert_eq!(later, first);
        assert_eq!(later, format!("{WEBVIEW_BROWSER_ARGS} --lang=en-IN"));
    }

    #[test]
    fn the_regional_flag_sits_between_the_apps_flags_and_the_variables() {
        assert_eq!(
            webview_browser_args(None, Some("--lang=en-IN")),
            format!("{WEBVIEW_BROWSER_ARGS} --lang=en-IN")
        );
        assert_eq!(
            webview_browser_args(
                Some(OsStr::new("--remote-debugging-port=9339")),
                Some("--lang=en-IN")
            ),
            format!("{WEBVIEW_BROWSER_ARGS} --lang=en-IN --remote-debugging-port=9339")
        );
    }

    #[test]
    fn another_region_of_the_display_language_sets_the_date_language() {
        assert_eq!(
            regional_lang_flag("en-US", "en-IN").as_deref(),
            Some("--lang=en-IN")
        );
        assert_eq!(
            regional_lang_flag("en-US", "en-GB").as_deref(),
            Some("--lang=en-GB")
        );
        assert_eq!(
            regional_lang_flag("en", " en-AU ").as_deref(),
            Some("--lang=en-AU")
        );
        assert_eq!(
            regional_lang_flag("zh-Hans-CN", "zh-Hant-TW").as_deref(),
            Some("--lang=zh-Hant-TW")
        );
    }

    #[test]
    fn the_same_locale_or_another_language_adds_no_flag() {
        assert_eq!(regional_lang_flag("en-US", "en-US"), None);
        assert_eq!(regional_lang_flag("en-US", "EN-us"), None);
        // German formats under an English display: the menus would turn German.
        assert_eq!(regional_lang_flag("en-US", "de-DE"), None);
    }

    #[test]
    fn names_that_are_not_plain_locale_names_add_no_flag() {
        for bad in [
            "",
            "  ",
            "de-DE_phoneb",
            "en-IN --disable-gpu",
            "en-",
            "-IN",
            "en\u{0}IN",
            "1n-IN",
        ] {
            assert_eq!(regional_lang_flag("en-US", bad), None, "{bad:?}");
            assert_eq!(regional_lang_flag(bad, "en-IN"), None, "{bad:?}");
        }
    }

    #[test]
    fn only_the_normal_profile_is_single_instance() {
        assert!(NORMAL.uses_single_instance());
        let demo = Profile::from_env_value(Some(absolute("shodh-demo").as_os_str())).unwrap();
        assert!(!demo.uses_single_instance());
    }
}
