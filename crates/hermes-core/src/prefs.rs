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
        if value.is_null() {
            self.values.remove(key);
        } else {
            self.values.insert(key.to_owned(), value);
        }
        self.save()
    }

    /// Replace every value (import) and persist.
    pub fn replace_all(&mut self, values: Map<String, Value>) -> std::io::Result<()> {
        self.values = values;
        self.save()
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

/// Defaults for device prefs, matching the original's store defaults.
pub fn default_for(key: &str) -> &'static str {
    match key {
        "hermes.desktop.intro-splash.v1" => "true",
        "hermes.desktop.model-pricing.v1" => "false",
        "hermes.desktop.tips.rotation.v1" => "true",
        "hermes.desktop.tours.v1" => "true",
        "hermes-desktop-mode-v1" => "system",
        "hermes-desktop-theme-v2" => "nous",
        "zoom" | "hermes.desktop.chat-text-scale.v1" => "100",
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
