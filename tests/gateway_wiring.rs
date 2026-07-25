//! Gateway module wiring — DB-free unit tests for the builder surface added in
//! council rec #1 (the settlement engine is reachable from `PaymentGatewayModule`)
//! and rec #2 (status/money errors map to their own wire codes).
//!
//! No DATABASE_URL required: the module is built with a lazy (non-connecting)
//! pool, and `gateway_webhook_router()` only clones `Arc`s — neither touches the
//! DB. The refund-of-pending → `invalid_status` *path* needs a live row, so it
//! lives with the other DB probes in `gateway_webhook_probes.rs` (gwp4).

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;

use backbone_payment_gateway::application::service::{
    AccountingPostEnvelope, GatewayError, GlPostAck, GlPostRejected, GlPostSink,
};
use backbone_payment_gateway::PaymentGatewayModule;

/// A fee sink that always succeeds — enough to satisfy the builder / router shape
/// without an accounting client.
struct OkFee;
#[async_trait]
impl GlPostSink for OkFee {
    async fn post(&self, _e: &AccountingPostEnvelope) -> Result<GlPostAck, GlPostRejected> {
        Ok(GlPostAck {
            post_id: uuid::Uuid::nil(),
            journal_id: uuid::Uuid::nil(),
            idempotent_reuse: false,
        })
    }
}

/// A lazy pool that never connects — enough to build the module off-line.
fn lazy_pool() -> PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgresql://nobody:nobody@localhost:5432/nodb")
        .expect("connect_lazy does not connect")
}

#[tokio::test]
async fn builder_constructs_the_engine() {
    // Council rec #1: the settlement engine is reachable from the module (it used
    // to be constructable only inside tests/). Held with a default logging sink.
    let module = PaymentGatewayModule::builder()
        .with_database(lazy_pool())
        .build()
        .expect("module builds without a live DB");
    // The engine is constructed and held on the module.
    let _engine: &Arc<_> = &module.write_service;
}

#[tokio::test]
async fn webhook_router_is_none_without_fee_sink() {
    // No with_fee_sink(..) ⇒ the webhook cannot post the fee ⇒ no router.
    let module = PaymentGatewayModule::builder()
        .with_database(lazy_pool())
        .build()
        .unwrap();
    assert!(module.gateway_webhook_router().is_none());
}

#[tokio::test]
async fn webhook_router_is_some_with_fee_sink() {
    // A composition-provided fee sink unlocks the webhook router.
    let module = PaymentGatewayModule::builder()
        .with_database(lazy_pool())
        .with_fee_sink(Arc::new(OkFee) as Arc<dyn GlPostSink>)
        .build()
        .unwrap();
    assert!(module.gateway_webhook_router().is_some());
}

#[test]
fn error_codes_distinguish_status_money_currency() {
    // Council rec #2: status/money errors are no longer mislabeled
    // `unsupported_currency` on the wire.
    assert_eq!(GatewayError::InvalidStatus("pending".into()).code(), "invalid_status");
    assert_eq!(GatewayError::InvalidMoney("net != gross - fee".into()).code(), "invalid_money");
    assert_eq!(GatewayError::UnsupportedCurrency("USD".into()).code(), "unsupported_currency");

    // Status + money + currency are all 422 (client error); NotFound is 404.
    assert_eq!(GatewayError::InvalidStatus("x".into()).http_status(), 422);
    assert_eq!(GatewayError::InvalidMoney("x".into()).http_status(), 422);
    assert_eq!(GatewayError::UnsupportedCurrency("x".into()).http_status(), 422);
    assert_eq!(GatewayError::NotFound(uuid::Uuid::nil()).http_status(), 404);
}
