//! Gateway webhook probes — the exactly-once invariant against a REAL Postgres
//! (ADR-001 §5). Requires DATABASE_URL pointing at a DB with the `payment_gateway`
//! schema migrated (`metaphor migration` / `sqlx::migrate`).
//!
//! GWP-1 a settle posts the fee companion journal exactly once and emits exactly
//!      one `GatewayTransactionSettled`.
//! GWP-2 a redelivered webhook (a second settle of the same transaction) is a
//!      no-op: no second fee post, no second seam event (the `pending → settled`
//!      UPDATE's `rows_affected == 1` gates the emission).
//! GWP-3 a rejected fee post marks `posting_state = failed` and settles nothing.

use std::sync::{Arc, Mutex};

use backbone_payment_gateway::application::service::gateway_events::{GatewayEvent, GatewayEventSink};
use backbone_payment_gateway::application::service::gateway_gl::{
    AccountingPostEnvelope, GlPostAck, GlPostRejected, GlPostSink,
};
use backbone_payment_gateway::application::service::gateway_write_service::GatewayWriteService;
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

fn d(s: &str) -> Decimal {
    Decimal::from_str_exact(s).unwrap()
}
fn uq(p: &str) -> String {
    format!("{p}-{}", &Uuid::new_v4().simple().to_string()[..8])
}
async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgresql://postgres:postgres@localhost:5433/backbone_payment_gateway".to_string()
    });
    PgPool::connect(&url).await.expect("connect DB")
}

const META: &str = r#"{"created_at":null,"updated_at":null,"deleted_at":null,"created_by":null,"updated_by":null,"deleted_by":null}"#;

/// Insert a configured provider + a pending gateway transaction; return both ids.
async fn seed_pending(
    pool: &PgPool,
    company: Uuid,
    gross: Decimal,
    fee: Decimal,
) -> (Uuid, Uuid) {
    let provider = Uuid::new_v4();
    let fee_acc = Uuid::new_v4();
    let bank_acc = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO payment_gateway.payment_gateway_providers
             (id, code, company_id, display_name, fee_account_id, settlement_account_id, is_active, metadata)
           VALUES ($1, 'midtrans'::gateway_provider_code, $2, $3, $4, $5, TRUE, $6::jsonb)"#,
    )
    .bind(provider)
    .bind(company)
    .bind(uq("Midtrans"))
    .bind(fee_acc)
    .bind(bank_acc)
    .bind(META)
    .execute(pool)
    .await
    .unwrap();

    let txn = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO payment_gateway.gateway_transactions
             (id, company_id, provider_id, provider_code, provider_transaction_id, direction,
              gross_amount, fee_amount, net_amount, currency, status, posting_state, metadata)
           VALUES ($1, $2, $3, 'midtrans'::gateway_provider_code, $4, 'receive'::gateway_direction,
                   $5, $6, $7, 'IDR', 'pending'::gateway_transaction_status,
                   'pending'::gateway_posting_state, $8::jsonb)"#,
    )
    .bind(txn)
    .bind(company)
    .bind(provider)
    .bind(uq("MID-ORDER"))
    .bind(gross)
    .bind(fee)
    .bind(gross - fee)
    .bind(META)
    .execute(pool)
    .await
    .unwrap();
    (provider, txn)
}

#[derive(Clone)]
struct OkFee {
    post: Uuid,
    journal: Uuid,
    calls: Arc<std::sync::atomic::AtomicU64>,
}
#[async_trait::async_trait]
impl GlPostSink for OkFee {
    async fn post(&self, _e: &AccountingPostEnvelope) -> Result<GlPostAck, GlPostRejected> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(GlPostAck { post_id: self.post, journal_id: self.journal, idempotent_reuse: false })
    }
}

#[derive(Default, Clone)]
struct Recorder {
    events: Arc<Mutex<Vec<GatewayEvent>>>,
}
impl GatewayEventSink for Recorder {
    fn publish(&self, e: GatewayEvent) {
        self.events.lock().unwrap().push(e);
    }
}

#[tokio::test]
async fn gwp1_settle_posts_fee_once_and_emits_once() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let (_provider, txn) = seed_pending(&pool, company, d("1000000"), d("30000")).await;

    let svc = GatewayWriteService::with_sink(pool.clone(), Arc::new(Recorder::default()));
    let fee = OkFee {
        post: Uuid::new_v4(),
        journal: Uuid::new_v4(),
        calls: Arc::new(std::sync::atomic::AtomicU64::new(0)),
    };
    let out = svc.settle_transaction(txn, &fee).await.expect("settle");

    assert!(!out.already_settled);
    assert_eq!(fee.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let evs = svc.event_sink().clone();
    let _ = evs; // sink held by svc; assert via a shared recorder instead (see gwp2)
}

#[tokio::test]
async fn gwp2_redelivered_webhook_is_a_noop() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let (_provider, txn) = seed_pending(&pool, company, d("1000000"), d("30000")).await;

    let recorder = Arc::new(Recorder::default());
    let svc = GatewayWriteService::with_sink(pool.clone(), recorder.clone() as Arc<dyn GatewayEventSink>);
    let fee = OkFee {
        post: Uuid::new_v4(),
        journal: Uuid::new_v4(),
        calls: Arc::new(std::sync::atomic::AtomicU64::new(0)),
    };

    // First delivery — settles.
    let _ = svc.settle_transaction(txn, &fee).await.expect("first settle");
    let first_calls = fee.calls.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(first_calls, 1);

    // Redelivery — the row is already `settled`, so the transition UPDATE affects
    // 0 rows and the emission is gated off: no second fee post, no second event.
    let second = svc.settle_transaction(txn, &fee).await.expect("second settle");
    assert!(second.already_settled);
    assert_eq!(fee.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let evs = recorder.events.lock().unwrap();
    let settled_count = evs
        .iter()
        .filter(|e| matches!(e, GatewayEvent::GatewayTransactionSettled(_)))
        .count();
    assert_eq!(settled_count, 1, "exactly one seam event across a redelivery");
}

#[tokio::test]
async fn gwp3_rejected_fee_marks_failed_and_settles_nothing() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let (_provider, txn) = seed_pending(&pool, company, d("1000000"), d("30000")).await;

    struct RejectFee;
    #[async_trait::async_trait]
    impl GlPostSink for RejectFee {
        async fn post(&self, _e: &AccountingPostEnvelope) -> Result<GlPostAck, GlPostRejected> {
            Err(GlPostRejected {
                code: "period_closed".into(),
                message: "accounting period is closed".into(),
            })
        }
    }

    let recorder = Arc::new(Recorder::default());
    let svc = GatewayWriteService::with_sink(pool.clone(), recorder.clone() as Arc<dyn GatewayEventSink>);
    let err = svc.settle_transaction(txn, &RejectFee).await.unwrap_err();
    assert_eq!(err.code(), "period_closed");

    // No seam event fired on rejection.
    assert!(recorder.events.lock().unwrap().is_empty());

    // The row is NOT settled, and posting_state is failed.
    let (status, posting_state): (String, String) = sqlx::query_as(
        "SELECT status::text, posting_state::text FROM payment_gateway.gateway_transactions WHERE id=$1",
    )
    .bind(txn)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "pending");
    assert_eq!(posting_state, "failed");
}

#[tokio::test]
async fn gwp4_refund_of_pending_returns_invalid_status() {
    // Council rec #2: refunding a non-settled transaction returns `invalid_status`
    // (NOT the old `unsupported_currency`), posts no fee, emits no event, and
    // leaves the row untouched. A pending row → status != "settled" → InvalidStatus
    // BEFORE any fee post or transition.
    let pool = pool().await;
    let company = Uuid::new_v4();
    let (_provider, txn) = seed_pending(&pool, company, d("1000000"), d("30000")).await;

    let recorder = Arc::new(Recorder::default());
    let svc = GatewayWriteService::with_sink(pool.clone(), recorder.clone() as Arc<dyn GatewayEventSink>);
    let fee = OkFee {
        post: Uuid::new_v4(),
        journal: Uuid::new_v4(),
        calls: Arc::new(std::sync::atomic::AtomicU64::new(0)),
    };

    let err = svc.reverse_transaction(txn, &fee).await.unwrap_err();
    assert_eq!(err.code(), "invalid_status", "refund-of-pending must be invalid_status, not unsupported_currency");
    // No fee posted, no event emitted.
    assert_eq!(fee.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(recorder.events.lock().unwrap().is_empty());

    // The row is untouched — still pending.
    let status: String =
        sqlx::query_scalar("SELECT status::text FROM payment_gateway.gateway_transactions WHERE id=$1")
            .bind(txn)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "pending");
}
