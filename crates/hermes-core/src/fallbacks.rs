//! UI-free fallback-chain drafts. Only complete new rows are saved; routing fields
//! remain opaque. Stable keys prevent a delayed edit from targeting a different row.
use serde_json::{Value, json};

/// Pure front-end action encoding; all network writes still use SettingsState.action.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub verb: String,
    pub context: String,
    pub row: String,
    pub value: String,
    pub revision: i32,
}

pub fn encode_action(verb: &str, context: &str, row: &str, value: &str, revision: i32) -> String {
    let action = Action {
        verb: verb.into(),
        context: context.into(),
        row: row.into(),
        value: value.into(),
        revision,
    };
    format!(
        "models:fallback:{}",
        serde_json::to_string(&action).expect("string action serializes")
    )
}

pub fn decode_action(value: &str) -> Result<Action, String> {
    serde_json::from_str(value).map_err(|_| "Invalid fallback action; no change was saved.".into())
}

#[derive(Clone)]
pub struct Row {
    pub key: String,
    entry: Value,
    // Do not silently delete malformed pre-existing data when another row is edited.
    untouched_incomplete: Option<Value>,
}

impl Row {
    pub fn provider(&self) -> &str {
        self.entry["provider"].as_str().unwrap_or_default()
    }

    pub fn model(&self) -> &str {
        self.entry["model"].as_str().unwrap_or_default()
    }

    fn complete(&self) -> bool {
        !self.provider().is_empty() && !self.model().is_empty()
    }
}

#[derive(Clone)]
pub struct Draft {
    epoch: u64,
    next_key: u64,
    pub rows: Vec<Row>,
    saved: Vec<Value>,
}

/// Legacy strings are accepted without splitting a model id at its later slashes.
/// Objects retain every field. Other stored values stay opaque until explicitly edited.
pub fn entries(value: &Value) -> Vec<Value> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| match entry {
            Value::String(text) => match text.split_once('/') {
                Some((provider, model)) if !provider.is_empty() => json!({"provider": provider, "model": model}),
                _ => json!({"provider": "", "model": text}),
            },
            other => other.clone(),
        })
        .collect()
}

impl Draft {
    /// The caller supplies a fresh epoch on profile entry or genuine external change.
    pub fn new(epoch: u64, value: &Value) -> Self {
        let saved = entries(value);
        let mut draft = Self {
            epoch,
            next_key: 0,
            rows: Vec::new(),
            saved: saved.clone(),
        };
        for entry in saved {
            let key = draft.key();
            let display = if entry.is_object() {
                entry.clone()
            } else {
                json!({"provider":"", "model":""})
            };
            let mut row = Row {
                key,
                entry: display,
                untouched_incomplete: None,
            };
            if !row.complete() {
                row.untouched_incomplete = Some(entry);
            }
            draft.rows.push(row);
        }
        draft
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    fn key(&mut self) -> String {
        let key = format!("{}-{}", self.epoch, self.next_key);
        self.next_key += 1;
        key
    }

    pub fn add(&mut self) -> String {
        let key = self.key();
        self.rows.push(Row {
            key: key.clone(),
            entry: json!({"provider":"", "model":""}),
            untouched_incomplete: None,
        });
        key
    }

    fn row_mut(&mut self, key: &str) -> Result<&mut Row, String> {
        self.rows
            .iter_mut()
            .find(|row| row.key == key)
            .ok_or_else(|| "That fallback row changed; reopen the page.".into())
    }

    pub fn set_provider(&mut self, key: &str, provider: &str) -> Result<bool, String> {
        if provider.chars().any(char::is_control) {
            return Err("The provider identifier contains a control character.".into());
        }
        let row = self.row_mut(key)?;
        if row.provider() == provider {
            return Ok(false);
        }
        row.entry["provider"] = json!(provider);
        row.entry["model"] = json!("");
        row.untouched_incomplete = None;
        Ok(true)
    }

    pub fn set_model(&mut self, key: &str, model: &str) -> Result<bool, String> {
        if model.chars().any(char::is_control) {
            return Err("The model identifier contains a control character.".into());
        }
        let row = self.row_mut(key)?;
        if row.model() == model {
            return Ok(false);
        }
        row.entry["model"] = json!(model);
        row.untouched_incomplete = None;
        Ok(true)
    }

    pub fn remove(&mut self, key: &str) -> Result<(), String> {
        let index = self
            .rows
            .iter()
            .position(|row| row.key == key)
            .ok_or("That fallback row changed; reopen the page.")?;
        self.rows.remove(index);
        Ok(())
    }

    pub fn move_by(&mut self, key: &str, delta: i32) -> Result<bool, String> {
        let index = self
            .rows
            .iter()
            .position(|row| row.key == key)
            .ok_or("That fallback row changed; reopen the page.")?;
        let next = index as i64 + i64::from(delta);
        if next < 0 || next >= self.rows.len() as i64 {
            return Ok(false);
        }
        self.rows.swap(index, next as usize);
        Ok(true)
    }

    pub fn snapshot(&self) -> Vec<Value> {
        self.rows
            .iter()
            .filter_map(|row| {
                if row.complete() {
                    Some(row.entry.clone())
                } else {
                    row.untouched_incomplete.clone()
                }
            })
            .collect()
    }

    pub fn needs_save(&self) -> bool {
        self.snapshot() != self.saved
    }

    /// An autosave echo advances the baseline without destroying blank or partial rows.
    pub fn acknowledge(&mut self, expected: &[Value], actual: &Value) -> Result<(), String> {
        let actual = entries(actual);
        if actual != expected {
            return Err("The backend did not save this fallback chain. Refresh before retrying.".into());
        }
        self.saved = actual;
        Ok(())
    }

    pub fn sync_external(&mut self, epoch: u64, actual: &Value) -> bool {
        if entries(actual) == self.saved {
            return false;
        }
        *self = Self::new(epoch, actual);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> Value {
        json!([
            {"provider":"custom:local", "model":"org/model:v1", "base_url":"http://127.0.0.1:8081/v1", "api_key":"[REDACTED]", "key_env":"FIXTURE_KEY", "api_mode":"chat_completions"},
            {"provider":"other", "model":"backup"}
        ])
    }

    #[test]
    fn actions_keep_colons_slashes_and_quotes_unambiguous() {
        let encoded = encode_action("model", "work:7", "7-1", "org/model:v2\"quoted", 4);
        let action = decode_action(encoded.strip_prefix("models:fallback:").unwrap()).unwrap();
        assert_eq!(action.value, "org/model:v2\"quoted");
        assert_eq!(action.context, "work:7");
        assert_eq!(action.row, "7-1");
        assert!(decode_action(r#"{"verb":"flush","extra":true}"#).is_err());
    }

    #[test]
    fn blank_and_partial_rows_stay_local_across_an_autosave_echo() {
        let initial = chain();
        let mut draft = Draft::new(1, &initial);
        let row = draft.add();
        assert!(!draft.needs_save());
        draft.set_provider(&row, "custom:local").unwrap();
        assert_eq!(draft.snapshot(), initial.as_array().unwrap().clone());
        let expected = draft.snapshot();
        draft.acknowledge(&expected, &initial).unwrap();
        assert!(!draft.sync_external(2, &initial));
        assert_eq!(draft.rows.len(), 3);
        assert_eq!(draft.rows.last().unwrap().provider(), "custom:local");
        assert!(draft.rows.last().unwrap().model().is_empty());
    }

    #[test]
    fn provider_changes_clear_only_the_model_and_keep_routing_fields() {
        let initial = chain();
        let mut draft = Draft::new(1, &initial);
        let key = draft.rows[0].key.clone();
        assert!(!draft.set_provider(&key, "custom:local").unwrap());
        assert_eq!(draft.rows[0].model(), "org/model:v1");
        draft.set_provider(&key, "custom:second").unwrap();
        assert!(draft.rows[0].model().is_empty());
        draft.set_model(&key, "family/new-model:v2").unwrap();
        let saved = draft.snapshot();
        assert_eq!(saved[0]["base_url"], initial[0]["base_url"]);
        assert_eq!(saved[0]["api_key"], initial[0]["api_key"]);
        assert_eq!(saved[0]["api_mode"], initial[0]["api_mode"]);
        assert_eq!(saved[0]["key_env"], initial[0]["key_env"]);
        assert_eq!(saved[0]["provider"], "custom:second");
        assert_eq!(saved[0]["model"], "family/new-model:v2");
    }

    #[test]
    fn removed_or_previous_epoch_keys_cannot_redirect_to_another_row() {
        let mut draft = Draft::new(1, &chain());
        let old = draft.rows[0].key.clone();
        draft.remove(&old).unwrap();
        assert!(draft.set_model(&old, "wrong").is_err());
        assert_eq!(draft.rows[0].model(), "backup");
        assert!(draft.sync_external(2, &json!([{"provider":"other","model":"external"}])));
        assert!(draft.set_model(&old, "wrong").is_err());
    }

    #[test]
    fn failed_readback_keeps_dirty_state_and_does_not_discard_newer_edits() {
        let initial = chain();
        let mut draft = Draft::new(1, &initial);
        let row = draft.rows[1].key.clone();
        draft.set_model(&row, "new").unwrap();
        let sent = draft.snapshot();
        assert!(draft.acknowledge(&sent, &initial).is_err());
        assert!(draft.needs_save());
        draft.set_model(&row, "newer").unwrap();
        draft.acknowledge(&sent, &Value::Array(sent.clone())).unwrap();
        assert!(draft.needs_save());
        assert_eq!(draft.rows[1].model(), "newer");
    }

    #[test]
    fn removing_and_reordering_preserve_other_entries_and_opaque_data() {
        let initial = chain();
        let mut draft = Draft::new(1, &initial);
        let first = draft.rows[0].key.clone();
        let second = draft.rows[1].key.clone();
        assert!(draft.move_by(&first, 1).unwrap());
        assert_eq!(draft.snapshot()[1], initial[0]);
        draft.remove(&second).unwrap();
        assert_eq!(draft.snapshot(), vec![initial[0].clone()]);
    }

    #[test]
    fn legacy_ids_and_unedited_malformed_data_are_not_silently_lost() {
        let initial =
            json!(["custom:local/org/model:v1", {"provider":"incomplete", "model":"", "key_env":"KEEP"}, null]);
        let mut draft = Draft::new(4, &initial);
        assert_eq!(draft.rows[0].provider(), "custom:local");
        assert_eq!(draft.rows[0].model(), "org/model:v1");
        draft.add();
        assert!(!draft.needs_save());
        assert_eq!(draft.snapshot()[1], initial[1]);
        assert_eq!(draft.snapshot()[2], Value::Null);
    }
}
