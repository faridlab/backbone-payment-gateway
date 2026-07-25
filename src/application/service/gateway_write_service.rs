//! Settlement engine for gateway transactions (hand-authored, user-owned) —
//! ADR-003 §3, §4, §5.
//!
//! On a gateway transaction's `pending → settled` transition this service:
//!   1. posts the gateway-fee companion journal (`Dr Fee Expense · Cr Bank`) via
//!      a [`GlPostSink`] — only when `fee_amount > 0` (zero-fee settlements
//!      skip the companion post); and
//!   2. emits [`GatewayTransactionSettled`] via a [`GatewayEventSink`] so the
//!      composition ACL creates a PaymentEntry (paid = gross) and routes into
//!      payment's existing settlement + billing drawdown.
//!
//! Exactly-once: the emission is gated on the transition UPDATE's
//! `rows_affected == 1`, so a concurrent double-settle or a redelivered webhook
//! settles + emits once — it never double-creates a PaymentEntry or double-posts
//! the fee. Payment is untouched (no regeneration, no Cargo edge).
//!
//! **Layering:** this service ORCHESTRATES (validates, owns the unit of work,
//! drives the sinks, publishes). It holds no SQL — every statement lives on
//! [`GatewayTransactionRepository`]'s custom methods, which take the caller's
//! transaction so the transition + emission commit as one unit.

use backbone_orm::company_scope;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

use crate::infrastructure::persistence::{
    compose_fee_post, FeeSourceRow, GatewayTransactionRepository,
};

use super::gateway_events::{
    GatewayEvent, GatewayEventSink, GatewayTransactionSettled, LoggingGatewaySink,
};
use super::gateway_gl::{AccountingPostEnvelope, GlPostSink, GlPostAck, GlPostRejected};

/// The outcome of settling a gateway transaction.
#[derive(Debug, Clone)]
pub struct SettleOutcome {
    pub gateway_transaction_id: Uuid,
    /// `true` if this call found the transaction already settled (idempotent re-entry).
    pub already_settled: bool,
    /// The fee companion post, if one was made (`fee_amount > 0` and accounts configured).
    pub fee_post: Option<GlPostAck>,
}

/// Settlement-path errors.
#[derive(Debug)]
pub enum GatewayError {
    NotFound(Uuid),
    /// The provider's fee/settlement GL accounts are not configured, so the fee
    /// post cannot be emitted. Surfaced only when a non-zero fee is present.
    FeeAccountsMissing,
    UnsupportedCurrency(String),
    FeeSinkRejected { code: String, message: String },
    Db(sqlx::Error),
}

impl GatewayError {
    pub fn code(&self) -> String {
        match self {
            GatewayError::NotFound(_) => "gateway_transaction_not_found".into(),
            GatewayError::FeeAccountsMissing => "fee_accounts_missing".into(),
            GatewayError::UnsupportedCurrency(_) => "unsupported_currency".into(),
            GatewayError::FeeSinkRejected { code, .. } => code.clone(),
            GatewayError::Db(_) => "internal_error".into(),
        }
    }
    pub fn http_status(&self) -> u16 {
        match self {
            GatewayError::NotFound(_) => 404,
            GatewayError::Db(_) => 500,
            _ => 422,
        }
    }
}
impl std::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GatewayError::FeeSinkRejected { code, message } => write!(f, "{code}: {message}"),
            other => write!(f, "{}", other.code()),
        }
    }
}
impl std::error::Error for GatewayError {}
impl From<sqlx::Error> for GatewayError {
    fn from(e: sqlx::Error) -> Self { GatewayError::Db(e) }
}

/// The write service. The fee sink is passed per-call (mirrors payment's
/// `GlPostSink`); the event sink is held (mirrors `PaymentEventSink`).
#[derive(Clone)]
pub struct GatewayWriteService {
    db_pool: PgPool,
    sink: Arc<dyn GatewayEventSink>,
    txns: Arc<GatewayTransactionRepository>,
}

impl GatewayWriteService {
    pub fn new(db_pool: PgPool) -> Self {
        Self::with_sink(db_pool, Arc::new(LoggingGatewaySink))
    }
    pub fn with_sink(db_pool: PgPool, sink: Arc<dyn GatewayEventSink>) -> Self {
        Self {
            txns: Arc::new(GatewayTransactionRepository::new(db_pool.clone())),
            db_pool,
            sink,
        }
    }

    /// Build (but do not post) the fee companion journal. `None` ⇒ nothing to post
    /// (zero fee, or the provider's GL accounts not yet configured). Pure DB read +
    /// the pure composer — no side effects. Exposed for inspection / testing.
    pub async fn build_fee_post(
        &self,
        gateway_transaction_id: Uuid,
    ) -> Result<Option<AccountingPostEnvelope>, GatewayError> {
        let src = self
            .txns
            .fetch_fee_source(&self.db_pool, gateway_transaction_id)
            .await?
            .ok_or(GatewayError::NotFound(gateway_transaction_id))?;
        if src.currency != "IDR" {
            return Err(GatewayError::UnsupportedCurrency(src.currency));
        }
        Ok(compose_fee_post(&src, chrono::Utc::now().date_naive()))
    }

    /// Settle a gateway transaction: transition `pending → settled`, post the fee
    /// companion journal (if any), and emit [`GatewayTransactionSettled`]. The
    /// emission is gated on the transition's `rows_affected == 1`, so a concurrent
    /// or repeated settle is a no-op that returns `already_settled: true` and emits
    /// nothing — exactly-once across the seam.
    pub async fn settle_transaction(
        &self,
        gateway_transaction_id: Uuid,
        fee_sink: &dyn GlPostSink,
    ) -> Result<SettleOutcome, GatewayError> {
        // One read: the transaction + its provider's GL accounts (scoped by the
        // request/inherited company). Resolved BEFORE the transition so a
        // missing-accounts / non-IDR failure leaves the row untouched.
        let src = self
            .txns
            .fetch_fee_source(&self.db_pool, gateway_transaction_id)
            .await?
            .ok_or(GatewayError::NotFound(gateway_transaction_id))?;
        if src.currency != "IDR" {
            return Err(GatewayError::UnsupportedCurrency(src.currency));
        }
        // Redelivery guard: a webhook for an already-terminal transaction is a no-op — return
        // idempotently WITHOUT re-posting the fee or re-emitting the seam event. (A genuine
        // *concurrent* race is still possible between this read and the transition UPDATE; there the
        // transition's `rows_affected == 1` gate below ensures the event fires once, and the fee post
        // is idempotent at accounting on `source_id` — the same posture payment takes for its GL post.)
        if src.status == "settled" || src.status == "refunded" {
            return Ok(SettleOutcome {
                gateway_transaction_id,
                already_settled: true,
                fee_post: None,
            });
        }
        let company_id = src.company_id;
        let fee_env = compose_fee_post(&src, chrono::Utc::now().date_naive());

        // Post the fee first (outside the transition tx — accounting is its own UoW
        // and dedups on source_id). A rejection marks posting_state=failed and aborts
        // the settle: a half-settled row (settled with no fee post) is worse than a
        // retryable failure. Payment's settlement post is NOT made here — the ACL
        // creates the PaymentEntry from the seam event.
        let fee_ack = match fee_env.as_ref() {
            Some(env) => match fee_sink.post(env).await {
                Ok(ack) => Some(ack),
                Err(GlPostRejected { code, message }) => {
                    let _ = self.txns.mark_fee_failed(&self.db_pool, gateway_transaction_id).await;
                    return Err(GatewayError::FeeSinkRejected { code, message });
                }
            },
            None => None,
        };

        // Transition + emit in ONE tx (crash-safe): the transition UPDATE and the
        // header read ride the same connection; the event fires only if THIS call
        // performed the transition (rows_affected == 1).
        let mut tx = self.db_pool.begin().await?;
        company_scope::bind_company_on(&mut tx, company_id).await?;
        let posting_state = if fee_ack.is_some() { "posted" } else { "pending" };
        let rows = self
            .txns
            .transition_to_settled(
                &mut tx,
                gateway_transaction_id,
                fee_ack.as_ref().map(|a| a.post_id),
                posting_state,
            )
            .await?;
        if rows == 0 {
            // Already settled (or terminal) — nothing to emit. Idempotent re-entry.
            tx.rollback().await?;
            return Ok(SettleOutcome { gateway_transaction_id, already_settled: true, fee_post: fee_ack });
        }
        let hdr = self.txns.fetch_settled_header_on(&mut tx, gateway_transaction_id).await?;
        tx.commit().await?;

        // Only the winner of the transition publishes — exactly-once across the seam.
        self.sink.publish(GatewayEvent::GatewayTransactionSettled(GatewayTransactionSettled {
            gateway_transaction_id,
            company_id: hdr.company_id,
            provider_code: hdr.provider_code,
            provider_transaction_id: hdr.provider_transaction_id,
            direction: hdr.direction,
            party_type: hdr.party_type,
            party_id: hdr.party_id,
            gross_amount: hdr.gross_amount,
            fee_amount: hdr.fee_amount,
            net_amount: hdr.net_amount,
            currency: hdr.currency,
            settled_at: hdr.settled_at.unwrap_or_else(chrono::Utc::now),
            reference_no: hdr.reference_no,
        }));

        Ok(SettleOutcome { gateway_transaction_id, already_settled: false, fee_post: fee_ack })
    }

    /// Convenience access to the pure composer for callers/tests that already hold
    /// a [`FeeSourceRow`].
    pub fn compose_fee(src: &FeeSourceRow, posting_date: chrono::NaiveDate) -> Option<AccountingPostEnvelope> {
        compose_fee_post(src, posting_date)
    }

    /// Resolve a gateway transaction by the provider dedup key
    /// `(provider_code, provider_transaction_id)` and settle it. This is the
    /// webhook entrypoint: a normalized provider notification carries the code +
    /// the provider's own transaction id; this finds the row and runs the
    /// transition-gated settle. Provider-specific payload parsing / signature
    /// verification stays at the composition layer.
    pub async fn settle_by_provider_tx(
        &self,
        provider_code: &str,
        provider_transaction_id: &str,
        fee_sink: &dyn GlPostSink,
    ) -> Result<SettleOutcome, GatewayError> {
        let id = self
            .txns
            .find_id_by_provider_tx(&self.db_pool, provider_code, provider_transaction_id)
            .await?
            .ok_or_else(|| GatewayError::NotFound(Uuid::nil()))?;
        self.settle_transaction(id, fee_sink).await
    }

    /// Borrow the event sink (for wiring / inspection).
    pub fn event_sink(&self) -> &Arc<dyn GatewayEventSink> {
        &self.sink
    }

    /// The gross/fee/net invariant check shared with validation: net = gross − fee.
    pub fn check_money(gross: Decimal, fee: Decimal, net: Decimal) -> Result<(), GatewayError> {
        if gross < Decimal::ZERO || fee < Decimal::ZERO || net < Decimal::ZERO {
            return Err(GatewayError::UnsupportedCurrency("negative amount".into()));
        }
        if net != gross - fee {
            return Err(GatewayError::UnsupportedCurrency(format!("net {net} != gross {gross} - fee {fee}")));
        }
        Ok(())
    }
}
