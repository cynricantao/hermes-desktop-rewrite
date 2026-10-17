//! End-to-end check of the in-app browser helper, against a local page — no network, no
//! backend, no model. Ignored by default: it starts a GTK/WebKit window, so it needs a
//! display, and it must NEVER run on the user's desktop (AGENTS.md §4). Run it under a
//! private headless display only:
//!
//! ```sh
//! cd helpers/hermes-browser
//! CARGO_TARGET_DIR=~/.cache/hermes-target-helper cargo build --release
//!
//! cd -
//! HERMES_BROWSER_HELPER="$PWD/helpers/hermes-browser/…/hermes-browser" \
//!   scripts/headless.sh cargo test -p hermes-core --test browser_helper -- --ignored --nocapture
//! ```
//!
//! The page it drives is written into a temp directory and opened as `file://`, so this
//! exercises exactly what the controller does: `preview.read` params → command → answer →
//! `preview_read_answer`, and `preview.act` params → command → answer →
//! `preview_act_answer`.

use std::path::PathBuf;
use std::time::Duration;

use hermes_core::browser::{
    BrowserHelper, Command, PageText, PreviewActParams, PreviewReadParams, preview_act_answer, preview_act_params,
    preview_read_answer, preview_read_params,
};
use serde_json::{Value, json};

/// The helper this test drives: `HERMES_BROWSER_HELPER` must point at the built binary.
fn helper_path() -> PathBuf {
    let path = std::env::var("HERMES_BROWSER_HELPER")
        .expect("set HERMES_BROWSER_HELPER to the built helpers/hermes-browser binary");
    let path = PathBuf::from(path);
    assert!(
        path.is_file(),
        "HERMES_BROWSER_HELPER is not a file: {}",
        path.display()
    );
    path
}

/// A throwaway directory holding the page; removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn with_page(html: &str) -> Self {
        let path = std::env::temp_dir().join(format!("hermes-browser-test-{}", std::process::id()));
        std::fs::create_dir_all(&path).expect("temp dir");
        std::fs::write(path.join("index.html"), html).expect("page");
        Self(path)
    }

    fn url(&self) -> String {
        format!("file://{}/index.html", self.0.display())
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The page: a title, prose, a button that proves a click landed, and a form that proves
/// typed text + submit landed.
const PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Browser helper test</title></head>
<body>
  <h1>Browser helper test</h1>
  <p id="prose">The quick brown fox jumps over the lazy dog.</p>
  <button id="go">Go</button>
  <div id="clicked"></div>
  <form id="form" onsubmit="return false;">
    <input id="name" name="name" placeholder="Name">
    <button id="save" type="submit">Save</button>
  </form>
  <div id="greeting"></div>
  <script>
    document.getElementById('go').addEventListener('click', function () {
      document.getElementById('clicked').textContent = 'clicked: yes';
    });
    document.getElementById('save').addEventListener('click', function () {
      document.getElementById('greeting').textContent = 'Hello ' + document.getElementById('name').value;
    });
  </script>
</body></html>
"#;

/// Drive one `preview.read` through the same mapping the controller uses.
async fn read_page(helper: &mut BrowserHelper, params: &PreviewReadParams) -> Value {
    let answer = helper.send(preview_read_params(params)).await.expect("read answered");
    let page = PageText::from_result(answer.outcome().expect("read result"));
    preview_read_answer(params, &page)
}

/// Drive one `preview.act` through the same mapping the controller uses.
async fn act(helper: &mut BrowserHelper, params: Value) -> Value {
    let params: PreviewActParams = serde_json::from_value(params).expect("act params");
    match preview_act_params(&params) {
        Ok(command) => {
            let command_for_answer = command.clone();
            let answer = helper.send(command).await.expect("act answered");
            preview_act_answer(&command_for_answer, &answer)
        }
        Err(message) => json!({ "error": message, "success": false }),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts a GTK/WebKit window — run under a private headless display (scripts/headless.sh)"]
async fn drives_a_local_page_end_to_end() {
    let page = TempDir::with_page(PAGE);
    let url = page.url();
    println!("page: {url}");

    let mut helper = BrowserHelper::spawn(&helper_path()).await.expect("helper spawn");
    helper.set_timeout(Duration::from_secs(20));

    // ── navigate + wait-for-load ──
    let answer = helper
        .send(Command::Navigate { url: url.clone() })
        .await
        .expect("navigate answered");
    println!("navigate: {answer:?}");
    assert!(answer.outcome().is_ok(), "navigate: {:?}", answer.error);

    let answer = helper
        .send(Command::WaitForLoad {
            timeout_ms: Some(10_000),
        })
        .await
        .expect("wait-for-load answered");
    println!("wait-for-load: {answer:?}");
    let loaded = answer.outcome().expect("load state");
    assert_eq!(loaded["ready_state"], json!("complete"), "{loaded}");

    // ── read ──
    let read = read_page(
        &mut helper,
        &PreviewReadParams {
            session_id: "test".into(),
            start: None,
            count: None,
        },
    )
    .await;
    println!("read: {:?}", read["title"]);
    assert_eq!(read["kind"], json!("url"));
    assert_eq!(read["title"], json!("Browser helper test"));
    assert_eq!(read["url"], json!(url));
    assert!(
        read["text"].as_str().expect("text").contains("quick brown fox"),
        "{}",
        read["text"]
    );
    assert!(read["total_chars"].as_u64().expect("total") > 0);

    // Windowing: the same page paged by start/count.
    let slice = read_page(
        &mut helper,
        &PreviewReadParams {
            session_id: "test".into(),
            start: Some(0),
            count: Some(10),
        },
    )
    .await;
    assert_eq!(slice["end"], json!(10));
    assert_eq!(slice["text"], json!(read["text"].as_str().unwrap()[..10]));

    // ── click ──
    let clicked = act(
        &mut helper,
        json!({"session_id": "test", "action": "click", "selector": "#go"}),
    )
    .await;
    println!("click: {clicked:?}");
    assert_eq!(clicked["success"], json!(true), "{clicked}");
    assert_eq!(clicked["acted"], json!("clicked button \"Go\""));

    let after_click = read_page(&mut helper, &PreviewReadParams::default()).await;
    assert!(
        after_click["text"].as_str().expect("text").contains("clicked: yes"),
        "{}",
        after_click["text"]
    );

    // ── type + submit ──
    let typed = act(
        &mut helper,
        json!({"session_id": "test", "action": "type", "selector": "#name", "text": "Hermes", "submit": true}),
    )
    .await;
    println!("type: {typed:?}");
    assert_eq!(typed["success"], json!(true), "{typed}");
    assert_eq!(typed["acted"], json!("typed into input \"Name\" and submitted"));

    let after_type = read_page(&mut helper, &PreviewReadParams::default()).await;
    assert!(
        after_type["text"].as_str().expect("text").contains("Hello Hermes"),
        "{}",
        after_type["text"]
    );

    // ── scroll: the page fits, so the pane's "nothing to scroll" note comes back ──
    let scrolled = act(
        &mut helper,
        json!({"session_id": "test", "action": "scroll", "amount": 400}),
    )
    .await;
    println!("scroll: {scrolled:?}");
    assert_eq!(scrolled["success"], json!(true), "{scrolled}");
    assert_eq!(scrolled["acted"], json!("scrolled the page"));

    // ── history verbs ──
    for action in ["back", "forward", "reload"] {
        let answer = act(&mut helper, json!({"session_id": "test", "action": action})).await;
        println!("{action}: {answer:?}");
        assert_eq!(answer["success"], json!(true), "{action}: {answer}");
        assert_eq!(answer["acted"], json!(action));
    }

    // ── an unsupported verb answers the clear error, not a hang ──
    let unsupported = act(
        &mut helper,
        json!({"session_id": "test", "action": "elements", "ref": "btn-sign-in"}),
    )
    .await;
    assert_eq!(unsupported["success"], json!(false));
    assert!(
        unsupported["error"].as_str().expect("error").contains("not supported"),
        "{unsupported}"
    );

    // ── the window is still there, and the helper goes quietly ──
    assert!(helper.is_running(), "the helper should still be running");
    helper.shutdown().await;
    println!("helper shut down");
}
