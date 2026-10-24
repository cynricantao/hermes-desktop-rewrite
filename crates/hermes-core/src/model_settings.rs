//! Pure Main model settings contracts: staged assignment and profile-default controls.
//! Mirrors the original desktop's model-settings.tsx, not the current chat's overrides.
use serde_json::{Value, json};

pub const REASONING_VALUES: &[&str] = crate::projects::REASONING_LEVELS;

/// Resolve catalog identities without splitting provider ids such as `custom:local`.
pub fn provider<'a>(options: &'a Value, name: &str) -> Option<&'a Value> {
    let rows = options.get("providers")?.as_array()?;
    rows.iter().find(|row| row["slug"].as_str() == Some(name)).or_else(|| {
        let matches = |row: &&Value| {
            row["aliases"]
                .as_array()
                .is_some_and(|aliases| aliases.iter().any(|alias| alias.as_str() == Some(name)))
        };
        rows.iter()
            .filter(matches)
            .find(|row| row["is_current"].as_bool() == Some(true))
            .or_else(|| rows.iter().find(matches))
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelDraft {
    pub provider: String,
    pub model: String,
}

impl ModelDraft {
    pub fn applied(options: &Value) -> Self {
        let raw = options["provider"].as_str().unwrap_or_default();
        Self {
            provider: provider(options, raw)
                .and_then(|row| row["slug"].as_str())
                .unwrap_or(raw)
                .to_owned(),
            model: options["model"].as_str().unwrap_or_default().to_owned(),
        }
    }

    /// A provider switch clears its old model. Selecting the same provider preserves edits.
    pub fn select_provider(&mut self, options: &Value, name: &str) -> Result<(), String> {
        let row = provider(options, name).ok_or("Choose a configured provider.")?;
        let name = row["slug"].as_str().ok_or("This provider has no identifier.")?;
        if self.provider != name {
            self.provider = name.to_owned();
            self.model.clear();
        }
        Ok(())
    }

    /// Keep exact model ids, including slashes and colons; the backend validates availability.
    pub fn assignment(&self, options: &Value) -> Result<Value, String> {
        if self.model.trim().is_empty() || self.model.chars().any(char::is_control) {
            return Err("Choose a model before applying.".into());
        }
        let row = provider(options, &self.provider).ok_or("Choose a configured provider.")?;
        let mut body = json!({"scope": "main", "provider": self.provider, "model": self.model});
        if let Some(url) = row["api_url"].as_str().filter(|url| !url.is_empty()) {
            body["base_url"] = json!(url);
        }
        Ok(body)
    }

    pub fn matches(&self, options: &Value) -> bool {
        Self::applied(options) == *self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MainDefaults {
    pub reasoning: bool,
    pub fast: bool,
    pub ultrafast: bool,
    pub effort: String,
    pub speed: String,
}

impl MainDefaults {
    /// Controls follow the applied model, not the model currently being drafted.
    pub fn read(options: &Value, config: &Value) -> Self {
        let applied = ModelDraft::applied(options);
        let caps = provider(options, &applied.provider)
            .and_then(|row| row.get("capabilities").and_then(|caps| caps.get(&applied.model)));
        let supported = |key: &str, default: bool| caps.and_then(|caps| caps[key].as_bool()).unwrap_or(default);
        let raw = config.pointer("/agent/reasoning_effort");
        let effort = match raw {
            Some(Value::Bool(false)) => "none".to_owned(),
            Some(Value::String(value))
                if matches!(value.trim().to_lowercase().as_str(), "false" | "disabled" | "off") =>
            {
                "none".into()
            }
            Some(Value::String(value)) if !value.trim().is_empty() => value.trim().to_lowercase(),
            _ => "medium".into(),
        };
        let speed = match config
            .pointer("/agent/service_tier")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "fast" | "priority" => "fast",
            "ultrafast" => "ultrafast",
            _ => "normal",
        }
        .to_owned();
        Self {
            reasoning: !applied.model.is_empty() && supported("reasoning", true),
            fast: supported("fast", false),
            ultrafast: supported("ultrafast", false),
            effort,
            speed,
        }
    }

    pub fn validate(&self, key: &str, value: &str) -> Result<Value, String> {
        let valid = match key {
            "agent.reasoning_effort" => self.reasoning && REASONING_VALUES.contains(&value),
            "agent.service_tier" => match value {
                "normal" => self.fast || self.ultrafast,
                "fast" => self.fast,
                "ultrafast" => self.ultrafast,
                _ => false,
            },
            _ => false,
        };
        if valid {
            Ok(json!(value))
        } else {
            Err("This model does not offer that default; no change was saved.".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> Value {
        json!({"provider":"custom:one", "model":"family/model:v1", "providers":[
            {"slug":"custom:one", "api_url":"http://127.0.0.1:8081/v1", "models":["family/model:v1"],
             "capabilities":{"family/model:v1":{"reasoning":true,"fast":true,"ultrafast":false}}},
            {"slug":"other", "models":["plain"], "capabilities":{"plain":{"reasoning":false,"ultrafast":true}}}
        ]})
    }
    #[test]
    fn switching_provider_stages_without_carrying_the_old_model() {
        let options = catalog();
        let mut draft = ModelDraft::applied(&options);
        draft.select_provider(&options, "custom:one").unwrap();
        assert_eq!(draft.model, "family/model:v1");
        draft.select_provider(&options, "other").unwrap();
        assert!(draft.model.is_empty());
        assert!(draft.assignment(&options).is_err());
        assert_eq!(options["provider"], "custom:one");
        assert!(draft.select_provider(&options, "missing").is_err());
        assert_eq!(draft.provider, "other");
    }
    #[test]
    fn assignments_keep_exact_ids_and_custom_endpoint() {
        let options = catalog();
        let draft = ModelDraft::applied(&options);
        assert_eq!(
            draft.assignment(&options).unwrap(),
            json!({"scope":"main", "provider":"custom:one", "model":"family/model:v1", "base_url":"http://127.0.0.1:8081/v1"})
        );
        assert!(draft.matches(&options));
        let mut wrong = options;
        wrong["model"] = json!("different");
        assert!(!draft.matches(&wrong));
    }
    #[test]
    fn defaults_follow_applied_capabilities_and_normalize_legacy_values() {
        let defaults = MainDefaults::read(
            &catalog(),
            &json!({"agent":{"reasoning_effort":false,"service_tier":"priority"}}),
        );
        assert_eq!(defaults.effort, "none");
        assert_eq!(defaults.speed, "fast");
        assert!(defaults.validate("agent.reasoning_effort", "ultra").is_ok());
        assert!(defaults.validate("agent.service_tier", "fast").is_ok());
        assert!(defaults.validate("agent.service_tier", "ultrafast").is_err());
        assert!(defaults.validate("agent.reasoning_effort", "invented").is_err());
    }
    #[test]
    fn unreported_speed_capabilities_fail_closed() {
        let defaults = MainDefaults::read(&json!({"provider":"unknown","model":"m"}), &json!({}));
        assert!(defaults.reasoning);
        assert_eq!(defaults.effort, "medium");
        assert!(!defaults.fast && !defaults.ultrafast);
        assert!(defaults.validate("agent.service_tier", "fast").is_err());
    }
}
