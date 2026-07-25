//! Outbound GL-posting port for the gateway FEE (hand-authored, user-owned) — re-export of the shared
//! contract.
//!
//! The GL-posting wire types (`AccountingPostEnvelope`, `GlPostLine`, `GlPostAck`, `GlPostRejected`)
//! and the `GlPostSink` port now live in the shared `backbone-gl-posting` crate (backbone-framework
//! v2.7.5) — the single source for all producers (phase 2). This file re-exports them; the gateway
//! posts its fee companion journal (`Dr Gateway Fee Expense · Cr Bank`, `source_type = "gateway_fee"`)
//! through the shared `GlPostSink`. Zero normal Cargo edge into backbone-accounting.
//!
//! # Idempotency contract the gateway relies on
//!
//! `GatewayWriteService::settle_transaction` posts the fee companion journal OUTSIDE the
//! `pending → settled` transition tx — accounting is its own unit of work, and a rejection must abort
//! the settle without leaving a half-settled row. Exactly-once of the FEE therefore depends on the
//! composition-provided `GlPostSink` deduping on the envelope's `source_id` (= the gateway transaction
//! id; `idempotency_key` is the same value). That dedup guarantee is part of the shared contract:
//! `backbone-gl-posting` states accounting dedups on `source_id`, and `GlPostAck::idempotent_reuse`
//! reports when a post short-circuited as a duplicate. A composition `GlPostSink` that does NOT honor
//! `source_id` dedup would double-post the fee on a concurrent or redelivered settle. The seam EVENT
//! itself (`GatewayTransactionSettled`) is independently gated exactly-once by the transition
//! UPDATE's `rows_affected == 1`, so a misbehaving fee sink can never cause a double PaymentEntry —
//! only a duplicate fee line, which accounting's own dedup is expected to absorb.

pub use backbone_gl_posting::{
    AccountingPostEnvelope, GlPostAck, GlPostLine, GlPostRejected, GlPostSink,
};
