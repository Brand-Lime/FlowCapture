//! Checks, without opening the browsers, whether the FlowCapture Companion extension is
//! installed and enabled in the browsers on this computer (Chrome, Edge, Brave, Opera, Firefox).
//! Without it FlowCapture cannot tell what was clicked in web pages, so recording is refused
//! (see `required`) and the user is asked to contact IT.
//!
//! Chromium browsers list their extensions in the `Preferences` / `Secure Preferences` files of
//! each profile (`extensions.settings`, with the folder of each extension); Firefox in the
//! `extensions.json` of each profile. The extension is recognised by its manifest name (Chromium:
//! the id changes between stores and unpacked installs) or by its fixed add-on id (Firefox).
//! Only browsers that were opened at least once (that have a profile) are checked.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// `name` of browser-extension/manifest.json.
pub const EXTENSION_NAME: &str = "FlowCapture Companion";
/// Firefox add-on id: must match FIREFOX_EXTENSION_ID in scripts/build-browser-extensions.mjs.
pub const FIREFOX_EXTENSION_ID: &str = "companion@flowcapture.app";
/// Setting: "false" lets users record without the extension (changed in admin mode).
pub const REQUIRED_SETTING: &str = "browser_extension_required";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionState {
    Missing,
    Disabled,
    Active,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowserExtensionInfo {
    pub id: &'static str,
    pub name: &'static str,
    /// The browser was opened at least once on this account (it has a profile).
    pub used: bool,
    pub extension: ExtensionState,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowserExtensionStatus {
    /// The check is available on this system (Windows); elsewhere the user is only told once.
    pub supported: bool,
    /// Recording needs the extension in every used browser.
    pub required: bool,
    pub browsers: Vec<BrowserExtensionInfo>,
    /// Used browsers where the extension is missing or disabled.
    pub missing: Vec<String>,
}

pub fn extension_required(db: &crate::storage::Database) -> bool {
    db.get_setting(REQUIRED_SETTING).ok().flatten().as_deref() != Some("false")
}

pub fn check(required: bool) -> BrowserExtensionStatus {
    let browsers = detect_browsers();
    let missing = browsers
        .iter()
        .filter(|browser| browser.used && browser.extension != ExtensionState::Active)
        .map(|browser| browser.name.to_string())
        .collect();
    BrowserExtensionStatus { supported: cfg!(windows), required, browsers, missing }
}

#[derive(Clone, Copy)]
enum Engine {
    Chromium,
    Firefox,
}

struct BrowserDef {
    id: &'static str,
    name: &'static str,
    engine: Engine,
    /// Executables: the browser counts as installed when one exists.
    executables: Vec<PathBuf>,
    /// `App Paths` registry name of the executable (other install locations).
    app_path: &'static str,
    /// Chromium: the "User Data" folder; Firefox: the folder holding profiles.ini.
    data_dir: Option<PathBuf>,
}

#[cfg(windows)]
fn browser_defs() -> Vec<BrowserDef> {
    let env = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let local = env("LOCALAPPDATA");
    let roaming = env("APPDATA");
    let program_dirs: Vec<PathBuf> = [env("ProgramFiles"), env("ProgramFiles(x86)"), env("ProgramW6432")]
        .into_iter()
        .flatten()
        .collect();
    let in_programs = |relative: &str| -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = program_dirs.iter().map(|dir| dir.join(relative)).collect();
        if let Some(local) = &local {
            paths.push(local.join(relative));
            paths.push(local.join("Programs").join(relative));
        }
        paths
    };

    vec![
        BrowserDef {
            id: "chrome",
            name: "Google Chrome",
            engine: Engine::Chromium,
            executables: in_programs(r"Google\Chrome\Application\chrome.exe"),
            app_path: "chrome.exe",
            data_dir: local.as_ref().map(|dir| dir.join(r"Google\Chrome\User Data")),
        },
        BrowserDef {
            id: "edge",
            name: "Microsoft Edge",
            engine: Engine::Chromium,
            executables: in_programs(r"Microsoft\Edge\Application\msedge.exe"),
            app_path: "msedge.exe",
            data_dir: local.as_ref().map(|dir| dir.join(r"Microsoft\Edge\User Data")),
        },
        BrowserDef {
            id: "brave",
            name: "Brave",
            engine: Engine::Chromium,
            executables: in_programs(r"BraveSoftware\Brave-Browser\Application\brave.exe"),
            app_path: "brave.exe",
            data_dir: local.as_ref().map(|dir| dir.join(r"BraveSoftware\Brave-Browser\User Data")),
        },
        BrowserDef {
            id: "opera",
            name: "Opera",
            engine: Engine::Chromium,
            executables: [in_programs(r"Opera\opera.exe"), in_programs(r"Opera\launcher.exe")].concat(),
            app_path: "opera.exe",
            data_dir: roaming.as_ref().map(|dir| dir.join(r"Opera Software\Opera Stable")),
        },
        // BrowserDef {
        //     id: "firefox",
        //     name: "Mozilla Firefox",
        //     engine: Engine::Firefox,
        //     executables: in_programs(r"Mozilla Firefox\firefox.exe"),
        //     app_path: "firefox.exe",
        //     data_dir: roaming.as_ref().map(|dir| dir.join(r"Mozilla\Firefox")),
        // },
    ]
}

#[cfg(not(windows))]
fn browser_defs() -> Vec<BrowserDef> {
    Vec::new()
}

fn detect_browsers() -> Vec<BrowserExtensionInfo> {
    browser_defs()
        .into_iter()
        .filter(|def| def.executables.iter().any(|exe| exe.is_file()) || registered_app_path(def.app_path))
        .map(|def| {
            let profiles = match (&def.data_dir, def.engine) {
                (Some(dir), Engine::Chromium) => chromium_profiles(dir),
                (Some(dir), Engine::Firefox) => firefox_profiles(dir),
                (None, _) => Vec::new(),
            };
            let extension = profiles
                .iter()
                .map(|profile| match def.engine {
                    Engine::Chromium => chromium_extension_state(profile),
                    Engine::Firefox => firefox_extension_state(profile),
                })
                .max()
                .unwrap_or(ExtensionState::Missing);
            BrowserExtensionInfo { id: def.id, name: def.name, used: !profiles.is_empty(), extension }
        })
        .collect()
}

/// Browser installed somewhere else, registered in `App Paths` (HKCU or HKLM).
#[cfg(windows)]
fn registered_app_path(exe: &str) -> bool {
    ["HKCU", "HKLM"].iter().any(|hive| {
        let key = format!(r"{hive}\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{exe}");
        crate::process_util::background_command("reg")
            .args(["query", &key, "/ve"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| {
                let text = String::from_utf8_lossy(&output.stdout).to_string();
                let value = text.split("REG_SZ").nth(1)?.trim().trim_matches('"').to_string();
                Some(Path::new(&value).is_file())
            })
            .unwrap_or(false)
    })
}

#[cfg(not(windows))]
fn registered_app_path(_exe: &str) -> bool {
    false
}

/// Profile folders of a Chromium "User Data" folder (`Default`, `Profile 1`…). Opera keeps its
/// only profile in the data folder itself.
fn chromium_profiles(user_data: &Path) -> Vec<PathBuf> {
    if user_data.join("Preferences").is_file() {
        return vec![user_data.to_path_buf()];
    }
    let Ok(entries) = std::fs::read_dir(user_data) else {
        return Vec::new();
    };
    let mut profiles: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name != "System Profile" && name != "Guest Profile" && path.join("Preferences").is_file()
        })
        .collect();
    profiles.sort();
    profiles
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

fn chromium_extension_state(profile: &Path) -> ExtensionState {
    let mut best = ExtensionState::Missing;
    for file in ["Secure Preferences", "Preferences"] {
        let Some(prefs) = read_json(&profile.join(file)) else {
            continue;
        };
        let Some(settings) = prefs.pointer("/extensions/settings").and_then(|v| v.as_object()) else {
            continue;
        };
        for entry in settings.values() {
            if !is_flowcapture_extension(profile, entry) {
                continue;
            }
            let state = if chromium_entry_disabled(entry) { ExtensionState::Disabled } else { ExtensionState::Active };
            best = best.max(state);
        }
    }
    best
}

fn is_flowcapture_extension(profile: &Path, entry: &serde_json::Value) -> bool {
    // Older versions keep the manifest in the preferences.
    if entry.pointer("/manifest/name").and_then(|v| v.as_str()) == Some(EXTENSION_NAME) {
        return true;
    }
    let Some(path) = entry.get("path").and_then(|v| v.as_str()).filter(|p| !p.is_empty()) else {
        return false;
    };
    // Unpacked extensions have an absolute path, store/policy ones `<id>\<version>` under Extensions.
    let folder = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        profile.join("Extensions").join(path)
    };
    read_json(&folder.join("manifest.json"))
        .and_then(|manifest| manifest.get("name").and_then(|v| v.as_str()).map(|name| name == EXTENSION_NAME))
        .unwrap_or(false)
}

fn chromium_entry_disabled(entry: &serde_json::Value) -> bool {
    match entry.get("disable_reasons") {
        Some(serde_json::Value::Number(reasons)) => reasons.as_u64().unwrap_or(0) != 0,
        Some(serde_json::Value::Array(reasons)) => !reasons.is_empty(),
        _ => entry.get("state").and_then(|v| v.as_i64()) == Some(0),
    }
}

/// Profile folders listed in Firefox's profiles.ini (relative to it or absolute).
fn firefox_profiles(firefox_dir: &Path) -> Vec<PathBuf> {
    let Ok(ini) = std::fs::read_to_string(firefox_dir.join("profiles.ini")) else {
        return Vec::new();
    };
    let mut profiles = Vec::new();
    let mut path: Option<String> = None;
    let mut relative = true;
    let mut flush = |path: &mut Option<String>, relative: bool| {
        if let Some(value) = path.take() {
            let folder = if relative {
                value.split('/').fold(firefox_dir.to_path_buf(), |folder, part| folder.join(part))
            } else {
                PathBuf::from(value)
            };
            if folder.is_dir() {
                profiles.push(folder);
            }
        }
    };
    for line in ini.lines().map(str::trim) {
        if line.starts_with('[') {
            flush(&mut path, relative);
            relative = true;
        } else if let Some(value) = line.strip_prefix("Path=") {
            path = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("IsRelative=") {
            relative = value != "0";
        }
    }
    flush(&mut path, relative);
    profiles
}

fn firefox_extension_state(profile: &Path) -> ExtensionState {
    let Some(data) = read_json(&profile.join("extensions.json")) else {
        return ExtensionState::Missing;
    };
    let addon = data
        .get("addons")
        .and_then(|v| v.as_array())
        .and_then(|addons| addons.iter().find(|a| a.get("id").and_then(|v| v.as_str()) == Some(FIREFOX_EXTENSION_ID)));
    match addon {
        Some(addon) if addon.get("active").and_then(|v| v.as_bool()) == Some(true) => ExtensionState::Active,
        Some(_) => ExtensionState::Disabled,
        None => ExtensionState::Missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flowcapture-ext-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn finds_the_extension_in_chromium_profiles() {
        let user_data = temp_dir("chromium");
        let unpacked = temp_dir("unpacked");
        write(&unpacked.join("manifest.json"), "\u{feff}{\"name\": \"FlowCapture Companion\"}");

        // Default: installed from a store, enabled.
        let default = user_data.join("Default");
        write(&default.join("Extensions/abc/1.0.1_0/manifest.json"), r#"{"name": "FlowCapture Companion"}"#);
        write(&default.join("Extensions/zzz/2.0_0/manifest.json"), r#"{"name": "Other"}"#);
        write(
            &default.join("Secure Preferences"),
            r#"{"extensions":{"settings":{"zzz":{"path":"zzz\\2.0_0"},"abc":{"path":"abc\\1.0.1_0","disable_reasons":0}}}}"#,
        );
        write(&default.join("Preferences"), "{}");

        // Profile 1: unpacked but disabled. Profile 2: nothing.
        let profile1 = user_data.join("Profile 1");
        let unpacked_path = serde_json::to_string(&unpacked.to_string_lossy()).unwrap();
        write(
            &profile1.join("Preferences"),
            &format!(r#"{{"extensions":{{"settings":{{"def":{{"path":{unpacked_path},"disable_reasons":[1]}}}}}}}}"#),
        );
        let profile2 = user_data.join("Profile 2");
        write(&profile2.join("Preferences"), r#"{"extensions":{"settings":{}}}"#);
        write(&user_data.join("System Profile/Preferences"), "{}");

        let profiles = chromium_profiles(&user_data);
        assert_eq!(profiles, vec![default.clone(), profile1.clone(), profile2.clone()]);
        assert_eq!(chromium_extension_state(&default), ExtensionState::Active);
        assert_eq!(chromium_extension_state(&profile1), ExtensionState::Disabled);
        assert_eq!(chromium_extension_state(&profile2), ExtensionState::Missing);

        // Opera: the profile is the data folder itself.
        let opera = temp_dir("opera");
        write(&opera.join("Preferences"), "{}");
        assert_eq!(chromium_profiles(&opera), vec![opera.clone()]);

        for dir in [user_data, unpacked, opera] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// Diagnostics: `cargo test --lib browser_extensions -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn prints_the_status_of_this_computer() {
        println!("{:#?}", check(true));
    }

    #[test]
    fn finds_the_extension_in_firefox_profiles() {
        let firefox = temp_dir("firefox");
        let external = temp_dir("external-profile");
        write(
            &firefox.join("profiles.ini"),
            &format!(
                "[General]\nStartWithLastProfile=1\n\n[Profile0]\nName=default\nIsRelative=1\nPath=Profiles/abc.default\n\n[Profile1]\nName=work\nIsRelative=0\nPath={}\n",
                external.display()
            ),
        );
        let relative = firefox.join("Profiles").join("abc.default");
        write(
            &relative.join("extensions.json"),
            r#"{"addons":[{"id":"other@x","active":true},{"id":"companion@flowcapture.app","active":true}]}"#,
        );
        write(&external.join("extensions.json"), r#"{"addons":[{"id":"companion@flowcapture.app","active":false}]}"#);

        let profiles = firefox_profiles(&firefox);
        assert_eq!(profiles.len(), 2);
        assert_eq!(firefox_extension_state(&profiles[0]), ExtensionState::Active);
        assert_eq!(firefox_extension_state(&profiles[1]), ExtensionState::Disabled);

        let _ = std::fs::remove_dir_all(firefox);
        let _ = std::fs::remove_dir_all(external);
    }
}
