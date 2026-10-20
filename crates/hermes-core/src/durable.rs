//! Durable queue, failed-prompt and background-result state.
//!
//! The state file is scoped to a **backend/profile identity** and a **stored chat
//! id**. Runtime session ids are deliberately not persisted: the backend mints a
//! new runtime id whenever a stored chat is resumed, while the stored id is the
//! durable conversation identity.
//!
//! Writes are atomic (`create_new` temp file, mode 0600, fsync, rename, parent
//! fsync). A file that cannot be parsed or validated is never overwritten: callers
//! receive an error and the controller disables persistence until the next start.
//!
//! Prompts can contain expanded private skill bodies, so [`Debug`] for every type
//! here is redacted and no prompt text is logged or used in error messages.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::PendingPrompt;

pub const FORMAT_VERSION: u32 = 1;
/// Hard ceiling for the whole JSON document, checked before reading and saving.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_BACKENDS: usize = 16;
pub const MAX_CHATS_PER_BACKEND: usize = 500;
pub const MAX_QUEUE_ENTRIES: usize = 100;
pub const MAX_PROMPT_BYTES: usize = 64 * 1024;
pub const MAX_RESULTS_PER_CHAT: usize = 100;
pub const MAX_RESULT_TEXT_BYTES: usize = 32 * 1024;
pub const MAX_ERROR_BYTES: usize = 2 * 1024;
pub const MAX_ID_BYTES: usize = 256;

/// Default state file.
///
/// `HERMES_DESKTOP_STATE` is the explicit override used by tests/advanced setups.
/// When `HERMES_HOME` points somewhere other than the everyday `~/.hermes` (the
/// isolated test/backend case), state lives beside that private home so it can
/// never touch the user's real store. The normal desktop path is under
/// `$XDG_STATE_HOME` (not config and not the session store).
pub fn default_path() -> PathBuf {
    if let Some(path) = std::env::var_os("HERMES_DESKTOP_STATE") {
        return PathBuf::from(path);
    }
    let everyday = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".hermes"));
    if let Some(home) = std::env::var_os("HERMES_HOME").map(PathBuf::from)
        && everyday.as_ref() != Some(&home)
    {
        return home.join("desktop").join("durable-state.json");
    }
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".local")
                .join("state")
        });
    base.join("hermes-desktop-rs").join("durable-state.json")
}

/// Stable opaque namespace for a backend root + profile. Never derived from a
/// port or token, both of which change between launches.
pub fn backend_id(root: &Path, profile: Option<&str>) -> String {
    let profile = profile.map(str::trim).filter(|p| !p.is_empty()).unwrap_or("default");
    let mut hash = Fnv::new();
    hash.write(root.to_string_lossy().as_bytes());
    hash.write(&[0]);
    hash.write(profile.as_bytes());
    format!("fnv1a64:{:016x}", hash.finish())
}

/// Identity of a background completion, stable across redelivery of the same
/// event. `seq` is used when the gateway supplies one; otherwise the payload
/// itself is the identity.
pub fn result_id(task: &str, seq: Option<u64>, text: &str, error: &str) -> String {
    let mut hash = Fnv::new();
    hash.write(task.as_bytes());
    hash.write(&[0]);
    if let Some(seq) = seq {
        hash.write(b"seq");
        hash.write(&seq.to_le_bytes());
    } else {
        hash.write(text.as_bytes());
        hash.write(&[0]);
        hash.write(error.as_bytes());
    }
    format!("{task}:{:016x}", hash.finish())
}

#[derive(Debug)]
pub enum DurableError {
    Io { context: &'static str, source: io::Error },
    Corrupt(String),
    Schema(String),
    UnsupportedVersion { found: u64 },
    TooLarge { what: &'static str, limit: usize },
    Invalid(&'static str),
    Conflict,
    Busy,
}

impl fmt::Display for DurableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { context, source } => write!(f, "{context}: {source}"),
            Self::Corrupt(why) => write!(f, "state file is corrupt ({why})"),
            Self::Schema(why) => write!(f, "state file does not match schema ({why})"),
            Self::UnsupportedVersion { found } => {
                write!(
                    f,
                    "state file version {found} is not supported (expected {FORMAT_VERSION})"
                )
            }
            Self::TooLarge { what, limit } => write!(f, "{what} exceeds the {limit}-item/byte limit"),
            Self::Invalid(what) => write!(f, "invalid {what}"),
            Self::Conflict => write!(
                f,
                "saved state changed in another app instance; restart before changing this queue"
            ),
            Self::Busy => write!(
                f,
                "another app instance is saving state; queue action was not submitted"
            ),
        }
    }
}

impl std::error::Error for DurableError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for DurableError {
    fn from(source: io::Error) -> Self {
        Self::Io {
            context: "state file I/O failed",
            source,
        }
    }
}

/// A prompt whose backend acknowledgement was never observed, or whose submit
/// failed. It is recovered into the composer, never sent automatically.
#[derive(Clone, PartialEq, Eq)]
pub struct FailedPrompt {
    pub prompt: PendingPrompt,
    pub error: Option<String>,
}

impl fmt::Debug for FailedPrompt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FailedPrompt")
            .field("prompt", &self.prompt)
            .field("error", &self.error.as_ref().map(|e| e.len()))
            .finish()
    }
}

/// A background completion that has not been shown yet.
#[derive(Clone, PartialEq, Eq)]
pub struct RecoveredResult {
    pub id: String,
    pub task: String,
    pub text: String,
    pub error: bool,
}

impl fmt::Debug for RecoveredResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecoveredResult")
            .field("id", &self.id)
            .field("task", &self.task)
            .field("text_len", &self.text.len())
            .field("error", &self.error)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiskState {
    version: u32,
    #[serde(default)]
    backends: BTreeMap<String, BackendState>,
}

impl Default for DiskState {
    fn default() -> Self {
        Self {
            version: FORMAT_VERSION,
            backends: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackendState {
    #[serde(default)]
    chats: BTreeMap<String, ChatState>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChatState {
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    entries: Vec<PendingPrompt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending: Option<PendingRecord>,
    #[serde(default)]
    results: Vec<ResultRecord>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingRecord {
    text: String,
    display: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultRecord {
    id: String,
    task: String,
    text: String,
    #[serde(default)]
    error: bool,
}

/// The persisted state of one backend namespace. Cloning a snapshot is cheap
/// enough for the controller to hand it to `spawn_blocking` for an atomic write.
#[derive(Clone)]
pub struct DurableState {
    path: PathBuf,
    backend: String,
    disk: DiskState,
    // Shared by snapshots written by this controller, but not by another opener.
    expected: Arc<Mutex<Option<Vec<u8>>>>,
}

impl fmt::Debug for DurableState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DurableState")
            .field("path", &self.path)
            .field("backend", &self.backend)
            .field("backends", &self.disk.backends.len())
            .finish()
    }
}

impl DurableState {
    /// Read and validate the file; a missing file is an empty state. Corrupt,
    /// oversized or newer-version files return an error and are left untouched.
    pub fn open(path: impl Into<PathBuf>, backend: &str) -> Result<Self, DurableError> {
        validate_backend(backend)?;
        let path = path.into();
        let Some(bytes) = read_state_bytes(&path)? else {
            return Ok(Self {
                path,
                backend: backend.into(),
                disk: DiskState::default(),
                expected: Arc::new(Mutex::new(None)),
            });
        };
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| DurableError::Corrupt("invalid JSON".into()))?;
        let version = value
            .get("version")
            .and_then(Value::as_u64)
            .ok_or_else(|| DurableError::Schema("missing numeric version".into()))?;
        if version != FORMAT_VERSION as u64 {
            return Err(DurableError::UnsupportedVersion { found: version });
        }
        let disk: DiskState = serde_json::from_value(value)
            .map_err(|_| DurableError::Schema("invalid document shape or field type".into()))?;
        validate_disk(&disk)?;
        Ok(Self {
            path,
            backend: backend.into(),
            disk,
            expected: Arc::new(Mutex::new(Some(bytes))),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn backend(&self) -> &str {
        &self.backend
    }

    /// Stored chat ids in this backend namespace (deterministic order).
    pub fn chat_ids(&self) -> Vec<String> {
        self.disk
            .backends
            .get(&self.backend)
            .map(|b| b.chats.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn entries(&self, chat: &str) -> Vec<PendingPrompt> {
        self.chat(chat).map(|c| c.entries.clone()).unwrap_or_default()
    }

    pub fn paused(&self, chat: &str) -> bool {
        self.chat(chat).is_some_and(|c| c.paused)
    }

    pub fn pending(&self, chat: &str) -> Option<FailedPrompt> {
        self.chat(chat).and_then(|c| c.pending.as_ref()).map(|p| FailedPrompt {
            prompt: PendingPrompt {
                text: p.text.clone(),
                display: p.display.clone(),
            },
            error: p.error.clone(),
        })
    }

    pub fn results(&self, chat: &str) -> Vec<RecoveredResult> {
        self.chat(chat)
            .map(|c| c.results.iter().map(result_from).collect())
            .unwrap_or_default()
    }

    /// Mark every restored non-empty queue (and every caretaker prompt) paused.
    /// Returns whether anything changed.
    pub fn force_recovery_pauses(&mut self) -> Result<bool, DurableError> {
        let mut changed = false;
        if let Some(backend) = self.disk.backends.get_mut(&self.backend) {
            for chat in backend.chats.values_mut() {
                if (!chat.entries.is_empty() || chat.pending.is_some()) && !chat.paused {
                    chat.paused = true;
                    changed = true;
                }
            }
        }
        Ok(changed)
    }

    /// Replace a queue and its paused bit. Empty, unpaused, result-free chats are
    /// pruned so a long-lived file does not accumulate dead ids.
    pub fn set_queue(&mut self, chat: &str, entries: Vec<PendingPrompt>, paused: bool) -> Result<(), DurableError> {
        validate_chat(chat)?;
        validate_entries(&entries)?;
        let state = self.chat_mut(chat);
        state.entries = entries;
        state.paused = paused;
        self.prune(chat);
        Ok(())
    }

    pub fn set_paused(&mut self, chat: &str, paused: bool) -> Result<(), DurableError> {
        validate_chat(chat)?;
        self.chat_mut(chat).paused = paused;
        self.prune(chat);
        Ok(())
    }

    /// Record an in-flight prompt before `prompt.submit`; `error` is set when a
    /// submit failed. Until [`clear_pending`](Self::clear_pending), a restart
    /// treats the prompt as uncertain and never resends it.
    pub fn record_pending(
        &mut self,
        chat: &str,
        prompt: PendingPrompt,
        error: Option<&str>,
    ) -> Result<(), DurableError> {
        validate_chat(chat)?;
        validate_prompt(&prompt)?;
        if let Some(error) = error {
            validate_text("prompt error", error, MAX_ERROR_BYTES)?;
        }
        let state = self.chat_mut(chat);
        state.pending = Some(PendingRecord {
            text: prompt.text,
            display: prompt.display,
            error: error.map(str::to_owned),
        });
        Ok(())
    }

    pub fn clear_pending(&mut self, chat: &str) -> Result<(), DurableError> {
        validate_chat(chat)?;
        if let Some(state) = self.chat_mut_opt(chat) {
            state.pending = None;
        }
        self.prune(chat);
        Ok(())
    }

    /// Persist a background completion. Duplicate ids are ignored (returns
    /// `false`); the cap is an explicit error, never a silent drop.
    pub fn record_result(
        &mut self,
        chat: &str,
        id: &str,
        task: &str,
        text: &str,
        error: bool,
    ) -> Result<bool, DurableError> {
        validate_chat(chat)?;
        validate_text("result id", id, MAX_ID_BYTES)?;
        validate_text("result task", task, MAX_ID_BYTES)?;
        validate_text("result text", text, MAX_RESULT_TEXT_BYTES)?;
        let state = self.chat_mut(chat);
        if state.results.iter().any(|r| r.id == id) {
            return Ok(false);
        }
        if state.results.len() >= MAX_RESULTS_PER_CHAT {
            return Err(DurableError::TooLarge {
                what: "background results per chat",
                limit: MAX_RESULTS_PER_CHAT,
            });
        }
        state.results.push(ResultRecord {
            id: id.into(),
            task: task.into(),
            text: text.into(),
            error,
        });
        Ok(true)
    }

    /// Remove and return every pending result for a chat. Callers persist before
    /// showing them so a reopen cannot show the same uncontrolled notice twice.
    pub fn take_results(&mut self, chat: &str) -> Result<Vec<RecoveredResult>, DurableError> {
        validate_chat(chat)?;
        let results = if let Some(state) = self.chat_mut_opt(chat) {
            std::mem::take(&mut state.results)
                .into_iter()
                .map(|r| result_from(&r))
                .collect()
        } else {
            Vec::new()
        };
        self.prune(chat);
        Ok(results)
    }

    /// Validate and write atomically. The previous file is replaced only after a
    /// complete, fsynced temp file exists; any error removes the temp file.
    pub fn save(&self) -> Result<(), DurableError> {
        validate_disk(&self.disk)?;
        let body =
            serde_json::to_vec(&self.disk).map_err(|e| DurableError::Schema(format!("could not serialize: {e}")))?;
        if body.len() as u64 > MAX_FILE_BYTES {
            return Err(DurableError::TooLarge {
                what: "serialized state file",
                limit: MAX_FILE_BYTES as usize,
            });
        }
        write_atomic(&self.path, &body, &self.expected)
    }

    fn chat(&self, chat: &str) -> Option<&ChatState> {
        self.disk.backends.get(&self.backend)?.chats.get(chat)
    }

    fn chat_mut(&mut self, chat: &str) -> &mut ChatState {
        self.disk
            .backends
            .entry(self.backend.clone())
            .or_default()
            .chats
            .entry(chat.into())
            .or_default()
    }

    fn chat_mut_opt(&mut self, chat: &str) -> Option<&mut ChatState> {
        self.disk.backends.get_mut(&self.backend)?.chats.get_mut(chat)
    }

    fn prune(&mut self, chat: &str) {
        let empty = self
            .chat(chat)
            .is_some_and(|c| c.entries.is_empty() && c.pending.is_none() && c.results.is_empty() && !c.paused);
        if empty && let Some(backend) = self.disk.backends.get_mut(&self.backend) {
            backend.chats.remove(chat);
            if backend.chats.is_empty() {
                self.disk.backends.remove(&self.backend);
            }
        }
    }
}

fn result_from(r: &ResultRecord) -> RecoveredResult {
    RecoveredResult {
        id: r.id.clone(),
        task: r.task.clone(),
        text: r.text.clone(),
        error: r.error,
    }
}

fn validate_disk(disk: &DiskState) -> Result<(), DurableError> {
    if disk.version != FORMAT_VERSION {
        return Err(DurableError::UnsupportedVersion {
            found: disk.version as u64,
        });
    }
    if disk.backends.len() > MAX_BACKENDS {
        return Err(DurableError::TooLarge {
            what: "backends",
            limit: MAX_BACKENDS,
        });
    }
    for (backend, state) in &disk.backends {
        validate_backend(backend)?;
        if state.chats.len() > MAX_CHATS_PER_BACKEND {
            return Err(DurableError::TooLarge {
                what: "chats per backend",
                limit: MAX_CHATS_PER_BACKEND,
            });
        }
        for (chat, state) in &state.chats {
            validate_chat(chat)?;
            validate_entries(&state.entries)?;
            if let Some(p) = &state.pending {
                validate_prompt(&PendingPrompt {
                    text: p.text.clone(),
                    display: p.display.clone(),
                })?;
                if let Some(error) = &p.error {
                    validate_text("prompt error", error, MAX_ERROR_BYTES)?;
                }
            }
            if state.results.len() > MAX_RESULTS_PER_CHAT {
                return Err(DurableError::TooLarge {
                    what: "background results per chat",
                    limit: MAX_RESULTS_PER_CHAT,
                });
            }
            for result in &state.results {
                validate_text("result id", &result.id, MAX_ID_BYTES)?;
                validate_text("result task", &result.task, MAX_ID_BYTES)?;
                validate_text("result text", &result.text, MAX_RESULT_TEXT_BYTES)?;
            }
        }
    }
    Ok(())
}

fn validate_backend(backend: &str) -> Result<(), DurableError> {
    validate_text("backend id", backend, MAX_ID_BYTES)
}

fn validate_chat(chat: &str) -> Result<(), DurableError> {
    if chat.is_empty() {
        return Err(DurableError::Invalid("stored chat id (empty)"));
    }
    validate_text("stored chat id", chat, MAX_ID_BYTES)
}

fn validate_entries(entries: &[PendingPrompt]) -> Result<(), DurableError> {
    if entries.len() > MAX_QUEUE_ENTRIES {
        return Err(DurableError::TooLarge {
            what: "queued prompts per chat",
            limit: MAX_QUEUE_ENTRIES,
        });
    }
    for prompt in entries {
        validate_prompt(prompt)?;
    }
    Ok(())
}

fn validate_prompt(prompt: &PendingPrompt) -> Result<(), DurableError> {
    if prompt.text.is_empty() {
        return Err(DurableError::Invalid("queued prompt text (empty)"));
    }
    validate_text("queued prompt text", &prompt.text, MAX_PROMPT_BYTES)?;
    validate_text("queued prompt display", &prompt.display, MAX_PROMPT_BYTES)
}

fn validate_text(what: &'static str, text: &str, limit: usize) -> Result<(), DurableError> {
    if text.len() > limit {
        return Err(DurableError::TooLarge { what, limit });
    }
    if text.contains('\0') {
        return Err(DurableError::Invalid(what));
    }
    Ok(())
}

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct TempGuard {
    path: PathBuf,
    armed: bool,
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn read_state_bytes(path: &Path) -> Result<Option<Vec<u8>>, DurableError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() {
        return Err(DurableError::Invalid("state path (not a regular file)"));
    }
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(DurableError::TooLarge {
            what: "state file",
            limit: MAX_FILE_BYTES as usize,
        });
    }
    Ok(Some(bytes))
}

fn write_atomic(path: &Path, body: &[u8], expected: &Mutex<Option<Vec<u8>>>) -> Result<(), DurableError> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    // Existing parent permissions are not changed. New state directories are private.
    if !parent.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(parent)?;
    }
    let mut expected = expected
        .lock()
        .map_err(|_| DurableError::Invalid("state writer lock"))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("durable-state.json");
    // Lock a stable sibling inode, not the state inode replaced by rename. OS locks
    // release on process exit/crash; a leftover empty .lock file is harmless.
    let lock_path = parent.join(format!(".{name}.lock"));
    if let Ok(metadata) = fs::symlink_metadata(&lock_path)
        && !metadata.is_file()
    {
        return Err(DurableError::Invalid("writer lock path"));
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(&lock_path)?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Err(DurableError::Busy),
        Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
    }
    if read_state_bytes(path)? != *expected {
        return Err(DurableError::Conflict);
    }
    let mut allocated = None;
    for _ in 0..8 {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!(".{name}.{}.{seq}.tmp", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&tmp) {
            Ok(file) => {
                allocated = Some((file, TempGuard { path: tmp, armed: true }));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let Some((mut file, mut guard)) = allocated else {
        return Err(DurableError::Busy);
    };
    file.write_all(body)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&guard.path, path)?;
    guard.armed = false;
    *expected = Some(body.to_vec());
    // Parent sync is best effort on platforms/filesystems that cannot sync directories.
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }

    fn finish(self) -> u64 {
        self.0
    }
}
