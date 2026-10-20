//! Real-model queue checks: a prompt queued while a reply streams is saved to the isolated
//! state file, delivered exactly once after the turn, and its records are cleared afterwards.
use super::*;

const QUEUED: &str = "QUEUED-LIVE-OK";

fn state_file() -> String {
    let path = std::env::var("HERMES_DESKTOP_STATE").expect("live test sets an isolated state file");
    std::fs::read_to_string(path).unwrap_or_default()
}

pub(super) async fn section(ui: &AppWindow, log: &Log) {
    let st = ui.global::<AppState>();
    st.invoke_new_chat();
    wait_until(10, || st.get_active_session().is_empty()).await;
    let prompt =
        "Write the numbers from one to one hundred and twenty as English words, one per line. No tools, no preamble.";
    by_label(ui, "Message").unwrap().set_accessible_value(prompt);
    press(ui, Key::Return);
    let streaming = wait_until(90, || running(ui) && last_assistant(ui).lines().count() >= 3).await;
    st.invoke_send(format!("/queue Reply with exactly: {QUEUED}").into());
    let queued = wait_until(10, || st.get_queue_count() == 1).await;
    let saved = wait_until(10, || state_file().contains(QUEUED)).await;
    record(
        log,
        "durable: prompt queued while a real reply streams is saved first",
        streaming && queued && saved,
        format!(
            "streaming={streaming} queue_count={} saved={saved}",
            st.get_queue_count()
        ),
    );

    let delivered = wait_until(240, || {
        st.get_queue_count() == 0 && !running(ui) && last_assistant(ui).contains(QUEUED)
    })
    .await;
    let first_reply = rows(ui)
        .iter()
        .any(|r| r.kind == 1 && r.text.to_lowercase().contains("one hundred"));
    let submits = rows(ui)
        .iter()
        .filter(|r| r.kind == 0 && r.text.contains(QUEUED))
        .count();
    record(
        log,
        "durable: queued prompt drains once after the real turn completes",
        delivered && first_reply && submits == 1,
        format!(
            "delivered={delivered} first_reply={first_reply} queued_user_rows={submits} last={:?}",
            last_assistant(ui).chars().take(60).collect::<String>()
        ),
    );
    let cleared = wait_until(15, || !state_file().contains(QUEUED)).await;
    record(
        log,
        "durable: completed turns leave no queue or uncertain record on disk",
        cleared && idle_clean(ui).is_ok(),
        format!("record_cleared={cleared} {:?}", idle_clean(ui)),
    );
}
