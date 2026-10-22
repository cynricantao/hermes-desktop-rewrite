//! Per-device preferences (the settings the original Hermes Desktop keeps in Electron
//! `localStorage` / `userData` rather than in the profile's `config.yaml`).
//!
//! Stored as one flat JSON object in `$XDG_CONFIG_HOME/hermes-desktop-rs/prefs.json`
//! (override with `HERMES_DESKTOP_PREFS=<file>`; tests always do). Keys are the
//! original's storage key names (`hermes.desktop.textDirection`, `hermes-desktop-mode-v1`,
//! …), so the mapping stays one-to-one and documented in docs/PARITY.md. We never read or
//! write the Electron app's own storage.
//!
//! Writes are atomic (temp file + rename) and the file is mode 0600. No secrets go here.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Default location of the prefs file.
pub fn default_path() -> PathBuf {
    if let Some(p) = std::env::var_os("HERMES_DESKTOP_PREFS") {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"));
    base.join("hermes-desktop-rs").join("prefs.json")
}

#[derive(Debug, Clone)]
pub struct Prefs {
    path: PathBuf,
    values: Map<String, Value>,
}

impl Prefs {
    /// Load from `path`; a missing or malformed file yields empty prefs (defaults apply).
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let values = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| match v {
                Value::Object(m) => Some(m),
                _ => None,
            })
            .unwrap_or_default();
        Self { path, values }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    /// String form of a stored value, or `default` when unset.
    pub fn get_str(&self, key: &str, default: &str) -> String {
        match self.values.get(key) {
            None | Some(Value::Null) => default.to_owned(),
            Some(Value::String(s)) => s.clone(),
            Some(Value::Bool(b)) => b.to_string(),
            Some(Value::Number(n)) => n.to_string(),
            Some(v) => v.to_string(),
        }
    }

    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        match self.values.get(key) {
            Some(Value::Bool(b)) => *b,
            Some(Value::String(s)) => matches!(s.as_str(), "true" | "on" | "1"),
            _ => default,
        }
    }

    pub fn all(&self) -> &Map<String, Value> {
        &self.values
    }

    /// Set (or with `Value::Null`, clear) a key and persist.
    pub fn set(&mut self, key: &str, value: Value) -> std::io::Result<()> {
        let _lock = self.write_lock()?;
        // Other windows and widget helpers may have saved since this instance loaded.
        // Never replace their keys, or overwrite a malformed/unreadable file with defaults.
        let mut latest = match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice::<Map<String, Value>>(&bytes)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => self.values.clone(),
            Err(e) => return Err(e),
        };
        if value.is_null() {
            latest.remove(key);
        } else {
            latest.insert(key.to_owned(), value);
        }
        let previous = std::mem::replace(&mut self.values, latest);
        if let Err(error) = self.save() {
            self.values = previous;
            return Err(error);
        }
        Ok(())
    }

    /// Replace every value (import) and persist.
    pub fn replace_all(&mut self, values: Map<String, Value>) -> std::io::Result<()> {
        let _lock = self.write_lock()?;
        let previous = std::mem::replace(&mut self.values, values);
        if let Err(error) = self.save() {
            self.values = previous;
            return Err(error);
        }
        Ok(())
    }

    fn write_lock(&self) -> std::io::Result<std::fs::File> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(self.path.with_extension("json.lock"))?;
        file.lock()?;
        Ok(file)
    }

    fn save(&self) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let body = serde_json::to_vec_pretty(&Value::Object(self.values.clone()))?;
        {
            use std::io::Write;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp)?;
            f.write_all(&body)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &self.path)
    }
}

/// Validated desktop scale presets, matching the original chat text store.
/// Unknown or externally edited values fall back rather than reaching the renderer.
pub fn scale_percent(key: &str, value: &str) -> u16 {
    let fallback = if key == "hermes.desktop.chat-text-scale.v1" {
        110
    } else {
        100
    };
    let number = value.parse::<f64>().ok();
    [90, 100, 110, 125, 150, 175]
        .into_iter()
        .find(|n| number == Some(f64::from(*n)))
        .unwrap_or(fallback)
}

/// Defaults for device prefs, matching the original's store defaults.
pub fn default_for(key: &str) -> &'static str {
    match key {
        "hermes.desktop.intro-splash.v1" => "true",
        "hermes.desktop.model-pricing.v1" => "false",
        "hermes.desktop.tips.rotation.v1" => "true",
        "hermes.desktop.tours.v1" => "true",
        "hermes-desktop-mode-v1" => "system",
        "hermes-desktop-theme-v2" => "nous",
        "zoom" => "100",
        "hermes.desktop.chat-text-scale.v1" => "110",
        "hermes.desktop.interfaceMode.v1" => "advanced",
        "hermes.desktop.sessionListDensity" => "compact",
        "hermes.desktop.tabStripDefault" => "auto",
        "hermes.desktop.titlebarAppActions" => "right",
        "minimize-to-tray" => "false",
        "hermes.desktop.backdrop.v1" => "false",
        "hermes.desktop.fileBrowser" => "true",
        "hermes.desktop.composerPopout.gesturesEnabled" => "true",
        "hermes.desktop.user-bubble-transparency.v1" => "0",
        "hermes.desktop.textDirection" => "auto",
        "hermes.desktop.hideThreadTimeline" => "false",
        "hermes.desktop.reactions.v1" => "false",
        "hermes.desktop.vibeHearts.v1" => "true",
        "hermes.desktop.toolView.technical" => "false",
        "hermes.desktop.toolView.hideCodeDiffs" => "false",
        "hermes.desktop.reasoning.collapsedByDefault" => "false",
        "hermes.desktop.embed-mode" => "ask",
        "data-url-read-max" => "16",
        "hermes.desktop.keepAwakeMode.v1" => "off",
        "hermes.desktop.disableF12.v1" => "false",
        "hermes.desktop.completionSoundVariantId" => "1",
        "hud-modifier" | "screenshot" => "false",
        k if k.starts_with("hermes:native-notifications.") => "true",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hermes-prefs-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("prefs.json")
    }

    #[test]
    fn stale_instances_preserve_other_saved_settings_and_refuse_corrupt_files() {
        let path = tmp("merge");
        let mut first = Prefs::load(&path);
        let mut second = Prefs::load(&path);
        first.set("chat-size", json!(110)).unwrap();
        second.set("palette", json!("ember")).unwrap();
        first.set("chat-size", json!(125)).unwrap();
        let saved = Prefs::load(&path);
        assert_eq!(saved.get("palette"), Some(&json!("ember")));
        assert_eq!(saved.get("chat-size"), Some(&json!(125)));
        std::fs::write(&path, "not valid JSON").unwrap();
        assert!(first.set("chat-size", json!(150)).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not valid JSON");
        assert_eq!(first.get("chat-size"), Some(&json!(125)));
    }

    #[test]
    fn failed_writes_do_not_change_the_effective_preferences() {
        let path = tmp("failed-write");
        let mut prefs = Prefs::load(&path);
        prefs.set("present", json!("old")).unwrap();
        let before = prefs.all().clone();
        let blocked = path.parent().unwrap().join("not-a-directory");
        std::fs::write(&blocked, "fixture").unwrap();
        prefs.path = blocked.join("prefs.json");
        assert!(prefs.set("present", json!("new")).is_err());
        assert_eq!(prefs.all(), &before);
        assert!(prefs.set("present", Value::Null).is_err());
        assert_eq!(prefs.all(), &before);
        assert!(prefs.set("missing", json!(true)).is_err());
        assert_eq!(prefs.all(), &before);
        assert!(prefs.replace_all(Map::new()).is_err());
        assert_eq!(prefs.all(), &before);
        assert_eq!(Prefs::load(&path).all(), &before);
    }

    #[test]
    fn round_trips_through_disk() {
        let p = tmp("rt");
        let mut a = Prefs::load(&p);
        assert_eq!(a.get_str("hermes.desktop.textDirection", "auto"), "auto");
        a.set("hermes.desktop.textDirection", json!("rtl")).unwrap();
        a.set("hermes.desktop.backdrop.v1", json!(true)).unwrap();
        let b = Prefs::load(&p);
        assert_eq!(b.get_str("hermes.desktop.textDirection", "auto"), "rtl");
        assert!(b.get_bool("hermes.desktop.backdrop.v1", false));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let mut c = Prefs::load(&p);
        c.set("hermes.desktop.textDirection", Value::Null).unwrap();
        assert_eq!(Prefs::load(&p).get("hermes.desktop.textDirection"), None);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn malformed_file_means_defaults() {
        let p = tmp("bad");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"not json").unwrap();
        assert!(Prefs::load(&p).all().is_empty());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn every_device_setting_has_a_default_or_is_text() {
        use crate::settings::{Control, SETTINGS, Store};
        for s in SETTINGS {
            if let Store::Device(k) = s.store
                && matches!(s.control, Control::Toggle | Control::Segmented(_))
            {
                assert!(!default_for(k).is_empty(), "no default for device pref {k}");
            }
        }
    }
}
