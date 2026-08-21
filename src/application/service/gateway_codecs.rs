//! Provider notification codecs (hand-authored, user-owned) — ADR-0021's
//! verification world made concrete, plus the two I/O ports the verified
//! ingest pipeline needs from composition.
//!
//! A codec is PURE: parse maps a raw payload to the normalized notification
//! shape; verify checks a raw-body HMAC against a caller-supplied secret.
//! Neither touches the network, the DB, or the clock. The two schemes allowed
//! by ADR-0021 (and nothing else):
//!
//! * [`VerificationScheme::HmacRawBody`] — the signature is a keyed digest over
//!   the exact raw bytes the server received (DOKU). Verification is
//!   constant-time and fail-closed; a missing/malformed signature is a
//!   verification FAILURE, not a skip.
//! * [`VerificationScheme::ApiRefetch`] — the payload's authenticity is NOT
//!   derivable from the bytes (field-subset signatures, static shared tokens);
//!   the only authoritative read is re-fetching the transaction from the
//!   provider API (Midtrans, Xendit). `verify` is a no-op for these codecs BY
//!   DESIGN — the re-fetch IS the verification, and the ingest pipeline never
//!   trusts the payload's own amounts for them.
//!
//! There is deliberately NO "no-verify"/demo codec. An unverified payload can
//! never reach a state write through this path.
//!
//! Credential/secret types crossing the [`CredentialReader`] port are edge-free
//! (no sapiens types) so this module keeps zero Cargo edges to the credential
//! store; composition adapts the port to its credential service.

use axum::http::HeaderMap;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// The two verification schemes ADR-0021 admits for provider webhooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VerificationScheme {
    /// Signature = HMAC over the exact raw request bytes (constant-time check).
    HmacRawBody,
    /// Payload is untrusted; the provider's status API is the only authority.
    ApiRefetch,
}

/// Purpose labels for [`CredentialReader`] — the credential store's purpose
/// enum as plain strings so this module carries no store types.
pub const PURPOSE_WEBHOOK_VERIFY: &str = "webhook_verify";
pub const PURPOSE_API_READ: &str = "api_read";

/// A secret read through the credential port. Redacted in Debug so it cannot
/// drift into a log line by accident.
pub struct GatewaySecret(String);

impl GatewaySecret {
    pub fn new(s: String) -> Self {
        Self(s)
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for GatewaySecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GatewaySecret([REDACTED])")
    }
}

/// Why a credential could not be read. The ingest pipeline treats every variant
/// as fail-closed (503, zero writes) — a missing/revoked credential must never
/// degrade into accepting an unverified payload.
#[derive(Debug, Clone, thiserror::Error)]
#[error("credential unavailable ({code}): {message}")]
pub struct CredentialFetch {
    pub code: String,
    pub message: String,
}

/// The read port into the composition's credential store (adapter over the
/// store's `read_secret`; `account_ref` is the provider-config pointer stored
/// on `PaymentGatewayProvider.credentials_ref`).
#[async_trait::async_trait]
pub trait CredentialReader: Send + Sync {
    async fn read_secret(
        &self,
        company_id: Uuid,
        account_ref: &str,
        purpose: &str,
    ) -> Result<GatewaySecret, CredentialFetch>;
}

/// The authoritative state a re-fetch returned.
#[derive(Debug, Clone)]
pub struct ProviderTruth {
    /// Whether the provider reports the transaction as SETTLED (money moved).
    pub settled: bool,
    /// The provider's authoritative gross amount.
    pub gross: Decimal,
    /// The provider's fee, when it reports one (Midtrans does not at this leg).
    pub fee: Option<Decimal>,
}

/// Why a re-fetch failed. `Transport` ⇒ the caller should retry (503,
/// zero writes — a transient outage must not lose the notification, and the
/// provider's retry redelivers it). `Provider` ⇒ the provider itself answered
/// "unknown transaction" (422, zero writes).
#[derive(Debug, Clone, thiserror::Error)]
pub enum RefetchError {
    #[error("refetch transport error: {0}")]
    Transport(String),
    #[error("provider reported: {0}")]
    Provider(String),
}

/// The re-fetch port: composition implements it with the provider's status API
/// (Midtrans `GET /v2/{order}/status`, Xendit `GET /v2/invoices/{id}`), reading
/// its own credentials through [`CredentialReader`].
#[async_trait::async_trait]
pub trait StatusRefetch: Send + Sync {
    async fn fetch(
        &self,
        company_id: Uuid,
        provider_code: &str,
        provider_transaction_id: &str,
    ) -> Result<ProviderTruth, RefetchError>;
}

/// What a normalized notification wants the pipeline to do.
#[derive(Debug, Clone, PartialEq)]
pub enum NotificationEvent {
    /// The provider claims settlement; `gross_hint` is the payload's own
    /// amount — a HINT only. The pipeline gates on the authoritative amount
    /// (post-verify body for HmacRawBody, re-fetch for ApiRefetch) versus the
    /// recorded row.
    SettleHint { gross_hint: Decimal },
    /// A non-terminal lifecycle update — acknowledged, no state change.
    StatusHint,
    /// Not actionable (pending / expired / denied / refund …). Carries the
    /// reason for the audit trail.
    Ignore { reason: String },
}

/// The provider-agnostic shape every codec emits.
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedNotification {
    /// "manual" | "midtrans" | "xendit" | "doku" — must match the route the
    /// composition declared for this codec.
    pub provider_code: &'static str,
    /// The provider's own transaction id — the dedup key.
    pub provider_transaction_id: String,
    /// The provider's secondary identifier for the charge, when the payload
    /// carries one (Midtrans `transaction_id` — the per-attempt id; Xendit
    /// `external_id` — the merchant's own reference) — diagnostic only.
    pub order_reference: Option<String>,
    pub event: NotificationEvent,
}

/// Verification failure. Every variant means REFUSE — there is no scheme under
/// which a verification failure is ignored or downgraded.
#[derive(Debug, Clone, thiserror::Error)]
pub enum VerifyError {
    #[error("signature header missing")]
    MissingSignature,
    #[error("signature malformed (not 64 hex chars)")]
    MalformedSignature,
    #[error("required digest headers missing: {0:?}")]
    MissingHeaders(Vec<&'static str>),
    #[error("signature mismatch")]
    Mismatch,
}

/// Parse failure — malformed payload, missing ids. Refused before any
/// verification or state read matters (422).
#[derive(Debug, Clone, thiserror::Error)]
pub enum ParseError {
    #[error("payload is not valid provider JSON: {0}")]
    Malformed(String),
    #[error("payload carries no usable transaction id")]
    MissingTransactionId,
}

/// Everything a codec needs about the request besides its bytes: the raw
/// headers and the exact request target (path + query) the server saw — DOKU's
/// digest includes both.
pub struct VerifyRequest<'a> {
    pub headers: &'a HeaderMap,
    pub request_target: &'a str,
}

/// A provider notification codec — parse + verify contracts, pure.
pub trait NotificationCodec: Send + Sync {
    /// The provider slug this codec handles (matches the route segment and the
    /// provider-config `code`).
    fn code(&self) -> &'static str;
    /// The verification scheme this provider's notifications are admitted
    /// under. The ingest pipeline refuses a codec whose scheme does not match
    /// the route's declaration — a provider can never be silently re-schemed.
    fn scheme(&self) -> VerificationScheme;
    /// Parse the raw payload into the normalized shape. For ApiRefetch codecs
    /// the parsed amounts are HINTS; the pipeline never books them.
    fn parse(&self, raw: &[u8]) -> Result<NormalizedNotification, ParseError>;
    /// Verify the raw request. Only meaningful for [`VerificationScheme::HmacRawBody`]
    /// (ApiRefetch codecs return Ok — the re-fetch is the verification).
    fn verify(
        &self,
        raw: &[u8],
        req: &VerifyRequest<'_>,
        secret: &GatewaySecret,
    ) -> Result<(), VerifyError>;
}

fn parse_json(raw: &[u8]) -> Result<serde_json::Value, ParseError> {
    serde_json::from_slice(raw).map_err(|e| ParseError::Malformed(e.to_string()))
}

/// `"1000000.00"` / `1000000` / `1000000.00` (string or number) → Decimal.
fn amount(v: &serde_json::Value) -> Option<Decimal> {
    match v {
        serde_json::Value::Number(n) => Decimal::from_str_exact(&n.to_string()).ok(),
        serde_json::Value::String(s) => Decimal::from_str_exact(s.trim()).ok(),
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DOKU — raw-body HMAC (scheme #1)
// ─────────────────────────────────────────────────────────────────────────────

/// DOKU (Jokul) HTTP notifications.
///
/// Digest construction (pinned by `pgc0_doku_signature_construction_matches_docs`):
/// the `Signature` header is `lowercase(hex(HMAC-SHA256(secret,
/// client_id ":" request_id ":" lowercase(request_timestamp) ":"
/// lowercase(request_target) ":" raw_body)))`, where the header names are
/// `Client-Id`, `Request-Id`, `Request-Timestamp` and the body is the exact raw
/// byte string received. Comparison is constant-time; a non-64-hex signature
/// is malformed (refused), never compared loosely.
///
/// Payload shape (VA/payment-code family): `order.amount`, `order.invoice_number`,
/// `transaction.id`, `transaction.status` — `SETTLE` is the settlement event.
pub struct DokuCodec;

impl NotificationCodec for DokuCodec {
    fn code(&self) -> &'static str {
        "doku"
    }

    fn scheme(&self) -> VerificationScheme {
        VerificationScheme::HmacRawBody
    }

    fn parse(&self, raw: &[u8]) -> Result<NormalizedNotification, ParseError> {
        let v = parse_json(raw)?;
        let txn_id = v
            .pointer("/transaction/id")
            .and_then(|t| t.as_str())
            .filter(|s| !s.is_empty())
            .ok_or(ParseError::MissingTransactionId)?;
        let status = v
            .pointer("/transaction/status")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let gross_hint = v
            .pointer("/order/amount")
            .and_then(amount)
            .unwrap_or(Decimal::ZERO);
        let event = match status.to_ascii_uppercase().as_str() {
            "SETTLE" => NotificationEvent::SettleHint { gross_hint },
            "PENDING" => NotificationEvent::Ignore {
                reason: "payment not yet settled".into(),
            },
            other => NotificationEvent::Ignore {
                reason: format!("doku status {other}"),
            },
        };
        Ok(NormalizedNotification {
            provider_code: "doku",
            provider_transaction_id: txn_id.to_string(),
            order_reference: v
                .pointer("/order/invoice_number")
                .and_then(|s| s.as_str())
                .map(String::from),
            event,
        })
    }

    fn verify(
        &self,
        raw: &[u8],
        req: &VerifyRequest<'_>,
        secret: &GatewaySecret,
    ) -> Result<(), VerifyError> {
        use hmac::Mac;
        type HmacSha256 = hmac::Hmac<sha2::Sha256>;

        let sig_header = req
            .headers
            .get("signature")
            .and_then(|v| v.to_str().ok())
            .ok_or(VerifyError::MissingSignature)?;
        let sig_bytes = hex_decode_32(sig_header).ok_or(VerifyError::MalformedSignature)?;

        let missing: Vec<&'static str> = ["client-id", "request-id", "request-timestamp"]
            .into_iter()
            .filter(|h| {
                req.headers
                    .get(*h)
                    .and_then(|v| v.to_str().ok())
                    .map(str::is_empty)
                    .unwrap_or(true)
            })
            .collect();
        if !missing.is_empty() {
            return Err(VerifyError::MissingHeaders(missing));
        }
        let client_id = req
            .headers
            .get("client-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let request_id = req
            .headers
            .get("request-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let timestamp = req
            .headers
            .get("request-timestamp")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let mut mac = HmacSha256::new_from_slice(secret.as_str().as_bytes())
            .expect("HMAC accepts any key length");
        mac.update(
            format!(
                "{}:{}:{}:{}:",
                client_id,
                request_id,
                timestamp.to_lowercase(),
                req.request_target.to_lowercase()
            )
            .as_bytes(),
        );
        mac.update(raw);
        mac.verify_slice(&sig_bytes)
            .map_err(|_| VerifyError::Mismatch)
    }
}

/// Strict 32-byte hex decode (64 chars, [0-9a-fA-F] only).
fn hex_decode_32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hi = (chunk[0] as char).to_digit(16)?;
        let lo = (chunk[1] as char).to_digit(16)?;
        out[i] = ((hi << 4) | lo) as u8;
    }
    Some(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// Midtrans — API re-fetch (scheme #2)
// ─────────────────────────────────────────────────────────────────────────────

/// Midtrans HTTP notifications. Midtrans's `signature_key` is a sha512 over a
/// SUBSET of fields — a field-subset signature, banned by ADR-0021 rule 3 — so
/// the notification is treated as an untrusted nudge: parsed, never verified
/// from its bytes, and every amount/decision is taken from the re-fetched
/// status (`GET /v2/{order_id}/status` at composition).
pub struct MidtransCodec;

impl NotificationCodec for MidtransCodec {
    fn code(&self) -> &'static str {
        "midtrans"
    }

    fn scheme(&self) -> VerificationScheme {
        VerificationScheme::ApiRefetch
    }

    fn parse(&self, raw: &[u8]) -> Result<NormalizedNotification, ParseError> {
        let v = parse_json(raw)?;
        // The ORDER id is the dedup key, not transaction_id: the status API
        // (`GET /v2/{order_id}/status`) is order-keyed, and an order's retries
        // share the order id while minting fresh transaction ids — deduping on
        // transaction_id would split one order's redeliveries across rows and
        // the re-fetch leg would 404 on every one of them.
        let txn_id = v
            .get("order_id")
            .and_then(|t| t.as_str())
            .filter(|s| !s.is_empty())
            .ok_or(ParseError::MissingTransactionId)?;
        let status = v
            .get("transaction_status")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let gross_hint = v
            .get("gross_amount")
            .and_then(amount)
            .unwrap_or(Decimal::ZERO);
        let event = match status {
            "settlement" | "capture" => NotificationEvent::SettleHint { gross_hint },
            "pending" => NotificationEvent::Ignore {
                reason: "payment not yet settled".into(),
            },
            other => NotificationEvent::Ignore {
                reason: format!("midtrans status {other}"),
            },
        };
        Ok(NormalizedNotification {
            provider_code: "midtrans",
            provider_transaction_id: txn_id.to_string(),
            // The per-attempt provider transaction id — diagnostic only.
            order_reference: v
                .get("transaction_id")
                .and_then(|s| s.as_str())
                .map(String::from),
            event,
        })
    }

    fn verify(
        &self,
        _raw: &[u8],
        _req: &VerifyRequest<'_>,
        _secret: &GatewaySecret,
    ) -> Result<(), VerifyError> {
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Xendit — API re-fetch (scheme #2)
// ─────────────────────────────────────────────────────────────────────────────

/// Xendit callbacks. Authenticated by a STATIC `X-Callback-Token` (a shared
/// secret, not a body digest) — under ADR-0021 rule 5 that is a re-fetch
/// scheme: the token only proves the sender, so amounts and settlement state
/// come from `GET /v2/invoices/{id}` at composition. The token header is
/// deliberately ignored here.
pub struct XenditCodec;

impl NotificationCodec for XenditCodec {
    fn code(&self) -> &'static str {
        "xendit"
    }

    fn scheme(&self) -> VerificationScheme {
        VerificationScheme::ApiRefetch
    }

    fn parse(&self, raw: &[u8]) -> Result<NormalizedNotification, ParseError> {
        let v = parse_json(raw)?;
        let txn_id = v
            .get("id")
            .and_then(|t| t.as_str())
            .filter(|s| !s.is_empty())
            .ok_or(ParseError::MissingTransactionId)?;
        let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("");
        let gross_hint = v
            .get("paid_amount")
            .and_then(amount)
            .unwrap_or(Decimal::ZERO);
        let event = match status.to_ascii_uppercase().as_str() {
            "PAID" | "SETTLED" => NotificationEvent::SettleHint { gross_hint },
            other => NotificationEvent::Ignore {
                reason: format!("xendit status {other}"),
            },
        };
        Ok(NormalizedNotification {
            provider_code: "xendit",
            provider_transaction_id: txn_id.to_string(),
            order_reference: v
                .get("external_id")
                .and_then(|s| s.as_str())
                .map(String::from),
            event,
        })
    }

    fn verify(
        &self,
        _raw: &[u8],
        _req: &VerifyRequest<'_>,
        _secret: &GatewaySecret,
    ) -> Result<(), VerifyError> {
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Manual — authenticated-operator path only
// ─────────────────────────────────────────────────────────────────────────────

/// The manual flow's non-webhook codec. Manual settlements are keyed by an
/// operator through an authenticated route (the operator's session IS the
/// authorization — no bare route exists for `manual`), so its notifications
/// carry no signature at all. This codec is ApiRefetch-schemed so that if a
/// composition ever routes a manual notification through the verified ingest
/// pipeline, it degrades to a re-fetch — which for manual reports `pending`
/// and settles NOTHING. An unverified manual payload can never write state.
pub struct ManualNoopCodec;

impl NotificationCodec for ManualNoopCodec {
    fn code(&self) -> &'static str {
        "manual"
    }

    fn scheme(&self) -> VerificationScheme {
        VerificationScheme::ApiRefetch
    }

    fn parse(&self, raw: &[u8]) -> Result<NormalizedNotification, ParseError> {
        let v = parse_json(raw)?;
        let txn_id = v
            .get("provider_transaction_id")
            .and_then(|t| t.as_str())
            .filter(|s| !s.is_empty())
            .ok_or(ParseError::MissingTransactionId)?;
        let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("");
        let event = match status {
            "settled" => NotificationEvent::SettleHint {
                gross_hint: Decimal::ZERO,
            },
            other => NotificationEvent::Ignore {
                reason: format!("manual status {other}"),
            },
        };
        Ok(NormalizedNotification {
            provider_code: "manual",
            provider_transaction_id: txn_id.to_string(),
            order_reference: None,
            event,
        })
    }

    fn verify(
        &self,
        _raw: &[u8],
        _req: &VerifyRequest<'_>,
        _secret: &GatewaySecret,
    ) -> Result<(), VerifyError> {
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Registry
// ─────────────────────────────────────────────────────────────────────────────

/// `code → codec`. [`GatewayCodecRegistry::with_builtin`] registers the four
/// codecs above. Composition may add codecs, never remove the scheme contract.
#[derive(Clone, Default)]
pub struct GatewayCodecRegistry {
    codecs: Arc<HashMap<&'static str, Arc<dyn NotificationCodec>>>,
}

impl GatewayCodecRegistry {
    pub fn new() -> Self {
        Self {
            codecs: Arc::new(HashMap::new()),
        }
    }

    pub fn with_builtin() -> Self {
        let mut r = Self::new();
        r.register(Arc::new(DokuCodec));
        r.register(Arc::new(MidtransCodec));
        r.register(Arc::new(XenditCodec));
        r.register(Arc::new(ManualNoopCodec));
        r
    }

    pub fn register(&mut self, codec: Arc<dyn NotificationCodec>) {
        Arc::make_mut(&mut self.codecs).insert(codec.code(), codec);
    }

    pub fn lookup(&self, code: &str) -> Option<Arc<dyn NotificationCodec>> {
        self.codecs.get(code).cloned()
    }

    pub fn codes(&self) -> Vec<&'static str> {
        let mut c: Vec<_> = self.codecs.keys().copied().collect();
        c.sort_unstable();
        c
    }
}
