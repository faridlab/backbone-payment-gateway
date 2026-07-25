//! Gateway domain events (hand-authored, user-owned) — the public extension
//! surface, i.e. the settlement seam into backbone-payment (ADR-003 §4).
//!
//! On a GatewayTransaction's `pending → settled` transition the gateway emits
//! `GatewayTransactionSettled { gross, fee, net, party, … }` through a
//! `GatewayEventSink`. A composition ACL consumes it and (a) creates a
//! `PaymentEntry` (paid_amount = gross) — which runs payment's existing
//! settlement post + `PaymentSettled` → billing drawdown — and (b) posts the
//! gateway-fee companion journal. The gateway carries NO invoice allocations:
//! reconciliation to invoices stays payment's job, so an unmapped settlement
//! lands as on-account (payment's existing path). Zero normal Cargo edges — the
//! envelope is the wire contract, the same shape as payment's seam.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A gateway transaction settled at the provider — money arrived, the fee is
/// known, and the composition layer should now create the matching PaymentEntry
/// and post the fee. Carries gross/fee/net so the ACL never re-derives them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatewayTransactionSettled {
    pub gateway_transaction_id: Uuid,
    pub company_id: Uuid,
    /// "manual" | "midtrans" | "xendit" | "doku" | "stripe" — the provider that
    /// observed the transaction (string so the seam carries no Rust enum edge).
    pub provider_code: String,
    /// The gateway's own transaction id — the dedup key the ACL must persist on
    /// the PaymentEntry (e.g. into `reference_no`) so a redelivery is recognisable.
    pub provider_transaction_id: String,
    /// "receive" | "pay".
    pub direction: String,
    pub party_type: Option<String>,
    pub party_id: Option<Uuid>,
    /// Total charged — becomes the PaymentEntry `paid_amount` (the A/R credit).
    pub gross_amount: Decimal,
    /// The gateway fee — posted by the companion `Dr Fee · Cr Bank` journal.
    pub fee_amount: Decimal,
    /// `gross_amount − fee_amount` — what actually lands in the bank.
    pub net_amount: Decimal,
    pub currency: String,
    pub settled_at: DateTime<Utc>,
    pub reference_no: Option<String>,
    // NOTE: no `allocations[]` — reconciliation to invoices stays payment's job.
}

/// The gateway domain-event union. Reserved for `GatewayTransactionRefunded`
/// (reversal wiring — deferred per ADR-003 Consequences).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum GatewayEvent {
    GatewayTransactionSettled(GatewayTransactionSettled),
}

/// Sink for gateway domain events. Fire-and-forget; a real adapter wires a bus
/// (and stages into an outbox for crash-survival), tests record.
pub trait GatewayEventSink: Send + Sync {
    fn publish(&self, event: GatewayEvent);
}

/// Default sink — emits structured tracing events.
pub struct LoggingGatewaySink;

impl GatewayEventSink for LoggingGatewaySink {
    fn publish(&self, event: GatewayEvent) {
        tracing::info!(target: "payment_gateway.events", ?event, "gateway domain event");
    }
}
