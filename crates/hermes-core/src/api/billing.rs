//! Billing, subscriptions and the free tier: `billing.*`, `subscription.*` and `free_tier.*`
//! RPCs plus their notifications (`billing.step_up.verification`, `free_tier.challenge`), and the
//! one `onboarding.*` method the other areas did not wrap yet (`onboarding.set_run`).
//!
//! Handlers: `tui_gateway/methods_session.py` (billing / subscription: `_billing_view`,
//! `_billing_route`, `billing.step_up`), `tui_gateway/billing_view.py` (the wire serializers),
//! `tui_gateway/methods_free_tier.py`, `tui_gateway/methods_onboarding.py`. Contracts:
//! `tui_gateway/contracts/billing_delegation_pets.py` and `config_free_tier_control.py`.
//!
//! # Money safety
//! **Every method here except [`free_tier_status`] reaches the Nous portal over the network or
//! writes local state**, and several of them spend money or change the plan. All of them are
//! therefore *builder + unit test only* — nothing here is exercised by the live test
//! (`tests/live_api_billing.rs`); only [`free_tier_status`], which the handler proves to be a local
//! read with no side effects, is called live. Each wrapper that can charge or change the plan says
//! so in bold; the UI must confirm before it calls one.
//!
//! # Shape of a reply
//! Billing routes answer a **structured envelope** (`result.ok` / `result.error`) instead of
//! JSON-RPC errors, so a wrapper succeeds even when the portal refused: check
//! [`BillingMutation::ok`] and [`BillingMutation::error`] (`insufficient_scope`,
//! `session_revoked`, `remote_spending_revoked`, `rate_limited`, `payment_failed`, …) and branch.
//! Only a broken pipe / unknown method surfaces as [`ApiError`].
//!
//! Money crosses the gateway wire as **strings** (`balance_usd`, `threshold_usd`,
//! `charge_presets`, …); the backend converts them to floats before calling the portal, so the
//! strings avoid client-side rounding only. The pre-formatted `*_display` fields are what a UI
//! can show as-is.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::api::system::{OnboardingState, UsageBars};
use crate::api::{ApiError, Call, call_as, call_of, decode};
use crate::rpc::Client;

// ── Timeouts ───────────────────────────────────────────────────────────────────
// The gateway default is 30 s (`rpc::DEFAULT_TIMEOUT`). Portal round-trips can exceed it, and
// `billing.step_up` deliberately blocks for minutes while the user finishes a browser flow.

/// `billing.state`, `billing.charge`, `billing.charge_status`, `billing.auto_reload`,
/// `free_tier.provision` and the `subscription.*` routes. Each is a few portal requests
/// (`billing.state` also fetches usage), each bounded by the portal client in tens of seconds.
pub const BILLING_PORTAL_TIMEOUT: Duration = Duration::from_secs(60);

/// `billing.step_up` — the device flow polls until the user approves in a browser or the
/// portal's `expires_in` passes (`hermes_cli/auth_device_flow.py`; typically 10–15 minutes).
/// A shorter client deadline would abandon a flow that can still succeed, so this covers the
/// device-code maximum. If it does time out, treat the outcome as unknown, not as denied.
pub const BILLING_STEP_UP_TIMEOUT: Duration = Duration::from_secs(900);

/// Send a prepared [`Call`] with an explicit timeout and decode the result.
async fn call_timed_as<T: DeserializeOwned>(client: &Client, c: &Call, timeout: Duration) -> Result<T, ApiError> {
    let v = client.request_with_timeout(c.method, c.params.clone(), timeout).await?;
    decode(v)
}

// ── Param helpers ──────────────────────────────────────────────────────────────

/// Build a params object, dropping `null` entries so an omitted option stays omitted.
fn params(pairs: &[(&str, Value)]) -> Value {
    let mut map = Map::new();
    for (k, v) in pairs.iter().filter(|(_, v)| !v.is_null()) {
        map.insert((*k).to_owned(), v.clone());
    }
    Value::Object(map)
}

/// The contract's optional `profile` param. The local billing handlers are not profile-scoped
/// (plain `@method` in `tui_gateway/methods_session.py`): they always answer for the account the
/// backend was launched with. The param only matters to a shared remote gateway that routes by
/// it, so a UI must not present it as "this profile's billing". Omitted when empty.
fn profile_param(profile: &str) -> Value {
    if profile.is_empty() {
        Value::Null
    } else {
        json!(profile)
    }
}

/// An optional string param (`None` and `""` are omitted).
fn opt_str(v: Option<&str>) -> Value {
    match v {
        Some(s) if !s.is_empty() => json!(s),
        _ => Value::Null,
    }
}

/// A wire scalar that may arrive as a JSON string or a number. Charge amounts are money: the
/// portal may quote `"12.34"` or `12.34`, and the backend passes either through untouched.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(untagged)]
pub enum BillingScalar {
    Text(String),
    Number(f64),
    Bool(bool),
}

impl BillingScalar {
    /// The value as text, for display (`12.34` and `"12.34"` both → `"12.34"`).
    pub fn as_text(&self) -> String {
        match self {
            BillingScalar::Text(s) => s.clone(),
            BillingScalar::Number(n) => n.to_string(),
            BillingScalar::Bool(b) => b.to_string(),
        }
    }

    /// The value as a number, when it is one (`"12.34"` parses).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            BillingScalar::Text(s) => s.trim().parse().ok(),
            BillingScalar::Number(n) => Some(*n),
            BillingScalar::Bool(_) => None,
        }
    }
}

/// Mint an idempotency key for a charge / upgrade. **Reuse the same key when retrying** that
/// operation: the portal de-duplicates on it, so a retry after a timeout cannot charge twice.
///
/// UUID v4 shaped, like every other caller (`agent/billing_view.py` uses `uuid.uuid4()`, the
/// Electron desktop `crypto.randomUUID()`), in case the portal validates the format. The 128
/// bits come from std's randomly seeded hasher mixed with time, pid and a counter, so no extra
/// crate is needed; uniqueness, not secrecy, is what the key requires.
pub fn new_idempotency_key() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let half = |salt: u64| {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(nanos);
        h.write_u32(std::process::id());
        h.write_u64(seq);
        h.write_u64(salt);
        h.finish()
    };
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&half(1).to_be_bytes());
    b[8..].copy_from_slice(&half(2).to_be_bytes());
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // RFC 4122 variant
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// Refuse a blank idempotency key before sending: the gateway mints a fresh key when the param
/// is missing or empty, so a retry with `""` would not be de-duplicated (a double charge).
/// The async wrappers call this; a caller that sends [`billing_charge_call`] or
/// [`subscription_upgrade_call`] itself must call it first.
pub fn require_idempotency_key(key: &str) -> Result<(), ApiError> {
    if key.trim().is_empty() {
        return Err(ApiError::Invalid(
            "idempotency_key must be a non-empty key reused across retries".into(),
        ));
    }
    Ok(())
}

// ── Shared billing types ─────────────────────────────────────────────────────

/// The saved card (`BillingCardInfo`); `display` is the ready-to-show label.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BillingCardInfo {
    pub brand: String,
    pub last4: String,
    /// e.g. `•••• 4242`.
    pub masked: String,
    pub display: Option<String>,
    /// Which discovery rung resolved it (portal account, payment-methods list, …).
    pub resolved_via: Option<String>,
}

/// The payment method the portal charges (`BillingPaymentMethod`). Each `kind` sends only its own
/// fields, so check `kind` before reading `last4` / `email`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BillingPaymentMethod {
    /// `card` | `link` | `unknown`.
    pub kind: String,
    pub brand: Option<String>,
    pub last4: Option<String>,
    /// Apple Pay / Google Pay, when the card was tokenized through a wallet.
    pub wallet: Option<String>,
    /// Link (Stripe Link) account email.
    pub email: Option<String>,
    /// What the server called a method this client does not understand.
    pub raw_kind: Option<String>,
    pub resolved_via: Option<String>,
}

/// The monthly spend ceiling (`BillingMonthlyCap`); `is_default_ceiling` means no explicit cap.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BillingMonthlyCap {
    pub limit_usd: Option<String>,
    pub limit_display: String,
    pub spent_this_month_usd: Option<String>,
    pub spent_display: String,
    pub is_default_ceiling: bool,
}

/// The card an auto-reload charges (`BillingAutoReloadCard`); only `distinct` names it.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BillingAutoReloadCard {
    /// `canonical` | `distinct` | `none`.
    pub kind: String,
    pub payment_method_id: Option<String>,
    pub brand: Option<String>,
    pub last4: Option<String>,
}

/// Current auto-reload setting (`BillingAutoReload`): top up to `reload_to` when the balance
/// drops below `threshold`. **Charges money** every time it fires — show it before enabling.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BillingAutoReload {
    pub enabled: bool,
    pub threshold_usd: Option<String>,
    pub threshold_display: String,
    pub reload_to_usd: Option<String>,
    pub reload_to_display: String,
    pub card: Option<BillingAutoReloadCard>,
}

/// `billing.state` (`BillingStateResult`). Fail-open: the handler's `except` fallback emits only
/// `ok`, `logged_in`, `free_tier_account` and `error`, so every other field is optional.
///
/// `free_tier_account` marks the Nous free tier: no account and no balance to show, so the UI
/// branches on it **before** `logged_in` (`free_tier_model` names the guest model).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BillingState {
    pub ok: bool,
    pub logged_in: bool,
    pub free_tier_account: bool,
    pub free_tier_model: Option<String>,
    pub org_name: Option<String>,
    pub org_slug: Option<String>,
    pub role: Option<String>,
    pub is_admin: Option<bool>,
    pub can_change_plan: Option<bool>,
    pub can_charge: Option<bool>,
    pub balance_usd: Option<String>,
    pub balance_display: Option<String>,
    pub cli_billing_enabled: Option<bool>,
    /// Preset top-up amounts as wire strings (money-safe), plus their display labels.
    #[serde(deserialize_with = "crate::api::null_default")]
    pub charge_presets: Vec<String>,
    #[serde(deserialize_with = "crate::api::null_default")]
    pub charge_presets_display: Vec<String>,
    pub min_usd: Option<String>,
    pub max_usd: Option<String>,
    pub card: Option<BillingCardInfo>,
    pub payment_method: Option<BillingPaymentMethod>,
    pub monthly_cap: Option<BillingMonthlyCap>,
    pub auto_reload: Option<BillingAutoReload>,
    pub portal_url: Option<String>,
    pub error: Option<String>,
    /// Two-bar dollar usage model, embedded (fail-open to `{available: false}`).
    pub usage: Option<UsageBars>,
}

/// The refusal / mutation envelope every billing write answers (`BillingMutationResult` family).
/// `ok: false` is a normal reply, not an error: branch on `error`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BillingMutation {
    pub ok: bool,
    /// `insufficient_scope` (→ run [`billing_step_up`]), `session_revoked`,
    /// `remote_spending_revoked`, `rate_limited`, `payment_failed`, `invalid_request`,
    /// `invalid_charge_id`, `error`, …
    pub error: Option<String>,
    pub message: Option<String>,
    /// Where to send the user to fix it (sign in again, finish SCA, manage the plan).
    pub portal_url: Option<String>,
    /// Seconds to wait before retrying, when the portal said so.
    pub retry_after: Option<f64>,
    /// The raw portal body, when it sent one.
    pub payload: Option<Value>,
    /// Remote-Spending contract extras: who must act, a machine code, and recovery copy.
    pub actor: Option<String>,
    pub code: Option<String>,
    pub recovery: Option<String>,
}

/// `billing.charge` reply (`BillingChargeResult`): `202` — money is **not** confirmed yet, poll
/// [`billing_charge_status`] with `charge_id`. `idempotency_key` is echoed (also on a refusal).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BillingCharge {
    #[serde(flatten)]
    pub envelope: BillingMutation,
    pub charge_id: Option<String>,
    pub idempotency_key: Option<String>,
}

/// One status read (`BillingChargeStatusResult`) — the caller drives the poll cadence.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BillingChargeStatus {
    #[serde(flatten)]
    pub envelope: BillingMutation,
    /// `pending` | `settled` | `failed` (portal-specific extras possible).
    pub status: Option<String>,
    pub amount_usd: Option<BillingScalar>,
    pub settled_at: Option<String>,
    pub reason: Option<String>,
}

/// `billing.step_up` reply: `granted` is false when the server downscopes (also on refusals).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BillingStepUp {
    #[serde(flatten)]
    pub envelope: BillingMutation,
    pub granted: Option<bool>,
}

// ── Subscription types ───────────────────────────────────────────────────────

/// One selectable plan (`SubscriptionTierOption`); the price arrives pre-formatted.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SubscriptionTierOption {
    pub tier_id: String,
    pub name: String,
    pub tier_order: i64,
    /// e.g. `$20` / `$20.00`.
    pub dollars_per_month_display: String,
    pub monthly_credits: Option<String>,
    pub is_current: bool,
    pub is_enabled: bool,
}

/// The plan in force (`CurrentSubscription`) plus any scheduled change.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CurrentSubscription {
    pub tier_id: Option<String>,
    pub tier_name: Option<String>,
    pub monthly_credits: Option<String>,
    pub credits_remaining: Option<String>,
    pub cycle_ends_at: Option<String>,
    pub pending_downgrade_tier_name: Option<String>,
    pub pending_downgrade_at: Option<String>,
    pub pending_downgrade_display: Option<String>,
    pub cancel_at_period_end: bool,
    pub cancellation_effective_at: Option<String>,
    pub cancellation_effective_display: Option<String>,
}

/// `subscription.state` (`SubscriptionStateResult`); the view's fallback emits only
/// `ok`, `logged_in`, `error`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SubscriptionState {
    pub ok: bool,
    pub logged_in: bool,
    pub is_admin: Option<bool>,
    pub can_change_plan: Option<bool>,
    pub org_name: Option<String>,
    pub org_id: Option<String>,
    pub role: Option<String>,
    /// `personal` | `team`.
    pub context: Option<String>,
    pub current: Option<CurrentSubscription>,
    #[serde(deserialize_with = "crate::api::null_default")]
    pub tiers: Vec<SubscriptionTierOption>,
    pub portal_url: Option<String>,
    pub error: Option<String>,
    pub usage: Option<UsageBars>,
}

/// A chargeless quote for a plan change (`SubscriptionPreviewResult`); `effect` drives the
/// confirm copy: `charge_now` | `scheduled` | `no_op` | `blocked`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SubscriptionPreview {
    #[serde(flatten)]
    pub envelope: BillingMutation,
    pub effect: Option<String>,
    pub reason: Option<String>,
    pub current_tier_id: Option<String>,
    pub current_tier_name: Option<String>,
    pub target_tier_id: Option<String>,
    pub target_tier_name: Option<String>,
    pub monthly_credits_delta: Option<String>,
    pub amount_due_now_cents: Option<i64>,
    pub effective_at: Option<String>,
}

/// The money route's reply (`SubscriptionUpgradeResult`): `status` separates a completed upgrade
/// from an SCA / decline that must finish in the portal at `recovery_url`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SubscriptionUpgrade {
    #[serde(flatten)]
    pub envelope: BillingMutation,
    /// `ok` | `requires_action` | `payment_failed` | …
    pub status: Option<String>,
    pub target_tier_name: Option<String>,
    pub recovery_url: Option<String>,
    pub reason: Option<String>,
    pub idempotency_key: Option<String>,
}

/// A scheduled change / cancellation (`BillingPendingChangeResult`): the portal's message plus
/// its raw body in `payload`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SubscriptionPendingChange {
    #[serde(flatten)]
    pub envelope: BillingMutation,
}

// ── Free tier types ──────────────────────────────────────────────────────────

/// The browser challenge the account service is waiting on (`FreeTierChallengePayload`). It
/// arrives as the `free_tier.challenge` event and as [`FreeTierStatus::challenge`] for a client
/// that connected after the event fired.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct FreeTierChallenge {
    /// Always `browser` today.
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    /// The portal will not proceed without it (vs. an optional speed-up).
    pub required: bool,
    pub expires_in: i64,
    pub message: String,
    pub attempt: Option<i64>,
}

/// `free_tier.status` (`FreeTierStatusResult`). `available` = a guest identity exists **and** the
/// tier carries inference here; whether inference actually routes through it is
/// `setup.runtime_check`'s answer. `enabled` = available, or the trial can still be claimed.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct FreeTierStatus {
    pub has_guest: bool,
    pub enabled: bool,
    pub available: bool,
    /// The one-time "free tier is on" notice has not been acknowledged yet.
    pub notice_pending: bool,
    pub model: String,
    pub label: String,
    /// Why there is no identity, when the last claim failed.
    pub error: Option<String>,
    pub error_code: Option<String>,
    pub retryable: Option<bool>,
    pub retry_after: Option<f64>,
    pub challenge: Option<FreeTierChallenge>,
    /// Seconds until the post-task sign-in offer is due; absent when none is pending.
    pub nudge_due_in: Option<f64>,
}

/// `free_tier.provision` reply — the retry of the welcome-trial claim.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct FreeTierProvision {
    pub has_guest: bool,
    pub enabled: bool,
    pub error: Option<String>,
    pub error_code: Option<String>,
    pub retryable: Option<bool>,
    pub retry_after: Option<f64>,
}

/// `free_tier.ack_notice` reply; `acked: false` when there is no identity to mark.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct FreeTierAck {
    pub acked: bool,
}

/// `free_tier.claim_nudge` reply; `claimed: true` for exactly one caller each time an offer is due.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct FreeTierClaimNudge {
    pub claimed: bool,
}

/// `free_tier.challenge_result` reply: whether the backend accepted the presentation hint.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct FreeTierChallengeResult {
    pub accepted: bool,
}

/// How a browser challenge ended, as `free_tier.challenge_result` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeOutcome {
    /// The user finished it.
    Done,
    Failed,
    /// The user dismissed the window.
    Closed,
    /// It expired (or the client gave up waiting).
    Timeout,
    /// The user refused to open it.
    Refused,
    Error,
    /// No browser could be opened.
    Unsupported,
}

impl ChallengeOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            ChallengeOutcome::Done => "done",
            ChallengeOutcome::Failed => "failed",
            ChallengeOutcome::Closed => "closed",
            ChallengeOutcome::Timeout => "timeout",
            ChallengeOutcome::Refused => "refused",
            ChallengeOutcome::Error => "error",
            ChallengeOutcome::Unsupported => "unsupported",
        }
    }
}

// ── Billing ─────────────────────────────────────────────────────────────────

/// `billing.state`: the focused profile's account, balance, card, cap, auto-reload and usage.
/// **Network** — the backend round-trips the Nous portal when logged in (the free tier is
/// answered locally). Read-only: no money moves, no plan changes. Builder + unit test only.
pub fn billing_state_call(profile: &str) -> Call {
    call_of("billing.state", params(&[("profile", profile_param(profile))]))
}

/// Read the billing view. **Network** (portal); read-only.
pub async fn billing_state(client: &Client, profile: &str) -> Result<BillingState, ApiError> {
    call_timed_as(client, &billing_state_call(profile), BILLING_PORTAL_TIMEOUT).await
}

/// `billing.auto_reload`: set the balance threshold and the top-up target.
/// **Charges money** — enabling it makes the portal charge the saved card, repeatedly, whenever
/// the balance falls below `threshold`. The handler requires **both** amounts, so a `None` here
/// yields an `invalid_request` envelope. Builder + unit test only.
pub fn billing_auto_reload_call(
    enabled: bool,
    threshold: Option<&str>,
    top_up_amount: Option<&str>,
    profile: &str,
) -> Call {
    call_of(
        "billing.auto_reload",
        params(&[
            ("profile", profile_param(profile)),
            ("enabled", json!(enabled)),
            ("threshold", opt_str(threshold)),
            ("top_up_amount", opt_str(top_up_amount)),
        ]),
    )
}

/// Change auto-reload. **Charges money** (recurring automatic top-ups).
pub async fn billing_auto_reload(
    client: &Client,
    enabled: bool,
    threshold: Option<&str>,
    top_up_amount: Option<&str>,
    profile: &str,
) -> Result<BillingMutation, ApiError> {
    call_timed_as(
        client,
        &billing_auto_reload_call(enabled, threshold, top_up_amount, profile),
        BILLING_PORTAL_TIMEOUT,
    )
    .await
}

/// `billing.charge`: charge the saved card `amount_usd` (a decimal string keeps money exact).
/// Only the *start* of a charge: the reply carries `charge_id`, and money is not confirmed until
/// [`billing_charge_status`] says `settled`.
///
/// **Charges money.** Pass a stable `idempotency_key` (e.g. [`new_idempotency_key`]) and reuse it
/// when retrying, or a timed-out retry can charge twice. A blank key is omitted here, so check it
/// with [`require_idempotency_key`] when not going through [`billing_charge`]. Builder + unit
/// test only.
pub fn billing_charge_call(amount_usd: &str, idempotency_key: &str, profile: &str) -> Call {
    call_of(
        "billing.charge",
        params(&[
            ("profile", profile_param(profile)),
            ("amount_usd", opt_str(Some(amount_usd))),
            ("idempotency_key", opt_str(Some(idempotency_key))),
        ]),
    )
}

/// Start a top-up charge. **Charges money.**
pub async fn billing_charge(
    client: &Client,
    amount_usd: &str,
    idempotency_key: &str,
    profile: &str,
) -> Result<BillingCharge, ApiError> {
    require_idempotency_key(idempotency_key)?;
    call_timed_as(
        client,
        &billing_charge_call(amount_usd, idempotency_key, profile),
        BILLING_PORTAL_TIMEOUT,
    )
    .await
}

/// `billing.charge_status`: one status read for a charge (`pending` | `settled` | `failed`).
/// **Network** (portal read). The caller owns the poll cadence — the desktop polls on an interval
/// with a cap, then reports "unconfirmed" rather than assuming success. Builder + unit test only.
pub fn billing_charge_status_call(charge_id: &str, profile: &str) -> Call {
    call_of(
        "billing.charge_status",
        params(&[
            ("profile", profile_param(profile)),
            ("charge_id", opt_str(Some(charge_id))),
        ]),
    )
}

/// Read one charge's status. **Network** (portal); no money moves.
pub async fn billing_charge_status(
    client: &Client,
    charge_id: &str,
    profile: &str,
) -> Result<BillingChargeStatus, ApiError> {
    call_timed_as(
        client,
        &billing_charge_status_call(charge_id, profile),
        BILLING_PORTAL_TIMEOUT,
    )
    .await
}

/// `billing.step_up`: the `billing:manage` device flow — unlocks charging / plan changes after an
/// `insufficient_scope` refusal. The URL and code arrive as the
/// [`BILLING_STEP_UP_VERIFICATION_EVENT`] notification (the desktop opens the browser itself,
/// never the gateway); the call blocks until the user finishes or the flow expires.
///
/// **Network + blocks for minutes**: run it off the UI thread and allow
/// [`BILLING_STEP_UP_TIMEOUT`]. Builder + unit test only.
pub fn billing_step_up_call(session_id: Option<&str>, profile: &str) -> Call {
    call_of(
        "billing.step_up",
        params(&[("profile", profile_param(profile)), ("session_id", opt_str(session_id))]),
    )
}

/// Run the step-up device flow. **Network**, blocking; opens nothing by itself.
pub async fn billing_step_up(
    client: &Client,
    session_id: Option<&str>,
    profile: &str,
) -> Result<BillingStepUp, ApiError> {
    call_timed_as(
        client,
        &billing_step_up_call(session_id, profile),
        BILLING_STEP_UP_TIMEOUT,
    )
    .await
}

// ── Subscriptions ───────────────────────────────────────────────────────────

/// `subscription.state`: the plan in force, scheduled changes and the selectable catalog.
/// **Network** (portal read). Builder + unit test only.
pub fn subscription_state_call(profile: &str) -> Call {
    call_of("subscription.state", params(&[("profile", profile_param(profile))]))
}

/// Read the subscription view. **Network** (portal); read-only.
pub async fn subscription_state(client: &Client, profile: &str) -> Result<SubscriptionState, ApiError> {
    call_timed_as(client, &subscription_state_call(profile), BILLING_PORTAL_TIMEOUT).await
}

/// `subscription.preview`: the chargeless quote for moving to `subscription_type_id` (effect,
/// proration, amount due now). Run it before [`subscription_upgrade`] so the confirm dialog shows
/// the real price. **Network** (portal); charges nothing. Builder + unit test only.
pub fn subscription_preview_call(subscription_type_id: &str, profile: &str) -> Call {
    call_of(
        "subscription.preview",
        params(&[
            ("profile", profile_param(profile)),
            ("subscription_type_id", opt_str(Some(subscription_type_id))),
        ]),
    )
}

/// Quote a plan change. **Network** (portal); no money moves.
pub async fn subscription_preview(
    client: &Client,
    subscription_type_id: &str,
    profile: &str,
) -> Result<SubscriptionPreview, ApiError> {
    call_timed_as(
        client,
        &subscription_preview_call(subscription_type_id, profile),
        BILLING_PORTAL_TIMEOUT,
    )
    .await
}

/// `subscription.change`: schedule a downgrade / same-price change (`subscription_type_id`) or a
/// period-end cancellation (`cancel: true`). **Changes the plan** and can re-enable recurring
/// spend; nothing is charged now (that is [`subscription_upgrade`]). The handler refuses when
/// neither argument is set. Builder + unit test only.
pub fn subscription_change_call(subscription_type_id: Option<&str>, cancel: bool, profile: &str) -> Call {
    call_of(
        "subscription.change",
        params(&[
            ("profile", profile_param(profile)),
            (
                "subscription_type_id",
                opt_str(subscription_type_id.filter(|_| !cancel)),
            ),
            ("cancel", json!(cancel)),
        ]),
    )
}

/// Schedule a plan change / cancellation. **Changes the plan** (recurring spend can resume).
pub async fn subscription_change(
    client: &Client,
    subscription_type_id: Option<&str>,
    cancel: bool,
    profile: &str,
) -> Result<SubscriptionPendingChange, ApiError> {
    call_timed_as(
        client,
        &subscription_change_call(subscription_type_id, cancel, profile),
        BILLING_PORTAL_TIMEOUT,
    )
    .await
}

/// `subscription.resume`: clear a scheduled downgrade or cancellation — the plan keeps renewing.
/// **Changes the plan and re-enables recurring spend** (the next cycle charges the card).
/// Builder + unit test only.
pub fn subscription_resume_call(profile: &str) -> Call {
    call_of("subscription.resume", params(&[("profile", profile_param(profile))]))
}

/// Clear a scheduled change. **Changes the plan** (recurring spend resumes).
pub async fn subscription_resume(client: &Client, profile: &str) -> Result<SubscriptionPendingChange, ApiError> {
    call_timed_as(client, &subscription_resume_call(profile), BILLING_PORTAL_TIMEOUT).await
}

/// `subscription.upgrade`: the money route — prorate, charge the card and flip the plan.
/// `status: requires_action` / `payment_failed` means the charge needs the portal at
/// `recovery_url`; only `ok` means the plan changed.
///
/// **Charges money and changes the plan.** Reuse `idempotency_key` on a retry
/// ([`new_idempotency_key`]); check a caller-supplied key with [`require_idempotency_key`] when
/// not going through [`subscription_upgrade`]. Builder + unit test only.
pub fn subscription_upgrade_call(subscription_type_id: &str, idempotency_key: &str, profile: &str) -> Call {
    call_of(
        "subscription.upgrade",
        params(&[
            ("profile", profile_param(profile)),
            ("subscription_type_id", opt_str(Some(subscription_type_id))),
            ("idempotency_key", opt_str(Some(idempotency_key))),
        ]),
    )
}

/// Upgrade the plan now. **Charges money and changes the plan.**
pub async fn subscription_upgrade(
    client: &Client,
    subscription_type_id: &str,
    idempotency_key: &str,
    profile: &str,
) -> Result<SubscriptionUpgrade, ApiError> {
    require_idempotency_key(idempotency_key)?;
    call_timed_as(
        client,
        &subscription_upgrade_call(subscription_type_id, idempotency_key, profile),
        BILLING_PORTAL_TIMEOUT,
    )
    .await
}

// ── Free tier (local) ───────────────────────────────────────────────────────

/// `free_tier.status`: guest identity, whether the welcome trial carries inference, the pending
/// notice, and a still-pending browser challenge.
///
/// This one is a **pure local read** — the handler answers from the auth store and creates no
/// identity as a side effect (`methods_free_tier.py`: "zero network and zero side effects"), so it
/// is the only method in this module the live test calls. Its failures are JSON-RPC errors
/// (`5090`), so they surface as [`ApiError`].
pub fn free_tier_status_call(profile: &str) -> Call {
    call_of("free_tier.status", params(&[("profile", profile_param(profile))]))
}

/// Read free-tier status. Local; safe to poll.
pub async fn free_tier_status(client: &Client, profile: &str) -> Result<FreeTierStatus, ApiError> {
    call_as(client, &free_tier_status_call(profile)).await
}

/// `free_tier.provision`: retry a welcome-trial claim that failed. **Network** — claims through
/// the Nous portal (blocking, one attempt per mint cooldown) and can change which identity this
/// machine bills to; it charges nothing. Builder + unit test only.
pub fn free_tier_provision_call(profile: &str) -> Call {
    call_of("free_tier.provision", params(&[("profile", profile_param(profile))]))
}

/// Retry the welcome-trial claim. **Network + writes the identity**; never run this in a test.
pub async fn free_tier_provision(client: &Client, profile: &str) -> Result<FreeTierProvision, ApiError> {
    // Pooled blocking portal claim (`_LONG_HANDLERS`); the 30 s default can expire mid-claim.
    call_timed_as(client, &free_tier_provision_call(profile), BILLING_PORTAL_TIMEOUT).await
}

/// `free_tier.claim_nudge`: claim the due post-task sign-in offer, so two windows never both show
/// it. **Writes** the offer state (local, no network); builder + unit test only because it is a
/// one-shot write the live test must not consume.
pub fn free_tier_claim_nudge_call(profile: &str) -> Call {
    call_of("free_tier.claim_nudge", params(&[("profile", profile_param(profile))]))
}

/// Claim the nudge. **Writes** the offer state (local).
pub async fn free_tier_claim_nudge(client: &Client, profile: &str) -> Result<FreeTierClaimNudge, ApiError> {
    call_as(client, &free_tier_claim_nudge_call(profile)).await
}

/// `free_tier.challenge_result`: tell the backend how the browser challenge it announced as
/// [`FREE_TIER_CHALLENGE_EVENT`] ended (`attempt` is that payload's `attempt`, so a stale window
/// cannot resolve a newer challenge). **Writes** the pending attempt; the backend still exchanges
/// with the portal itself. Builder + unit test only.
pub fn free_tier_challenge_result_call(url: &str, attempt: i64, outcome: ChallengeOutcome, profile: &str) -> Call {
    call_of(
        "free_tier.challenge_result",
        params(&[
            ("profile", profile_param(profile)),
            ("url", json!(url)),
            ("attempt", json!(attempt)),
            ("outcome", json!(outcome.as_str())),
        ]),
    )
}

/// Report a challenge outcome. **Writes** the pending attempt (local).
pub async fn free_tier_challenge_result(
    client: &Client,
    url: &str,
    attempt: i64,
    outcome: ChallengeOutcome,
    profile: &str,
) -> Result<FreeTierChallengeResult, ApiError> {
    call_as(client, &free_tier_challenge_result_call(url, attempt, outcome, profile)).await
}

/// `free_tier.ack_notice`: persist that the free-tier availability notice was shown, on the
/// identity itself (so it dies with that identity). **Writes** a local flag.
pub fn free_tier_ack_notice_call(profile: &str) -> Call {
    call_of("free_tier.ack_notice", params(&[("profile", profile_param(profile))]))
}

/// Acknowledge the notice. **Writes** the flag (local); builder + unit test only.
pub async fn free_tier_ack_notice(client: &Client, profile: &str) -> Result<FreeTierAck, ApiError> {
    call_as(client, &free_tier_ack_notice_call(profile)).await
}

// ── Onboarding (the method the other areas did not wrap) ────────────────────

/// `onboarding.set_run`: latch whether the first-run flow should open (`run`), optionally
/// recording that the profile-build offer was made (`mark_profile_offered`). Returns the new run
/// state (same shape as [`crate::api::system::onboarding_state`]).
///
/// Unscoped on purpose — it writes the **root** profile's `config.yaml` whichever profile the
/// backend serves, and its strict param model rejects a `profile` key, so none is sent.
/// **Writes config**; builder + unit test only, never live.
pub fn onboarding_set_run_call(run: bool, mark_profile_offered: bool) -> Call {
    call_of(
        "onboarding.set_run",
        params(&[
            ("run", json!(run)),
            ("mark_profile_offered", json!(mark_profile_offered)),
        ]),
    )
}

/// Set the onboarding run flag. **Writes the root config.**
pub async fn onboarding_set_run(
    client: &Client,
    run: bool,
    mark_profile_offered: bool,
) -> Result<OnboardingState, ApiError> {
    call_as(client, &onboarding_set_run_call(run, mark_profile_offered)).await
}

// ── Notifications ───────────────────────────────────────────────────────────

/// `billing.step_up.verification`: the device flow's URL and code, sent on the `session_id` the
/// caller passed to [`billing_step_up`] (blank when it passed none). Open `verification_url` in
/// the user's browser and show `user_code`; the RPC returns when they finish.
pub const BILLING_STEP_UP_VERIFICATION_EVENT: &str = "billing.step_up.verification";

/// Payload of [`BILLING_STEP_UP_VERIFICATION_EVENT`].
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BillingStepUpVerification {
    /// **Open this in the browser** (the gateway never does: stdout is its RPC pipe).
    pub verification_url: String,
    /// The code the user confirms on that page.
    pub user_code: String,
}

/// Parse a [`BILLING_STEP_UP_VERIFICATION_EVENT`] payload; `None` for any other event.
pub fn parse_billing_step_up_verification(event_type: &str, payload: &Value) -> Option<BillingStepUpVerification> {
    if event_type != BILLING_STEP_UP_VERIFICATION_EVENT {
        return None;
    }
    serde_json::from_value(payload.clone()).ok()
}

/// `free_tier.challenge`: the account service needs a browser challenge answered (broadcast, no
/// session). Show [`FreeTierChallenge::message`], open the URL, then report back with
/// [`free_tier_challenge_result`] (echoing `attempt`).
pub const FREE_TIER_CHALLENGE_EVENT: &str = "free_tier.challenge";

/// Parse a [`FREE_TIER_CHALLENGE_EVENT`] payload; `None` for any other event.
pub fn parse_free_tier_challenge(event_type: &str, payload: &Value) -> Option<FreeTierChallenge> {
    if event_type != FREE_TIER_CHALLENGE_EVENT {
        return None;
    }
    serde_json::from_value(payload.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn billing_builders() {
        assert_eq!(billing_state_call(""), call_of("billing.state", json!({})));
        assert_eq!(
            billing_state_call("swe"),
            call_of("billing.state", json!({"profile": "swe"}))
        );

        assert_eq!(
            billing_auto_reload_call(true, Some("5.00"), Some("25.00"), "swe"),
            call_of(
                "billing.auto_reload",
                json!({"profile": "swe", "enabled": true,
                       "threshold": "5.00", "top_up_amount": "25.00"})
            )
        );
        // Both amounts are required by the handler; omitted when absent.
        assert_eq!(
            billing_auto_reload_call(false, None, None, ""),
            call_of("billing.auto_reload", json!({"enabled": false}))
        );

        assert_eq!(
            billing_charge_call("10.00", "key-1", ""),
            call_of(
                "billing.charge",
                json!({"amount_usd": "10.00", "idempotency_key": "key-1"})
            )
        );
        assert_eq!(
            billing_charge_status_call("ch_123", "swe"),
            call_of(
                "billing.charge_status",
                json!({"profile": "swe", "charge_id": "ch_123"})
            )
        );
        assert_eq!(
            billing_step_up_call(Some("sess-1"), ""),
            call_of("billing.step_up", json!({"session_id": "sess-1"}))
        );
        assert_eq!(billing_step_up_call(None, ""), call_of("billing.step_up", json!({})));
    }

    #[test]
    fn subscription_builders() {
        assert_eq!(subscription_state_call(""), call_of("subscription.state", json!({})));
        assert_eq!(
            subscription_preview_call("tier_plus", ""),
            call_of("subscription.preview", json!({"subscription_type_id": "tier_plus"}))
        );
        assert_eq!(
            subscription_change_call(Some("tier_lite"), false, ""),
            call_of(
                "subscription.change",
                json!({"subscription_type_id": "tier_lite", "cancel": false})
            )
        );
        assert_eq!(
            subscription_change_call(None, true, "swe"),
            call_of("subscription.change", json!({"profile": "swe", "cancel": true}))
        );
        // `cancel` wins over a stray tier id, as the handler does.
        assert_eq!(
            subscription_change_call(Some("tier_lite"), true, ""),
            call_of("subscription.change", json!({"cancel": true}))
        );
        assert_eq!(subscription_resume_call(""), call_of("subscription.resume", json!({})));
        assert_eq!(
            subscription_upgrade_call("tier_max", "key-2", ""),
            call_of(
                "subscription.upgrade",
                json!({"subscription_type_id": "tier_max", "idempotency_key": "key-2"})
            )
        );
    }

    #[test]
    fn free_tier_builders() {
        assert_eq!(free_tier_status_call(""), call_of("free_tier.status", json!({})));
        assert_eq!(
            free_tier_provision_call("swe"),
            call_of("free_tier.provision", json!({"profile": "swe"}))
        );
        assert_eq!(
            free_tier_claim_nudge_call(""),
            call_of("free_tier.claim_nudge", json!({}))
        );
        assert_eq!(
            free_tier_ack_notice_call(""),
            call_of("free_tier.ack_notice", json!({}))
        );
        assert_eq!(
            free_tier_challenge_result_call("https://portal.example/challenge?x=1", 2, ChallengeOutcome::Closed, ""),
            call_of(
                "free_tier.challenge_result",
                json!({"url": "https://portal.example/challenge?x=1",
                       "attempt": 2, "outcome": "closed"})
            )
        );
        assert_eq!(ChallengeOutcome::Done.as_str(), "done");
        assert_eq!(ChallengeOutcome::Unsupported.as_str(), "unsupported");
    }

    #[test]
    fn onboarding_set_run_builder() {
        assert_eq!(
            onboarding_set_run_call(true, false),
            call_of(
                "onboarding.set_run",
                json!({"run": true, "mark_profile_offered": false})
            )
        );
        // No `profile`: the strict param model rejects one.
        assert!(onboarding_set_run_call(false, true).params.get("profile").is_none());
    }

    #[test]
    fn billing_state_decodes_and_defaults() {
        let state: BillingState = decode(json!({
            "ok": true, "logged_in": true, "free_tier_account": false, "free_tier_model": null,
            "org_name": "Acme", "org_slug": "acme", "role": "owner", "is_admin": true,
            "can_change_plan": true, "can_charge": true,
            "balance_usd": "42.10", "balance_display": "$42.10",
            "cli_billing_enabled": true,
            "charge_presets": ["10", "25"], "charge_presets_display": ["$10.00", "$25.00"],
            "min_usd": "5", "max_usd": "500",
            "card": {"brand": "visa", "last4": "4242", "masked": "•••• 4242",
                     "display": "Visa •••• 4242", "resolved_via": "account"},
            "payment_method": {"kind": "card", "brand": "visa", "last4": "4242",
                               "wallet": "apple_pay", "resolved_via": "methods"},
            "monthly_cap": {"limit_usd": "100", "limit_display": "$100.00",
                            "spent_this_month_usd": "57.9", "spent_display": "$57.90",
                            "is_default_ceiling": false},
            "auto_reload": {"enabled": true, "threshold_usd": "5", "threshold_display": "$5.00",
                            "reload_to_usd": "25", "reload_to_display": "$25.00",
                            "card": {"kind": "distinct", "payment_method_id": "pm_1",
                                     "brand": "visa", "last4": "4242"}},
            "portal_url": "https://portal.example/billing", "error": null,
            "usage": {"ok": true, "available": true, "status": "active", "plan_name": "Plus",
                      "has_topup": true,
                      "plan_bar": {"kind": "plan", "remaining_display": "$12.00",
                                   "total_display": "$20.00", "spent_display": "$8.00",
                                   "pct_used": 40, "fill_fraction": 0.4}}
        }))
        .unwrap();
        assert_eq!(state.balance_usd.as_deref(), Some("42.10"));
        assert_eq!(state.charge_presets, vec!["10", "25"]);
        assert_eq!(
            state.payment_method.as_ref().unwrap().wallet.as_deref(),
            Some("apple_pay")
        );
        let ar = state.auto_reload.clone().unwrap();
        assert_eq!(ar.card.unwrap().kind, "distinct");
        let usage = state.usage.unwrap();
        assert!(usage.available);
        assert_eq!(usage.plan_bar.unwrap().pct_used, Some(40));

        // The handler's `except` fallback shape, and a bare `{}`.
        let fallback: BillingState = decode(json!({
            "ok": true, "logged_in": false, "free_tier_account": false,
            "error": "could not load billing state"
        }))
        .unwrap();
        assert!(!fallback.logged_in);
        assert_eq!(fallback.error.as_deref(), Some("could not load billing state"));
        assert_eq!(decode::<BillingState>(json!({})).unwrap(), BillingState::default());
        assert!(decode::<BillingState>(json!([1])).is_err());
    }

    #[test]
    fn envelopes_decode() {
        // Refusal envelope from `_serialize_billing_error`.
        let refusal: BillingMutation = decode(json!({
            "ok": false, "error": "insufficient_scope", "message": "billing:manage required",
            "portal_url": "https://portal.example/step-up", "retry_after": 30,
            "payload": {"scope": "billing:manage"}, "actor": "admin", "code": "scope_required",
            "recovery": "ask_owner"
        }))
        .unwrap();
        assert!(!refusal.ok);
        assert_eq!(refusal.error.as_deref(), Some("insufficient_scope"));
        assert_eq!(refusal.retry_after, Some(30.0));
        assert_eq!(refusal.actor.as_deref(), Some("admin"));

        let charge: BillingCharge = decode(json!({
            "ok": true, "charge_id": "ch_123", "idempotency_key": "key-1"
        }))
        .unwrap();
        assert_eq!(charge.charge_id.as_deref(), Some("ch_123"));
        assert!(charge.envelope.ok);

        let failed: BillingCharge = decode(json!({
            "ok": false, "error": "payment_failed", "message": "card declined",
            "idempotency_key": "key-1"
        }))
        .unwrap();
        assert_eq!(failed.idempotency_key.as_deref(), Some("key-1"));
        assert!(!failed.envelope.ok);

        let status: BillingChargeStatus = decode(json!({
            "ok": true, "status": "settled", "amount_usd": "10.00",
            "settled_at": "2026-10-19T10:00:00Z", "reason": null
        }))
        .unwrap();
        assert_eq!(status.amount_usd.as_ref().unwrap().as_text(), "10.00");
        let numeric: BillingChargeStatus =
            decode(json!({"ok": true, "status": "pending", "amount_usd": 10.5})).unwrap();
        assert_eq!(numeric.amount_usd.unwrap().as_f64(), Some(10.5));

        let step: BillingStepUp = decode(json!({"ok": true, "granted": true})).unwrap();
        assert_eq!(step.granted, Some(true));
        let downscoped: BillingStepUp = decode(json!({"ok": false, "error": "error", "granted": false})).unwrap();
        assert_eq!(downscoped.granted, Some(false));

        assert!(!decode::<BillingMutation>(json!({})).unwrap().ok);
        assert!(!decode::<BillingCharge>(json!({})).unwrap().envelope.ok);
        assert!(!decode::<BillingChargeStatus>(json!({})).unwrap().envelope.ok);
        assert!(!decode::<BillingStepUp>(json!({})).unwrap().envelope.ok);
        assert!(!decode::<SubscriptionPreview>(json!({})).unwrap().envelope.ok);
        assert!(!decode::<SubscriptionUpgrade>(json!({})).unwrap().envelope.ok);
        assert!(!decode::<SubscriptionPendingChange>(json!({})).unwrap().envelope.ok);
    }

    #[test]
    fn subscription_decodes() {
        let state: SubscriptionState = decode(json!({
            "ok": true, "logged_in": true, "is_admin": true, "can_change_plan": true,
            "org_name": "Acme", "org_id": "org_1", "role": "owner", "context": "team",
            "current": {"tier_id": "tier_plus", "tier_name": "Plus", "monthly_credits": "2000",
                        "credits_remaining": "1500", "cycle_ends_at": "2026-11-01T00:00:00Z",
                        "pending_downgrade_tier_name": "Lite",
                        "pending_downgrade_at": "2026-11-01T00:00:00Z",
                        "pending_downgrade_display": "on Nov 1",
                        "cancel_at_period_end": false,
                        "cancellation_effective_at": null,
                        "cancellation_effective_display": null},
            "tiers": [{"tier_id": "tier_lite", "name": "Lite", "tier_order": 1,
                       "dollars_per_month_display": "$10", "monthly_credits": "1000",
                       "is_current": false, "is_enabled": true},
                      {"tier_id": "tier_plus", "name": "Plus", "tier_order": 2,
                       "dollars_per_month_display": "$20", "monthly_credits": null,
                       "is_current": true, "is_enabled": true}],
            "portal_url": "https://portal.example/subscription", "error": null,
            "usage": {"available": false}
        }))
        .unwrap();
        assert_eq!(state.context.as_deref(), Some("team"));
        let current = state.current.clone().unwrap();
        assert!(!current.cancel_at_period_end);
        assert_eq!(current.pending_downgrade_tier_name.as_deref(), Some("Lite"));
        assert_eq!(state.tiers.len(), 2);
        assert!(state.tiers[1].is_current);
        assert_eq!(
            state.usage.unwrap(),
            UsageBars {
                available: false,
                ..Default::default()
            }
        );

        let fallback: SubscriptionState = decode(json!({
            "ok": true, "logged_in": false, "error": "could not load subscription state"
        }))
        .unwrap();
        assert!(fallback.tiers.is_empty());
        assert_eq!(
            decode::<SubscriptionState>(json!({})).unwrap(),
            SubscriptionState::default()
        );

        let preview: SubscriptionPreview = decode(json!({
            "ok": true, "effect": "charge_now", "reason": null,
            "current_tier_id": "tier_lite", "current_tier_name": "Lite",
            "target_tier_id": "tier_plus", "target_tier_name": "Plus",
            "monthly_credits_delta": "1000", "amount_due_now_cents": 1450,
            "effective_at": "2026-10-19T00:00:00Z"
        }))
        .unwrap();
        assert_eq!(preview.effect.as_deref(), Some("charge_now"));
        assert_eq!(preview.amount_due_now_cents, Some(1450));

        let blocked: SubscriptionPreview =
            decode(json!({"ok": false, "error": "insufficient_scope", "message": "no"})).unwrap();
        assert_eq!(blocked.effect, None);
        assert!(!blocked.envelope.ok);

        let upgrade: SubscriptionUpgrade = decode(json!({
            "ok": true, "status": "requires_action", "target_tier_name": "Plus",
            "recovery_url": "https://portal.example/3ds", "reason": "sca",
            "idempotency_key": "key-2"
        }))
        .unwrap();
        assert_eq!(upgrade.status.as_deref(), Some("requires_action"));
        assert_eq!(upgrade.recovery_url.as_deref(), Some("https://portal.example/3ds"));

        let pending: SubscriptionPendingChange = decode(json!({
            "ok": true, "message": "Downgrade scheduled for Nov 1",
            "payload": {"effectiveAt": "2026-11-01T00:00:00Z"}
        }))
        .unwrap();
        assert!(pending.envelope.ok);
        assert!(pending.envelope.payload.is_some());
    }

    #[test]
    fn free_tier_decodes() {
        let status: FreeTierStatus = decode(json!({
            "has_guest": true, "enabled": true, "available": true, "notice_pending": true,
            "model": "hermes-3-405b", "label": "Nous free tier", "nudge_due_in": 3600,
            "challenge": {"type": "browser", "url": "https://portal.example/challenge",
                          "required": true, "expires_in": 300,
                          "message": "Confirm in your browser", "attempt": 1}
        }))
        .unwrap();
        assert!(status.available);
        assert_eq!(status.nudge_due_in, Some(3600.0));
        let challenge = status.challenge.clone().unwrap();
        assert_eq!(challenge.kind, "browser");
        assert!(challenge.required);
        assert_eq!(challenge.attempt, Some(1));

        // No identity but claimable: the trial is enabled with the last mint failure.
        let blocked: FreeTierStatus = decode(json!({
            "has_guest": false, "enabled": true, "available": false, "notice_pending": false,
            "model": "m", "label": "l", "error": "trial already claimed",
            "error_code": "trial_taken", "retryable": false, "retry_after": null
        }))
        .unwrap();
        assert!(!blocked.has_guest);
        assert_eq!(blocked.error_code.as_deref(), Some("trial_taken"));
        assert_eq!(decode::<FreeTierStatus>(json!({})).unwrap(), FreeTierStatus::default());

        let provision: FreeTierProvision = decode(json!({
            "has_guest": false, "enabled": false, "error": "portal refused",
            "error_code": "refused", "retryable": true, "retry_after": 60
        }))
        .unwrap();
        assert_eq!(provision.retry_after, Some(60.0));
        assert_eq!(
            decode::<FreeTierProvision>(json!({})).unwrap(),
            FreeTierProvision::default()
        );

        assert!(decode::<FreeTierAck>(json!({"acked": true})).unwrap().acked);
        assert!(!decode::<FreeTierClaimNudge>(json!({"claimed": false})).unwrap().claimed);
        assert!(
            decode::<FreeTierChallengeResult>(json!({"accepted": true}))
                .unwrap()
                .accepted
        );
        for empty in [
            decode::<FreeTierAck>(json!({})).unwrap().acked,
            decode::<FreeTierClaimNudge>(json!({})).unwrap().claimed,
            decode::<FreeTierChallengeResult>(json!({})).unwrap().accepted,
        ] {
            assert!(!empty);
        }
    }

    #[test]
    fn notifications_parse() {
        let payload = json!({"verification_url": "https://portal.example/device",
                             "user_code": "ABCD-1234"});
        let v = parse_billing_step_up_verification(BILLING_STEP_UP_VERIFICATION_EVENT, &payload).unwrap();
        assert_eq!(v.user_code, "ABCD-1234");
        assert_eq!(v.verification_url, "https://portal.example/device");
        assert!(parse_billing_step_up_verification("billing.step_up.other", &payload).is_none());
        assert_eq!(
            parse_billing_step_up_verification(BILLING_STEP_UP_VERIFICATION_EVENT, &json!({})).unwrap(),
            BillingStepUpVerification::default()
        );

        let challenge = json!({"type": "browser", "url": "https://portal.example/c",
                               "required": true, "expires_in": 120,
                               "message": "Confirm", "attempt": 0});
        let c = parse_free_tier_challenge(FREE_TIER_CHALLENGE_EVENT, &challenge).unwrap();
        assert_eq!(c.url, "https://portal.example/c");
        assert_eq!(c.expires_in, 120);
        assert_eq!(c.attempt, Some(0));
        assert!(parse_free_tier_challenge("free_tier.status", &challenge).is_none());
    }

    #[test]
    fn idempotency_keys_are_unique_uuid_v4() {
        let a = new_idempotency_key();
        let b = new_idempotency_key();
        assert_ne!(a, b);
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), [8, 4, 4, 4, 12]);
        assert!(a.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
        assert!(parts[2].starts_with('4'), "version nibble: {a}");
        assert!(matches!(&parts[3][..1], "8" | "9" | "a" | "b"), "variant: {a}");
    }

    #[test]
    fn blank_idempotency_key_is_refused_locally() {
        // The gateway would mint a fresh key for "", so a retry could charge twice.
        assert!(matches!(require_idempotency_key(""), Err(ApiError::Invalid(_))));
        assert!(matches!(require_idempotency_key("  "), Err(ApiError::Invalid(_))));
        assert!(require_idempotency_key("k-1").is_ok());
    }

    #[test]
    fn nullable_lists_decode_as_empty() {
        let state: BillingState = decode(json!({"charge_presets": null})).unwrap();
        assert!(state.charge_presets.is_empty());
        let subs: SubscriptionState = decode(json!({"tiers": null})).unwrap();
        assert!(subs.tiers.is_empty());
    }

    #[test]
    fn onboarding_run_state_decodes() {
        let state: OnboardingState = decode(json!({
            "run": true, "eligible": true, "session_id": "sess-1", "run_id": "run-1"
        }))
        .unwrap();
        assert!(state.run && state.eligible);
        assert_eq!(state.run_id.as_deref(), Some("run-1"));
        assert_eq!(
            decode::<OnboardingState>(json!({})).unwrap(),
            OnboardingState::default()
        );
    }
}
