//! Rich reply rendering without a model: a stored chat whose assistant message carries a
//! code fence, a GFM table and a diff fence is seeded through `session.create {messages}`,
//! opened from the sidebar path, and its hydrated row is checked segment by segment.
use super::*;
use serde_json::json;

const SEEDED_REPLY: &str = "Here is the code:\n\n```rust\nfn main() {\n    println!(\"hello\");\n}\n```\n\n\
| Name | Count |\n|:-----|------:|\n| apples | 3 |\n| pears | 12 |\n\n\
```diff\n@@ -1,1 +1,1 @@\n-let a = 1;\n+let a = 2;\n```\n\nDone.";

pub(super) async fn section(ui: &AppWindow, log: &Log) {
    let st = ui.global::<AppState>();
    let title = format!("Rich fixture {:x}", std::process::id());
    let r = backend_request(
        "session.create",
        json!({"source": "desktop", "title": title, "messages": [
            {"role": "user", "content": "Seeded rich-reply question"},
            {"role": "assistant", "content": SEEDED_REPLY}
        ]}),
    )
    .await
    .unwrap_or_default();
    let id = r["stored_session_id"].as_str().unwrap_or_default().to_owned();
    let _ = backend_request("session.close", json!({"session_id": r["session_id"]})).await;
    let listed = wait_until(15, || st.get_sessions().iter().any(|i| i.id == id)).await;
    if !listed {
        record(log, "rich (seeded): fixture chat listed", false, format!("id={id:?}"));
        return;
    }
    st.invoke_open_session(id.clone().into());
    let hydrated = wait_until(15, || {
        rows(ui)
            .iter()
            .any(|r| r.kind == 1 && r.segs.iter().any(|s| s.kind == 3))
    })
    .await;
    let r = rows(ui);
    let Some(last) = r.iter().rev().find(|r| r.kind == 1) else {
        record(log, "rich (seeded): reply rendered", false, "no assistant row");
        return;
    };
    let segs: Vec<hermes_ui::Seg> = last.segs.iter().collect();
    let kinds: Vec<i32> = segs.iter().map(|s| s.kind).collect();
    let code = segs.iter().find(|s| s.kind == 1);
    let diff = segs.iter().find(|s| s.kind == 2);
    let table = segs.iter().find(|s| s.kind == 3);
    record(
        log,
        "rich (seeded): reply → prose, code, table, diff segments",
        hydrated && kinds.first() == Some(&0) && code.is_some() && table.is_some() && diff.is_some(),
        format!("segment kinds = {kinds:?}"),
    );
    if let Some(c) = code {
        record(
            log,
            "rich (seeded): code card keeps the raw source for copy",
            c.raw.contains("fn main") && c.raw.contains("println!(\"hello\")") && c.lines.row_count() == 3,
            format!("{} lines, raw = {:?}", c.lines.row_count(), c.raw.to_string()),
        );
        // Highlighting runs off-thread; colours arrive as <font> runs in the line text.
        // A highlighted line differs from the same line rendered with no spans.
        let dark = ui.global::<hermes_ui::Theme>().get_dark();
        let coloured = wait_until(10, || {
            rows(ui).iter().rev().find(|r| r.kind == 1).is_some_and(|r| {
                r.segs.iter().filter(|s| s.kind == 1).any(|s| {
                    s.raw
                        .lines()
                        .zip(s.lines.iter())
                        .any(|(text, l)| l.md != hermes_ui::rows::code_line(text, &[], dark))
                })
            })
        })
        .await;
        record(
            log,
            "rich (seeded): code card is syntax-highlighted",
            coloured,
            format!("coloured spans arrived = {coloured}"),
        );
    }
    if let Some(t) = table {
        record(
            log,
            "rich (seeded): table has a 2-column header and 2 body rows",
            t.header.cells.row_count() == 2 && t.rows.row_count() == 2,
            format!(
                "header {} cells, {} rows",
                t.header.cells.row_count(),
                t.rows.row_count()
            ),
        );
    }
    if let Some(d) = diff {
        let kinds: Vec<i32> = d.lines.iter().map(|l| l.kind).collect();
        record(
            log,
            "rich (seeded): diff card counts +1/−1 with line kinds",
            d.added == 1 && d.removed == 1 && kinds.contains(&1) && kinds.contains(&2),
            format!("+{} −{}, line kinds = {kinds:?}", d.added, d.removed),
        );
    }
    sleep(300).await;
    let copy = ElementHandle::find_by_accessible_label(ui, "Copy code").count();
    record(
        log,
        "rich (seeded): code and diff cards expose a Copy code button",
        copy >= 2,
        format!("{copy} Copy code buttons"),
    );
    // The testing backend's clipboard is in-process: clicking never reaches the desktop's.
    let clicked = click(ui, "Copy code").await;
    let copied = wait_until(3, || by_label(ui, "Copied").is_some()).await;
    record(
        log,
        "rich (seeded): Copy code switches to the Copied state",
        clicked && copied,
        format!("clicked={clicked} copied={copied}"),
    );
    screenshot(ui, "rich-seeded");
    let _ = backend_rest("DELETE", &format!("/api/sessions/{id}"), None).await;
}
