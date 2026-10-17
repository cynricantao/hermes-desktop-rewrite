//! The helper's half of the control wire: line-delimited JSON on stdin
//! (commands) and stdout (events).
//!
//! The controller's half is `crates/hermes-core/src/browser.rs`, which documents
//! the protocol normatively. This program is a SEPARATE Cargo package outside
//! the workspace (root `Cargo.toml` `exclude`), so it cannot depend on
//! `hermes-core`: the two sets of types are kept in sync by hand — change both,
//! and the tests in both files pin the wire.
//!
//! * Request: `{"id":1,"cmd":"navigate","url":"https://example.com/"}`
//! * Answer: `{"id":1,"ok":true,"result":{…}}` or `{"id":1,"ok":false,"error":"…"}`
//! * Notice (no id): `{"event":"ready"}` once the window is up,
//!   `{"event":"closed"}` when it goes away.
//!
//! Unknown verbs answer `unsupported command '<verb>' …` rather than being
//! ignored; a line that cannot be decoded at all is reported the same way when
//! it carries an id, and logged on stderr when it does not.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The verbs this helper understands, in the order its error message lists them.
pub const COMMANDS: [&str; 10] = [
    "navigate",
    "back",
    "forward",
    "reload",
    "read",
    "click",
    "type",
    "scroll",
    "wait-for-load",
    "shutdown",
];

/// One command from the controller; `cmd` tags the verb.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum Command {
    /// Load `url` — any scheme WebKit resolves, including `file://`.
    Navigate { url: String },
    /// Go back one entry in the view's history.
    Back,
    /// Go forward one entry in the view's history.
    Forward,
    /// Reload the current document.
    Reload,
    /// Answer with the page's identity and text (see [`crate::page::read`]).
    Read,
    /// Click: the element matching the CSS `selector`, or the element at (x, y).
    Click {
        #[serde(default)]
        selector: Option<String>,
        #[serde(default)]
        x: Option<f64>,
        #[serde(default)]
        y: Option<f64>,
    },
    /// Put `text` into the field matching `selector`; `submit` also presses Enter
    /// and submits the owning form.
    Type {
        selector: String,
        text: String,
        #[serde(default)]
        submit: bool,
    },
    /// Scroll by `amount` pixels (negative = up; absent = one screen), or to `to`
    /// ("top" / "bottom").
    Scroll {
        #[serde(default)]
        amount: Option<f64>,
        #[serde(default)]
        to: Option<String>,
    },
    /// Answer once the document reports `readyState === "complete"`, or after
    /// `timeout_ms` (default 20 s).
    WaitForLoad {
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// Answer, then close the window and exit. `Drop` of the controller's handle
    /// accomplishes the same thing without the goodbye.
    Shutdown,
}

/// One request line: the command plus the id its answer must echo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub command: Command,
}

/// One answer to one [`Request`]. Exactly one of `result` / `error` is set.
/// `main.rs` writes answers with `json!`; this type pins that wire shape in tests.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What a stdin line means to the GTK loop.
#[derive(Debug, Clone, PartialEq)]
pub enum Signal {
    /// A well-formed command.
    Request(Box<Request>),
    /// The line could not be decoded: answer the error when it named an id,
    /// otherwise just log it.
    Bad { id: Option<u64>, message: String },
    /// stdin reached EOF — the controller dropped the handle, so the window
    /// should go away with it.
    StdinClosed,
}

/// Decode one stdin line into a [`Signal`].
pub fn parse_line(line: &str) -> Signal {
    let trimmed = line.trim();
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return Signal::Bad {
            id: None,
            message: format!("line is not JSON: {}", clip(trimmed, 120)),
        };
    };
    let id = value.get("id").and_then(Value::as_u64);
    match serde_json::from_value::<Request>(value.clone()) {
        Ok(request) => Signal::Request(Box::new(request)),
        Err(error) => {
            let verb = value.get("cmd").and_then(Value::as_str).unwrap_or("");
            let message = if !verb.is_empty() && !COMMANDS.contains(&verb) {
                format!(
                    "unsupported command '{verb}' — this helper understands: {}",
                    COMMANDS.join(", ")
                )
            } else {
                format!("bad command: {error}")
            };
            Signal::Bad { id, message }
        }
    }
}

/// First `max` characters of `text`, with an ellipsis when it was cut — error
/// messages quote page code, which can be arbitrarily long.
pub(crate) fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire the controller sends, verb for verb (browser.rs sends these).
    #[test]
    fn decodes_every_command_the_controller_sends() {
        let cases = [
            (
                r#"{"id":1,"cmd":"navigate","url":"file:///tmp/a.html"}"#,
                Command::Navigate {
                    url: "file:///tmp/a.html".into(),
                },
            ),
            (r#"{"id":2,"cmd":"back"}"#, Command::Back),
            (r#"{"id":3,"cmd":"forward"}"#, Command::Forward),
            (r#"{"id":4,"cmd":"reload"}"#, Command::Reload),
            (r#"{"id":5,"cmd":"read"}"#, Command::Read),
            (
                r##"{"id":6,"cmd":"click","selector":"#go"}"##,
                Command::Click {
                    selector: Some("#go".into()),
                    x: None,
                    y: None,
                },
            ),
            (
                r#"{"id":7,"cmd":"click","x":10.0,"y":20.5}"#,
                Command::Click {
                    selector: None,
                    x: Some(10.0),
                    y: Some(20.5),
                },
            ),
            (
                r##"{"id":8,"cmd":"type","selector":"#q","text":"hi","submit":true}"##,
                Command::Type {
                    selector: "#q".into(),
                    text: "hi".into(),
                    submit: true,
                },
            ),
            (
                r#"{"id":9,"cmd":"scroll","amount":600}"#,
                Command::Scroll {
                    amount: Some(600.0),
                    to: None,
                },
            ),
            (
                r#"{"id":10,"cmd":"scroll","to":"bottom"}"#,
                Command::Scroll {
                    amount: None,
                    to: Some("bottom".into()),
                },
            ),
            (
                r#"{"id":11,"cmd":"wait-for-load"}"#,
                Command::WaitForLoad { timeout_ms: None },
            ),
            (
                r#"{"id":12,"cmd":"wait-for-load","timeout_ms":500}"#,
                Command::WaitForLoad { timeout_ms: Some(500) },
            ),
            (r#"{"id":13,"cmd":"shutdown"}"#, Command::Shutdown),
        ];
        for (line, want) in cases {
            match parse_line(line) {
                Signal::Request(request) => assert_eq!(request.command, want, "{line}"),
                other => panic!("{line}: {other:?}"),
            }
        }
    }

    #[test]
    fn unsupported_verbs_name_themselves_and_the_known_ones() {
        match parse_line(r#"{"id":9,"cmd":"press","key":"Enter"}"#) {
            Signal::Bad { id: Some(9), message } => {
                assert!(message.starts_with("unsupported command 'press'"), "{message}");
                assert!(message.contains("wait-for-load"), "{message}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_known_verb_with_a_wrong_shape_is_a_bad_command() {
        match parse_line(r#"{"id":3,"cmd":"navigate"}"#) {
            Signal::Bad { id: Some(3), message } => assert!(message.starts_with("bad command:"), "{message}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn garbage_and_id_less_lines_are_reported_without_an_id() {
        assert!(matches!(parse_line("not json"), Signal::Bad { id: None, .. }));
        assert!(matches!(parse_line(r#"{"cmd":"read"}"#), Signal::Bad { id: None, .. }));
    }

    #[test]
    fn answers_serialize_to_the_controller_shape() {
        let answer = Answer {
            id: 4,
            ok: true,
            result: Some(serde_json::json!({"range": 0})),
            error: None,
        };
        assert_eq!(
            serde_json::to_string(&answer).unwrap(),
            r#"{"id":4,"ok":true,"result":{"range":0}}"#
        );
    }
}
