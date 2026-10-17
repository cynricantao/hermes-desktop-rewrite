//! In-app browser task: owns the `hermes-browser` helper process (see
//! `hermes_core::browser` and `docs/browser.md`) on its own tokio task, so a slow page load
//! or a 30 s helper timeout never stalls chat streaming.
//!
//! Inputs: the agent's `preview.open` / `preview.close` events, its `preview.read` /
//! `preview.act` server requests, and the `/preview <url>` slash command.

use hermes_core::Client;
use hermes_core::browser::{
    self as core, BrowserHelper, HelperError, NOT_ACTIVE_SESSION_ERROR, NOTHING_OPEN_ERROR, PageText, PreviewActParams,
    PreviewReadParams,
};
use serde_json::Value;
use slint::{ComponentHandle, Weak};
use tokio::sync::mpsc;

use crate::{AppState, AppWindow};

#[derive(Debug, Clone)]
pub enum BrowserCmd {
    /// Open (or reuse) the browser window on this target (URL, `localhost:port` or path).
    Open(String),
    /// Close the browser window.
    Close,
    /// Answer a `preview.read` server request.
    Read { id: String, params: Value },
    /// Answer a `preview.act` server request; `active` = the request's session is on screen.
    Act { id: String, params: Value, active: bool },
}

pub fn spawn(client: Client, weak: Weak<AppWindow>) -> mpsc::UnboundedSender<BrowserCmd> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut b = Browser {
        client,
        weak,
        helper: None,
    };
    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            b.handle(cmd).await;
        }
        if let Some(helper) = b.helper.take() {
            helper.shutdown().await;
        }
    });
    tx
}

struct Browser {
    client: Client,
    weak: Weak<AppWindow>,
    helper: Option<BrowserHelper>,
}

impl Browser {
    async fn handle(&mut self, cmd: BrowserCmd) {
        // A window the user closed leaves a dead process behind: forget it.
        if self.helper.as_ref().is_some_and(|h| !h.is_running()) {
            self.helper = None;
        }
        match cmd {
            BrowserCmd::Open(target) => self.open(&target).await,
            BrowserCmd::Close => {
                if let Some(helper) = self.helper.take() {
                    helper.shutdown().await;
                }
            }
            BrowserCmd::Read { id, params } => {
                let answer = self.read(params).await;
                self.client
                    .respond(&id, Ok(core::server_request_answer(answer.as_ref())));
            }
            BrowserCmd::Act { id, params, active } => {
                let answer = self.act(params, active).await;
                self.client.respond(&id, Ok(core::server_request_answer(Some(&answer))));
            }
        }
    }

    async fn open(&mut self, target: &str) {
        let url = core::preview_target_url(target);
        if self.helper.is_none() {
            match BrowserHelper::launch().await {
                Ok(helper) => self.helper = Some(helper),
                Err(e) => return self.banner(launch_error(&e), true),
            }
        }
        if let Some(helper) = self.helper.as_mut()
            && let Err(e) = helper.send(core::Command::Navigate { url: url.clone() }).await
        {
            self.banner(format!("The in-app browser could not open {url}: {e}"), true);
        }
    }

    /// `None` = nothing open (answered as the empty value, like the Electron pane).
    async fn read(&mut self, params: Value) -> Option<Value> {
        let params: PreviewReadParams = serde_json::from_value(params).unwrap_or_default();
        let helper = self.helper.as_mut()?;
        let answer = helper.send(core::preview_read_params(&params)).await.ok()?;
        let result = answer.outcome().ok()?;
        Some(core::preview_read_answer(&params, &PageText::from_result(result)))
    }

    async fn act(&mut self, params: Value, active: bool) -> Value {
        if !active {
            return core::preview_act_error(NOT_ACTIVE_SESSION_ERROR);
        }
        let params: PreviewActParams = serde_json::from_value(params).unwrap_or_default();
        let Some(helper) = self.helper.as_mut() else {
            return core::preview_act_error(NOTHING_OPEN_ERROR);
        };
        let command = match core::preview_act_params(&params) {
            Ok(command) => command,
            Err(message) => return core::preview_act_error(&message),
        };
        match helper.send(command.clone()).await {
            Ok(answer) => core::preview_act_answer(&command, &answer),
            Err(e) => core::preview_act_error(&e.to_string()),
        }
    }

    fn banner(&self, text: String, error: bool) {
        let _ = self.weak.upgrade_in_event_loop(move |w| {
            let st = w.global::<AppState>();
            st.set_banner(text.into());
            st.set_banner_error(error);
        });
    }
}

fn launch_error(e: &HelperError) -> String {
    format!(
        "The in-app browser is not available: {e}. Build it with `cd helpers/hermes-browser && cargo build --release` \
         and put `hermes-browser` next to the app (or set HERMES_BROWSER_HELPER)."
    )
}
