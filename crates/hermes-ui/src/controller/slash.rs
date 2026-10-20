//! Desktop command routing. The backend owns command names, aliases and skills.
use super::*;

impl Controller {
    pub(super) fn notice(&mut self, text: String, error: bool) {
        self.transcript
            .blocks
            .push(hermes_core::transcript::Block::Notice { text, error });
        self.reset = true;
        self.flush();
    }

    pub(super) fn queue_key(&self) -> Option<&str> {
        self.stored.as_deref().or(self.session.as_deref())
    }

    pub(super) fn publish_queue(&self) {
        let n = self.queue_key().map(|s| self.queues.entries(s).len()).unwrap_or(0);
        ui(&self.weak, move |w| w.global::<AppState>().set_queue_count(n as i32));
    }

    async fn enqueue(&mut self, text: String, display: String) -> bool {
        let Some(key) = self.queue_key().map(str::to_owned) else {
            return false;
        };
        let previous = self.queues.entries(&key);
        let saved = self.durable.clone();
        self.queues.push(
            &key,
            hermes_core::commands::PendingPrompt {
                text,
                display: display.clone(),
            },
        );
        if !self.sync_queue(&key) || !self.persist_durable().await {
            self.queues.set_entries(&key, previous);
            self.durable = saved;
            self.queue_paused.insert(key);
            self.publish_queue();
            self.restore_rejected_queue(&display);
            return false;
        }
        self.publish_queue();
        self.banner(
            "Queued and saved for the next turn. /queue lists pending prompts.".into(),
            false,
        );
        true
    }

    pub(super) async fn drain_queue(&mut self) {
        if self.transcript.running {
            return;
        }
        let Some(key) = self.queue_key().map(str::to_owned) else {
            return;
        };
        if self.queue_paused.contains(&key) {
            return;
        }
        // An open edit holds the queue: a prompt sent now would sit between the edit's
        // target and its send. Cancelling or sending the edit drains it.
        if self.composer.editing() {
            return;
        }
        if !self.attachments.is_empty() || !self.composer.files.is_empty() {
            self.queue_paused.insert(key.clone());
            self.sync_queue(&key);
            self.persist_durable().await;
            self.banner(
                "Queue paused to preserve your staged attachment. Send or remove it, then /queue resume.".into(),
                false,
            );
            return;
        }
        let previous = self.queues.entries(&key);
        if let Some(prompt) = self.queues.pop(&key) {
            let saved = self.durable.clone();
            // The pop and uncertain submission share one atomic write-ahead snapshot.
            // There is no crash window containing neither a queue entry nor a pending prompt.
            let recorded = self
                .durable
                .as_mut()
                .is_some_and(|store| store.record_pending(&key, prompt.clone(), None).is_ok());
            if !recorded || !self.sync_queue(&key) || !self.persist_durable().await {
                self.queues.set_entries(&key, previous);
                self.durable = saved;
                self.queue_paused.insert(key);
                self.publish_queue();
                return;
            }
            self.publish_queue();
            self.submit(prompt.text, prompt.display).await;
        }
    }

    pub(super) fn restore_rejected_queue(&mut self, draft: &str) {
        // Keep the rejected request recoverable even if typing has already resumed.
        self.notice(format!("Queue request not accepted:\n{draft}"), true);
        let key = self.stored.clone().unwrap_or_default();
        let draft = draft.to_owned();
        ui(&self.weak, move |w| {
            let st = w.global::<AppState>();
            if st.get_active_session().as_str() == key && st.get_draft().is_empty() {
                st.set_draft(draft.into());
            }
        });
    }

    /// Return whether the request was accepted; callers recover rejected drafts.
    pub(super) async fn queue_command(&mut self, arg: &str) -> bool {
        if self.ensure_session().await.is_none() {
            return false;
        }
        let sid = self.queue_key().unwrap_or_default().to_owned();
        let (verb, rest) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
        if matches!(arg, "clear-results" | "discard-pending") {
            if arg == "discard-pending" && self.transcript.running {
                self.banner(
                    "Stop the active turn before discarding its recovery record.".into(),
                    true,
                );
                return false;
            }
            let saved = self.durable.clone();
            let Some(store) = self.durable.as_mut() else {
                return false;
            };
            let result = if arg == "clear-results" {
                store.take_results(&sid).map(|_| ())
            } else {
                store.clear_pending(&sid)
            };
            if let Err(error) = result {
                self.banner(format!("Could not clear saved state: {error}"), true);
                return false;
            }
            if !self.persist_durable().await {
                self.durable = saved;
                return false;
            }
            self.notice("Saved recovery records cleared for this chat.".into(), false);
        } else if arg == "resume" {
            self.queue_paused.remove(&sid);
            if !self.sync_queue(&sid) || !self.persist_durable().await {
                self.queue_paused.insert(sid);
                return false;
            }
            self.drain_queue().await;
        } else if matches!(verb, "" | "list" | "edit" | "rm" | "move" | "clear") {
            let previous = self.queues.entries(&sid);
            let saved = self.durable.clone();
            let result = match self.queues.manage(&sid, arg) {
                Ok(result) => result,
                Err(error) => {
                    self.banner(error, true);
                    return false;
                }
            };
            if matches!(verb, "edit" | "rm" | "move" | "clear")
                && (!self.sync_queue(&sid) || !self.persist_durable().await)
            {
                self.queues.set_entries(&sid, previous);
                self.durable = saved;
                self.queue_paused.insert(sid);
                self.publish_queue();
                return false;
            }
            self.publish_queue();
            self.notice(result, false);
        } else {
            let text = if verb == "add" { rest.trim() } else { arg.trim() };
            if text.is_empty() {
                self.banner("Usage: /queue <prompt>".into(), true);
                return false;
            }
            if !self.attachments.is_empty() || !self.composer.files.is_empty() {
                self.banner(
                    "Send or remove staged attachments before queuing a prompt.".into(),
                    true,
                );
                return false;
            }
            if !self.enqueue(text.into(), text.into()).await {
                return false;
            }
            self.drain_queue().await;
        }
        true
    }

    async fn steer(&mut self, text: &str) {
        if text.is_empty() {
            return self.banner("Usage: /steer <prompt>".into(), true);
        }
        if !self.transcript.running {
            return self.submit(text.into(), text.into()).await;
        }
        let Some(sid) = &self.session else {
            return;
        };
        match self
            .client
            .request("session.steer", json!({"session_id":sid, "text":text}))
            .await
        {
            Ok(v) if matches!(v.get("status").and_then(Value::as_str), Some("queued" | "redirected")) => self.notice(
                "Steer queued by the backend without interrupting the turn.".into(),
                false,
            ),
            Ok(v) if v.get("status").and_then(Value::as_str) == Some("rejected") => {
                let _ = self.enqueue(text.into(), text.into()).await;
            }
            Err(hermes_core::RpcError::Server { code: 4010, .. }) => {
                let _ = self.enqueue(text.into(), text.into()).await;
            }
            Ok(_) => {
                self.banner(
                    "The backend returned an unknown steer status. It was not resent, to avoid a duplicate.".into(),
                    true,
                );
                let draft = format!("/steer {text}");
                ui(&self.weak, move |w| w.global::<AppState>().set_draft(draft.into()));
            }
            Err(e) => {
                self.banner(format!("Could not steer: {e}"), true);
                let draft = format!("/steer {text}");
                ui(&self.weak, move |w| w.global::<AppState>().set_draft(draft.into()));
            }
        }
    }

    async fn side_prompt(&mut self, name: &str, text: &str) {
        if text.is_empty() {
            return self.banner(format!("Usage: /{name} <prompt>"), true);
        }
        let Some(sid) = self.ensure_session().await else {
            return;
        };
        let method = if name == "btw" {
            "prompt.btw"
        } else {
            "prompt.background"
        };
        match self
            .client
            .request(method, json!({"session_id":sid, "text":text}))
            .await
        {
            Ok(v) => self.notice(
                format!(
                    "Background task started: {}. Result will appear in this chat.",
                    v.get("task_id").and_then(Value::as_str).unwrap_or("task")
                ),
                false,
            ),
            Err(e) => {
                self.banner(format!("Could not start /{name}: {e}"), true);
                let draft = format!("/{name} {text}");
                ui(&self.weak, move |w| w.global::<AppState>().set_draft(draft.into()));
            }
        }
    }

    async fn command_help(&mut self) {
        match hermes_core::api::sessions::commands_catalog(&self.client, self.session.as_deref(), "").await {
            Ok(cat) => {
                let mut text = String::from(
                    "Desktop commands: /bg, /btw, /queue, /steer, /skills, /model, /reasoning, /new, /stop, /preview.\nQueues are saved per chat and restored paused. Stop/errors pause draining; /queue resume continues. /queue clear-results clears saved background results; /queue discard-pending clears an uncertain saved submission. Inactive chats wait until reopened and resumed.\nOther commands below are backend-advertised, not a promise of full desktop support. Terminal/platform-only commands can be unavailable.\n",
                );
                for pair in cat.pairs {
                    if let [name, desc, ..] = pair.as_slice() {
                        text.push_str(&format!("\n{name}: {desc}"));
                    }
                }
                if !cat.warning.is_empty() {
                    text.push_str(&format!("\n{}", cat.warning));
                }
                self.notice(text, false);
            }
            Err(e) => self.banner(format!("Could not list commands: {e}"), true),
        }
    }

    pub(super) fn completions(&mut self, text: String) {
        if let Some(task) = self.completion_task.take() {
            task.abort();
        }
        // Do not let slow completion responses replace a newer draft or another
        // runtime's suggestions. Debounce typing without blocking the controller.
        let client = self.client.clone();
        let weak = self.weak.clone();
        let sid = self.session.clone();
        self.completion_task = Some(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(120)).await;
            let result = if text.starts_with('/') && !text.contains('\n') {
                hermes_core::api::sessions::complete_slash(&client, Some(&text), sid.as_deref(), "")
                    .await
                    .ok()
            } else {
                None
            };
            ui(&weak, move |w| {
                let s = w.global::<AppState>();
                if s.get_draft().as_str() != text
                    || s.get_runtime_session().as_str() != sid.as_deref().unwrap_or_default()
                {
                    return;
                }
                let mut rows = Vec::new();
                if let Some(result) = result {
                    for item in result.items.into_iter().take(5) {
                        rows.push(crate::CommandItem {
                            text: hermes_core::commands::complete(&text, result.replace_from, &item.text).into(),
                            label: item.display.into(),
                            detail: item.meta.into(),
                        });
                    }
                }
                s.set_completions(ModelRc::new(VecModel::from(rows)));
            });
        }));
    }
}

impl Controller {
    /// Slash commands never fall through to a model prompt as ordinary text.
    pub(super) async fn slash(&mut self, text: String) {
        self.slash_depth(text.clone(), 0, &text).await;
    }

    async fn slash_depth(&mut self, text: String, depth: usize, original: &str) {
        if depth >= 8 {
            return self.banner("Command alias loop or excessive nesting.".into(), true);
        }
        let (name, arg) = hermes_core::commands::parse(&text);
        let mut command = name.as_str();
        // Resolve built-in aliases through the backend registry, not a copied list.
        if let Ok(v) = self.client.request("command.resolve", json!({"name": command})).await
            && let Some(canonical) = v.get("canonical").and_then(Value::as_str)
            && canonical != command
        {
            match hermes_core::commands::alias(canonical, arg) {
                Ok(text) => Box::pin(self.slash_depth(text, depth + 1, original)).await,
                Err(e) => self.banner(e, true),
            }
            return;
        }
        // /background is the canonical name in older registries, /bg in newer ones.
        if command == "background" {
            command = "bg";
        }
        match command {
            "help" | "commands" if arg.is_empty() => {
                self.command_help().await;
                return;
            }
            "skills" if arg.is_empty() => {
                ui(&self.weak, |w| {
                    let s = w.global::<CapState>();
                    s.set_open(true);
                    s.set_tab(1);
                });
                let _ = self.caps.send(CapCmd::Tab(1));
                return;
            }
            "reload-skills" => {
                let Some(sid) = self.ensure_session().await else {
                    return;
                };
                match hermes_core::api::extras::skills_reload(&self.client, &sid, "").await {
                    Ok(_) => self.notice("Skills reloaded. Type / to discover the refreshed list.".into(), false),
                    Err(e) => self.banner(format!("Could not reload skills: {e}"), true),
                }
                return;
            }
            "queue" | "q" => {
                if !self.queue_command(arg).await {
                    self.restore_rejected_queue(original);
                }
                return;
            }
            "steer" => {
                self.steer(arg).await;
                return;
            }
            "bg" | "btw" => {
                self.side_prompt(command, arg).await;
                return;
            }
            "new" | "reset" | "clear" => {
                Box::pin(self.handle_cmd(Cmd::NewChat)).await;
                return;
            }
            "stop" => {
                Box::pin(self.handle_cmd(Cmd::Stop)).await;
                return;
            }
            "reasoning" if !arg.is_empty() => {
                self.select_effort(arg.into()).await;
                return;
            }
            "reasoning" => {
                ui(&self.weak, |w| w.set_effort_open(true));
                return;
            }
            // In-app browser (the app's own command; Hermes's /browser is the CDP connector).
            "preview" if arg == "close" => {
                let _ = self.browser.send(BrowserCmd::Close);
                return;
            }
            "preview" if !arg.is_empty() => {
                let _ = self.browser.send(BrowserCmd::Open(arg.to_owned()));
                return;
            }
            "preview" => {
                self.banner("Usage: /preview <url or file path>, or /preview close".into(), false);
                return;
            }
            "model" if arg.is_empty() => {
                self.refresh_models(false).await;
                ui(&self.weak, |w| w.set_picker_open(true));
                return;
            }
            "prompt" | "compose" | "worktree" | "snapshot" | "snap" | "export" | "import" | "journey" | "learning"
            | "memory-graph" | "handoff" | "palette" | "pet" | "hatch" | "wake" | "browser" | "skin" | "focus"
            | "timestamps" | "battery" | "subscription" | "sessions" | "resume" | "branch" => {
                self.banner(format!("/{command} needs a native desktop integration not implemented in this preview. Use Hermes in the terminal."), true);
                return;
            }
            _ => {}
        }
        let Some(sid) = self.ensure_session().await else {
            ui(&self.weak, move |w| w.global::<AppState>().set_draft(text.into()));
            return;
        };
        if command == "model" {
            match self
                .client
                .request(
                    "config.set",
                    json!({"key":"model", "session_id":sid, "value":format!("{arg} --session")}),
                )
                .await
            {
                Ok(result) if result.get("confirm_required").and_then(Value::as_bool) == Some(true) => self.banner(
                    result
                        .get("confirm_message")
                        .and_then(Value::as_str)
                        .unwrap_or("Model change requires confirmation.")
                        .into(),
                    true,
                ),
                Ok(result) if result.get("deferred").and_then(Value::as_bool) == Some(true) => {
                    self.banner("Model change queued for the next turn.".into(), false);
                }
                Ok(_) => {
                    self.refresh_model_label().await;
                    self.refresh_effort().await;
                }
                Err(e) => self.banner(format!("Could not switch model: {e}"), true),
            }
            return;
        }
        let catalog = match hermes_core::api::sessions::commands_catalog(&self.client, Some(&sid), "").await {
            Ok(c) => c,
            Err(e) => return self.banner(format!("Could not load command registry: {e}"), true),
        };
        let key = format!("/{command}");
        if let Some(meta) = catalog.commands.get(&key) {
            if let Some(reason) = meta.get("desktop").and_then(Value::as_str)
                && !matches!(reason, "hidden" | "advanced")
            {
                return self.banner(
                    format!(
                        "/{command} is unavailable here ({reason}). Use the matching settings or terminal surface."
                    ),
                    true,
                );
            }
            if let Some(subs) = meta.get("desktop_subcommands").and_then(Value::as_array) {
                let verb = arg.split_whitespace().next().unwrap_or_default();
                if !subs.iter().any(|v| v.as_str() == Some(verb)) {
                    return self.banner(
                        format!(
                            "Unsupported desktop subcommand for /{command}. Allowed: {}",
                            subs.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
                        ),
                        true,
                    );
                }
            }
        }
        // Skills, bundles, plugin and quick commands belong to dispatch. Running
        // an unknown extension through the CLI worker can swallow its prompt.
        let extension = catalog.commands.get(&key).is_none() || catalog.skills.get(&key).is_some();
        let request = if extension {
            self.client
                .request("command.dispatch", json!({"session_id":sid,"name":command,"arg":arg}))
                .await
        } else {
            self.client
                .request(
                    "slash.exec",
                    json!({"session_id":sid,"command":text.trim_start_matches('/')}),
                )
                .await
        };
        let result = match request {
            Ok(v) => Ok(v),
            Err(hermes_core::RpcError::Server { code: -32601, .. }) if !extension => {
                self.client
                    .request("command.dispatch", json!({"session_id":sid, "name":command, "arg":arg}))
                    .await
            }
            Err(e) => Err(e),
        };
        match result {
            Ok(result) => match result.get("type").and_then(Value::as_str).unwrap_or("exec") {
                "send" | "skill" => {
                    let prompt = result
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    if prompt.is_empty() {
                        return self.banner("The command returned no message to send.".into(), true);
                    }
                    // A skill's expanded prompt can include private instructions; its short display belongs in the transcript.
                    let display = result
                        .get("display")
                        .and_then(Value::as_str)
                        .unwrap_or(&text)
                        .to_owned();
                    if let Some(notice) = result.get("notice").and_then(Value::as_str) {
                        self.notice(notice.to_owned(), false);
                    }
                    if self.transcript.running {
                        let _ = self.enqueue(prompt, display).await;
                    } else {
                        self.submit(prompt, display).await;
                    }
                }
                "prefill" => {
                    let draft = result
                        .get("message")
                        .or_else(|| result.get("text"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    ui(&self.weak, move |w| w.global::<AppState>().set_draft(draft.into()));
                }
                "alias" => {
                    match hermes_core::commands::alias(
                        result.get("target").and_then(Value::as_str).unwrap_or_default(),
                        arg,
                    ) {
                        Ok(text) => Box::pin(self.slash_depth(text, depth + 1, original)).await,
                        Err(e) => self.banner(e, true),
                    }
                }
                _ => {
                    let output = result
                        .get("output")
                        .or_else(|| result.get("notice"))
                        .and_then(Value::as_str)
                        .unwrap_or("Command completed.")
                        .to_owned();
                    self.transcript.blocks.push(hermes_core::transcript::Block::Notice {
                        text: output,
                        error: false,
                    });
                    self.reset = true;
                    self.flush();
                    self.refresh_model_label().await;
                    self.refresh_effort().await;
                }
            },
            Err(e) => {
                self.banner(format!("Command failed: {e}"), true);
                ui(&self.weak, move |w| w.global::<AppState>().set_draft(text.into()));
            }
        }
    }
}
