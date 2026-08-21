//! Verified webhook ingestion (hand-authored, user-owned) — the pipeline that
//! turns a provider notification into a settled gateway transaction WITHOUT
//! ever trusting an unverified byte.
//!
//! Pipeline (ADR-0021/0022; each numbered step refuses before any state write
//! that follows it):
//!
//! 1. **Resolve** the provider config from the webhook URL's provider id via
//!    `payment_gateway.resolve_webhook_target(uuid)` — a narrow SECURITY
//!    DEFINER function (the bare route has no company scope; this is the only
//!    privileged read, and it projects no secrets). Unknown or inactive config
//!    ⇒ refuse.
//! 2. **Codec lookup + scheme check** — the codec must exist and its
//!    [`VerificationScheme`] must match the scheme the route declared for this
//!    provider slug. A provider can never be silently re-schemed.
//! 3. **HmacRawBody** → read the `webhook_verify` credential through the
//!    [`CredentialReader`] port (unavailable ⇒ 503 fail-closed, zero writes),
//!    then constant-time verify. Failure ⇒ 401, zero writes.
//! 4. **ApiRefetch** → re-fetch the transaction through the [`StatusRefetch`]
//!    port. Transport ⇒ 503 (provider retry wanted, zero writes); provider
//!    "unknown transaction" ⇒ 422, zero writes.
//! 5. **Money gate** (ADR-0022): the AUTHORITY is the verified body
//!    (HmacRawBody) or the re-fetch response (ApiRefetch). `check_money` runs
//!    on the authority values (fee defaults to the recorded row's fee when the
//!    authority does not report one; net re-derived), and the authority gross
//!    must equal the recorded row's gross. Mismatch ⇒ 422, zero writes.
//!    Not-settled at the authority ⇒ 200 acknowledged-and-ignored.
//! 6. **Settle** inside `with_company_scope(Some(company_id))` via
//!    [`GatewayWriteService::settle_by_provider_tx_verified`] — the
//!    transition-CAS exactly-once settle with the money gate re-enforced and
//!    the raw payload stamped post-verify (first delivery wins).
//! 7. **Respond** `{settled, already_settled}` — idempotent on redelivery.
//!
//! The module ships no HTTP for this; composition mounts the bare route
//! (`POST /webhooks/payment-gateway/{provider}/{provider_id}`) whose handler
//! calls [`WebhookIngestService::ingest`]. The generic unverified
//! `/webhook/settle` stays for authenticated operator flows only.

use rust_decimal::Decimal;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

use backbone_orm::company_scope;

use super::gateway_codecs::{
    CredentialFetch, CredentialReader, GatewayCodecRegistry, GatewaySecret, NotificationCodec,
    ProviderTruth, RefetchError, StatusRefetch, VerificationScheme, VerifyRequest,
};
use super::gateway_gl::GlPostSink;
use super::gateway_write_service::{GatewayError, GatewayWriteService};

/// The resolved provider config (step 1). `credentials_ref` is a POINTER into
/// the credential store, never a secret.
#[derive(Debug, Clone)]
pub struct WebhookTarget {
    pub company_id: Uuid,
    pub code: String,
    pub credentials_ref: Option<String>,
    pub status: String,
}

/// What the ingest pipeline did.
#[derive(Debug, Clone)]
pub struct IngestOutcome {
    pub provider_code: String,
    pub provider_transaction_id: String,
    pub settled: bool,
    pub already_settled: bool,
    /// True when the notification was valid but not a settlement (pending,
    /// expired, denied …) — acknowledged, no state change.
    pub ignored: bool,
    pub reason: Option<String>,
}

/// Ingest refusals. Every variant maps to an HTTP status at composition; the
/// 401/422/503 classes are the fail-closed legs the probes pin.
#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("no provider config for this webhook: {0}")]
    UnknownProviderConfig(String),
    #[error("provider config is not active")]
    ProviderConfigInactive,
    #[error("no codec registered for provider '{0}'")]
    CodecMissing(String),
    #[error("codec scheme does not match the route's declared scheme for '{0}'")]
    SchemeMismatch(String),
    #[error("payload parse failed: {0}")]
    Parse(#[from] super::gateway_codecs::ParseError),
    #[error("signature verification failed: {0}")]
    VerificationFailed(String),
    #[error("credential unavailable, failing closed: {0}")]
    CredentialUnavailable(CredentialFetch),
    #[error("provider status re-fetch failed (transport): {0}")]
    RefetchTransport(String),
    #[error("provider status re-fetch failed (provider): {0}")]
    RefetchProvider(String),
    #[error("authority money disagrees with the recorded transaction: {0}")]
    AmountMismatch(String),
    #[error("settlement engine: {0}")]
    Engine(#[from] GatewayError),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

impl IngestError {
    pub fn http_status(&self) -> u16 {
        match self {
            IngestError::UnknownProviderConfig(_) => 404,
            IngestError::ProviderConfigInactive
            | IngestError::RefetchProvider(_)
            | IngestError::AmountMismatch(_) => 422,
            IngestError::Parse(_) => 400,
            IngestError::VerificationFailed(_) => 401,
            IngestError::CredentialUnavailable(_) | IngestError::RefetchTransport(_) => 503,
            IngestError::CodecMissing(_) | IngestError::SchemeMismatch(_) | IngestError::Db(_) => {
                500
            }
            IngestError::Engine(e) => e.http_status(),
        }
    }

    pub fn code(&self) -> String {
        match self {
            IngestError::UnknownProviderConfig(_) => "webhook_target_not_found".into(),
            IngestError::ProviderConfigInactive => "webhook_target_inactive".into(),
            IngestError::CodecMissing(_) => "codec_missing".into(),
            IngestError::SchemeMismatch(_) => "scheme_mismatch".into(),
            IngestError::Parse(_) => "payload_parse_failed".into(),
            IngestError::VerificationFailed(_) => "signature_verification_failed".into(),
            IngestError::CredentialUnavailable(_) => "credential_unavailable".into(),
            IngestError::RefetchTransport(_) => "refetch_transport".into(),
            IngestError::RefetchProvider(_) => "refetch_provider".into(),
            IngestError::AmountMismatch(_) => "amount_mismatch".into(),
            IngestError::Engine(e) => e.code(),
            IngestError::Db(_) => "internal_error".into(),
        }
    }
}

/// The verified-ingest orchestrator. Ports (credential reader, re-fetcher, fee
/// sink) are composition-provided; everything else is this module's.
pub struct WebhookIngestService {
    db_pool: PgPool,
    write: Arc<GatewayWriteService>,
    codecs: GatewayCodecRegistry,
    credentials: Arc<dyn CredentialReader>,
    refetcher: Arc<dyn StatusRefetch>,
    fee_sink: Arc<dyn GlPostSink>,
}

impl WebhookIngestService {
    pub fn new(
        db_pool: PgPool,
        write: Arc<GatewayWriteService>,
        codecs: GatewayCodecRegistry,
        credentials: Arc<dyn CredentialReader>,
        refetcher: Arc<dyn StatusRefetch>,
        fee_sink: Arc<dyn GlPostSink>,
    ) -> Self {
        Self {
            db_pool,
            write,
            codecs,
            credentials,
            refetcher,
            fee_sink,
        }
    }

    /// Resolve the provider config a webhook URL points at (step 1). Narrow,
    /// non-secret, SECURITY DEFINER — the one read a bare route may make
    /// without a company scope.
    pub async fn resolve_target(
        &self,
        provider_id: Uuid,
    ) -> Result<Option<WebhookTarget>, sqlx::Error> {
        let row = sqlx::query_as::<_, (Uuid, String, Option<String>, String)>(
            "SELECT company_id, code, credentials_ref, status \
             FROM payment_gateway.resolve_webhook_target($1)",
        )
        .bind(provider_id)
        .fetch_optional(&self.db_pool)
        .await?;
        Ok(row.map(
            |(company_id, code, credentials_ref, status)| WebhookTarget {
                company_id,
                code,
                credentials_ref,
                status,
            },
        ))
    }

    /// The full pipeline. `provider_slug` and `declared_scheme` come from the
    /// ROUTE (the URL segment + the scheme the composition declared for it);
    /// they pin which codec may serve this request.
    pub async fn ingest(
        &self,
        provider_slug: &str,
        provider_id: Uuid,
        declared_scheme: VerificationScheme,
        raw: &[u8],
        req: &VerifyRequest<'_>,
    ) -> Result<IngestOutcome, IngestError> {
        // 1. Resolve — unknown or inactive config refuses with zero writes.
        let target = self
            .resolve_target(provider_id)
            .await?
            .ok_or_else(|| IngestError::UnknownProviderConfig(provider_id.to_string()))?;
        if target.status != "active" {
            return Err(IngestError::ProviderConfigInactive);
        }
        if target.code != provider_slug {
            return Err(IngestError::SchemeMismatch(format!(
                "route slug '{provider_slug}' does not match provider config code '{}'",
                target.code
            )));
        }

        // 2. Codec + scheme declaration.
        let codec = self
            .codecs
            .lookup(&target.code)
            .ok_or_else(|| IngestError::CodecMissing(target.code.clone()))?;
        if codec.scheme() != declared_scheme {
            return Err(IngestError::SchemeMismatch(format!(
                "provider '{}' is {} but the route declares {:?}",
                codec.code(),
                match codec.scheme() {
                    VerificationScheme::HmacRawBody => "HmacRawBody",
                    VerificationScheme::ApiRefetch => "ApiRefetch",
                },
                declared_scheme
            )));
        }

        // Parse (pure). Malformed payload ⇒ refuse before verification matters.
        let notification = codec.parse(raw)?;

        // 3/4. Scheme branch.
        let authority: ProviderTruth = match codec.scheme() {
            VerificationScheme::HmacRawBody => {
                let credentials_ref = target.credentials_ref.clone().ok_or_else(|| {
                    IngestError::CredentialUnavailable(CredentialFetch {
                        code: "no_credentials_ref".into(),
                        message: "provider config carries no credentials_ref".into(),
                    })
                })?;
                let secret = self
                    .credentials
                    .read_secret(
                        target.company_id,
                        &credentials_ref,
                        super::gateway_codecs::PURPOSE_WEBHOOK_VERIFY,
                    )
                    .await
                    .map_err(IngestError::CredentialUnavailable)?;
                codec
                    .verify(raw, req, &secret)
                    .map_err(|e| IngestError::VerificationFailed(e.to_string()))?;
                // The verified body IS the authority. A non-settle event is
                // acknowledged and ignored — never a write.
                match &notification.event {
                    super::gateway_codecs::NotificationEvent::SettleHint { gross_hint } => {
                        ProviderTruth {
                            settled: true,
                            gross: *gross_hint,
                            fee: None,
                        }
                    }
                    super::gateway_codecs::NotificationEvent::Ignore { reason } => {
                        return Ok(IngestOutcome {
                            provider_code: notification.provider_code.to_string(),
                            provider_transaction_id: notification.provider_transaction_id,
                            settled: false,
                            already_settled: false,
                            ignored: true,
                            reason: Some(reason.clone()),
                        })
                    }
                    super::gateway_codecs::NotificationEvent::StatusHint => {
                        return Ok(IngestOutcome {
                            provider_code: notification.provider_code.to_string(),
                            provider_transaction_id: notification.provider_transaction_id,
                            settled: false,
                            already_settled: false,
                            ignored: true,
                            reason: Some("status hint — no settlement claimed".into()),
                        })
                    }
                }
            }
            VerificationScheme::ApiRefetch => {
                // The payload is a nudge; the provider's API is the only
                // authority. Amounts in the payload are never read for booking.
                self.refetcher
                    .fetch(
                        target.company_id,
                        &target.code,
                        &notification.provider_transaction_id,
                    )
                    .await
                    .map_err(|e| match e {
                        RefetchError::Transport(m) => IngestError::RefetchTransport(m),
                        RefetchError::Provider(m) => IngestError::RefetchProvider(m),
                    })?
            }
        };

        // Non-settled at the authority ⇒ acknowledged, ignored (200 at the edge).
        if !authority.settled {
            return Ok(IngestOutcome {
                provider_code: notification.provider_code.to_string(),
                provider_transaction_id: notification.provider_transaction_id,
                settled: false,
                already_settled: false,
                ignored: true,
                reason: Some("provider reports the transaction not settled".into()),
            });
        }

        // 5+6. Money-gated, transition-CAS settle inside the resolved company
        // scope. Money failures surface as AmountMismatch (422, zero writes).
        let raw_payload: serde_json::Value =
            serde_json::from_slice(raw).unwrap_or(serde_json::Value::Null);
        let outcome = company_scope::with_company_scope(Some(target.company_id), async {
            self.write
                .settle_by_provider_tx_verified(
                    target.company_id,
                    &target.code,
                    &notification.provider_transaction_id,
                    authority.gross,
                    authority.fee,
                    if raw_payload.is_null() {
                        None
                    } else {
                        Some(raw_payload)
                    },
                    &*self.fee_sink,
                )
                .await
        })
        .await
        .map_err(|e| match e {
            GatewayError::InvalidMoney(m) => IngestError::AmountMismatch(m),
            other => IngestError::Engine(other),
        })?;

        // 7. Respond.
        Ok(IngestOutcome {
            provider_code: notification.provider_code.to_string(),
            provider_transaction_id: notification.provider_transaction_id,
            settled: !outcome.already_settled,
            already_settled: outcome.already_settled,
            ignored: false,
            reason: None,
        })
    }

    /// Expose the write service (composition's ACL drives link_payment_entry
    /// through the same engine).
    pub fn write_service(&self) -> &Arc<GatewayWriteService> {
        &self.write
    }
}
