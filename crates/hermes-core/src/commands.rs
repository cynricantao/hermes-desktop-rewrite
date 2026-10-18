//! UI-free slash helpers and a per-chat FIFO. Queue entries keep expanded skill
//! text separate from the short invocation displayed in the transcript.
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, PartialEq)]
pub struct PendingPrompt {
    pub text: String,
    pub display: String,
}

#[derive(Default)]
pub struct PromptQueues(HashMap<String, VecDeque<PendingPrompt>>);

impl PromptQueues {
    pub fn entries(&self, session: &str) -> Vec<PendingPrompt> {
        self.0
            .get(session)
            .map(|q| q.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn push(&mut self, session: &str, prompt: PendingPrompt) {
        self.0.entry(session.into()).or_default().push_back(prompt);
    }

    pub fn pop(&mut self, session: &str) -> Option<PendingPrompt> {
        self.0.get_mut(session)?.pop_front()
    }

    /// Apply the CLI's one-based management operations. Parse and validate before
    /// mutation so invalid indices never consume a queued prompt.
    pub fn manage(&mut self, session: &str, arg: &str) -> Result<String, String> {
        let (verb, rest) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
        let rest = rest.trim();
        let usage = "Usage: /queue [list|add <prompt>|edit N <prompt>|rm N|move A B|clear]";
        if verb.is_empty() || verb == "list" {
            let entries = self.entries(session);
            return Ok(if entries.is_empty() {
                "Queue is empty.".into()
            } else {
                entries
                    .iter()
                    .enumerate()
                    .map(|(i, p)| format!("{}. {}", i + 1, p.display))
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        if verb == "clear" && rest.is_empty() {
            self.0.remove(session);
            return Ok("Queue cleared.".into());
        }
        let q = self.0.entry(session.into()).or_default();
        let index = |s: &str| {
            s.parse::<usize>()
                .ok()
                .filter(|n| *n > 0 && *n <= q.len())
                .map(|n| n - 1)
                .ok_or_else(|| "Invalid queue position.".to_owned())
        };
        match verb {
            "rm" => {
                let i = index(rest)?;
                q.remove(i);
            }
            "edit" => {
                let (n, text) = rest.split_once(char::is_whitespace).ok_or(usage)?;
                let i = index(n)?;
                let text = text.trim();
                if text.is_empty() {
                    return Err(usage.into());
                }
                q[i] = PendingPrompt {
                    text: text.into(),
                    display: text.into(),
                };
            }
            "move" => {
                let parts: Vec<_> = rest.split_whitespace().collect();
                if parts.len() != 2 {
                    return Err(usage.into());
                }
                let (a, b) = (index(parts[0])?, index(parts[1])?);
                let p = q.remove(a).expect("validated index");
                q.insert(b, p);
            }
            _ => return Err(usage.into()),
        }
        Ok("Queue updated.".into())
    }
}

pub fn parse(text: &str) -> (String, &str) {
    let text = text.trim().trim_start_matches('/');
    let (name, arg) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    (name.to_lowercase(), arg.trim())
}

pub fn alias(target: &str, arg: &str) -> Result<String, String> {
    let target = target.trim().trim_start_matches('/');
    if target.is_empty() || target.contains(['\n', '\r']) {
        return Err("Invalid command alias target.".into());
    }
    Ok(format!(
        "/{target}{}",
        if arg.is_empty() {
            String::new()
        } else {
            format!(" {arg}")
        }
    ))
}

/// Gateway replace_from is a Python character offset, not a UTF-8 byte offset.
pub fn complete(draft: &str, from: Option<i64>, text: &str) -> String {
    let from = from.and_then(|n| usize::try_from(n).ok()).unwrap_or(1);
    let prefix: String = draft.chars().take(from).collect();
    format!("{prefix}{text} ")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn prompt(text: &str) -> PendingPrompt {
        PendingPrompt {
            text: text.into(),
            display: text.into(),
        }
    }
    #[test]
    fn fifo_is_scoped_to_runtime() {
        let mut q = PromptQueues::default();
        q.push("a", prompt("one"));
        q.push("b", prompt("other"));
        q.push("a", prompt("two"));
        assert_eq!(q.pop("a"), Some(prompt("one")));
        assert_eq!(q.pop("a"), Some(prompt("two")));
        assert_eq!(q.pop("a"), None);
        assert_eq!(q.pop("b"), Some(prompt("other")));
    }
    #[test]
    fn queue_management_and_invalid_positions_preserve_entries() {
        let mut q = PromptQueues::default();
        for p in ["one", "two", "three"] {
            q.push("s", prompt(p));
        }
        assert!(q.manage("s", "move 0 2").is_err());
        assert!(q.manage("s", "edit 4 nope").is_err());
        assert_eq!(q.entries("s").len(), 3);
        q.manage("s", "move 3 1").unwrap();
        q.manage("s", "edit 2 changed prompt").unwrap();
        q.manage("s", "rm 3").unwrap();
        assert_eq!(q.manage("s", "list").unwrap(), "1. three\n2. changed prompt");
        q.manage("s", "clear").unwrap();
        assert!(q.entries("s").is_empty());
    }
    #[test]
    fn aliases_and_unicode_completions() {
        assert_eq!(parse("/StEeR keep going"), ("steer".into(), "keep going"));
        assert_eq!(alias("/queue add", "hello").unwrap(), "/queue add hello");
        assert!(alias("", "hello").is_err());
        assert_eq!(complete("/技能 ar", Some(4), "args"), "/技能 args ");
        assert_eq!(complete("/he", Some(1), "help"), "/help ");
    }
}
