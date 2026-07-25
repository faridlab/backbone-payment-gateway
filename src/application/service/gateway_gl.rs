//! Outbound GL-posting port for the gateway FEE (hand-authored, user-owned) — re-export of the shared
//! contract.
//!
//! The GL-posting wire types (`AccountingPostEnvelope`, `GlPostLine`, `GlPostAck`, `GlPostRejected`)
//! and the `GlPostSink` port now live in the shared `backbone-gl-posting` crate (backbone-framework
//! v2.7.5) — the single source for all producers (phase 2). This file re-exports them; the gateway
//! posts its fee companion journal (`Dr Gateway Fee Expense · Cr Bank`, `source_type = "gateway_fee"`)
//! through the shared `GlPostSink`. Zero normal Cargo edge into backbone-accounting.

pub use backbone_gl_posting::{
    AccountingPostEnvelope, GlPostAck, GlPostLine, GlPostRejected, GlPostSink,
};
