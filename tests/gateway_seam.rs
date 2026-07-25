//! The gateway ↔ payment settlement seam — end-to-end (ADR-001 §3, §4).
//!
//! Proves the fee gap closes compositionally: a gateway settle emits
//! `GatewayTransactionSettled`; the composition ACL (implemented inline here, as
//! payment does for its billing seam) creates a `PaymentEntry` at gross, which
//! posts payment's unchanged `Dr Bank · Cr A/R` and emits `PaymentSettled`; and
//! the gateway posts its `Dr Fee · Cr Bank` companion. All three movements land in
//! ONE shared fake ledger, and the assertions check the net balances reconcile:
//! `Bank = gross − fee`, `A/R = −gross`, `Fee Expense = +fee`, `Σ = 0`.
//!
//! Requires a live Postgres with BOTH the `payment` and `payment_gateway` schemas
//! migrated, at DATABASE_URL. Dev-only edge on backbone-payment (ADR-002).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use backbone_payment::application::service::payment_events::{
    PaymentEvent, PaymentEventSink,
};
use backbone_payment::application::service::payment_gl::{
    AccountingPostEnvelope, GlPostAck, GlPostLine, GlPostRejected, GlPostSink,
};
use backbone_payment::application::service::payment_write_service::{
    NewPayment, PaymentWriteService,
};
use backbone_payment_gateway::application::service::gateway_events::{
    GatewayEvent, GatewayEventSink, GatewayTransactionSettled,
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

/// A shared fake ledger: net balance per GL account (debit-positive). Both the
/// payment `GlPostSink` and the gateway `GlPostSink` record into it, so the
/// two independent journals compose into one reconcilable picture.
#[derive(Default, Clone)]
struct Ledger {
    balances: Arc<Mutex<HashMap<Uuid, Decimal>>>,
}
impl Ledger {
    fn post_lines(&self, lines: &[impl LedgerLine]) {
        let mut b = self.balances.lock().unwrap();
        for l in lines {
            let e = b.entry(l.account_id()).or_insert(Decimal::ZERO);
            *e += l.debit() - l.credit();
        }
    }
    fn net(&self, account: Uuid) -> Decimal {
        *self.balances.lock().unwrap().get(&account).unwrap_or(&Decimal::ZERO)
    }
    fn total(&self) -> Decimal {
        self.balances.lock().unwrap().values().copied().sum()
    }
}
trait LedgerLine {
    fn account_id(&self) -> Uuid;
    fn debit(&self) -> Decimal;
    fn credit(&self) -> Decimal;
}
impl LedgerLine for GlPostLine {
    fn account_id(&self) -> Uuid { self.account_id }
    fn debit(&self) -> Decimal { self.debit }
    fn credit(&self) -> Decimal { self.credit }
}

#[async_trait::async_trait]
impl GlPostSink for Ledger {
    async fn post(&self, env: &AccountingPostEnvelope) -> Result<GlPostAck, GlPostRejected> {
        self.post_lines(&env.lines);
        Ok(GlPostAck { post_id: Uuid::new_v4(), journal_id: Uuid::new_v4(), idempotent_reuse: false })
    }
}

#[derive(Default, Clone)]
struct NoopPaymentEvents;
impl PaymentEventSink for NoopPaymentEvents {
    fn publish(&self, _e: PaymentEvent) {}
}

/// Captures the seam event so the test can drive the ACL as a direct async call
/// (the shipped `GatewayEventSink::publish` is sync fire-and-forget; the real ACL
/// runs on a bus. Testing the composition as a direct call is equivalent and
/// avoids a sync→async bridge.)
#[derive(Default, Clone)]
struct Recorder {
    events: Arc<Mutex<Vec<GatewayTransactionSettled>>>,
}
impl GatewayEventSink for Recorder {
    fn publish(&self, event: GatewayEvent) {
        if let GatewayEvent::GatewayTransactionSettled(s) = event {
            self.events.lock().unwrap().push(s);
        }
    }
}

/// The composition ACL: turns a `GatewayTransactionSettled` into a PaymentEntry at
/// gross + posts it. Owns the bank (settlement) + A/R accounts it resolves the
/// event into — standing in for real party→account resolution.
async fn apply_settlement_acl(
    s: &GatewayTransactionSettled,
    payments: &PaymentWriteService,
    gl: &dyn GlPostSink,
    bank_account: Uuid,
    ar_account: Uuid,
) {
    let payment_id = payments
        .create_payment(NewPayment {
            payment_number: uq("GWP"),
            company_id: s.company_id,
            branch_id: None,
            payment_type: "receive".into(),
            party_type: s.party_type.clone(),
            party_id: s.party_id,
            posting_date: s.settled_at.date_naive(),
            currency: Some(s.currency.clone()),
            mode_of_payment_id: None,
            bank_account_id: bank_account,
            party_account_id: ar_account,
            paid_amount: s.gross_amount,
            reference_no: Some(s.provider_transaction_id.clone()),
            allocations: vec![],
        })
        .await
        .expect("create PaymentEntry from gateway settle");
    payments
        .post_payment(payment_id, gl)
        .await
        .expect("post the settlement journal");
}

async fn seed_gateway_tx(pool: &PgPool, company: Uuid, bank: Uuid, gross: Decimal, fee: Decimal) -> Uuid {
    let provider = Uuid::new_v4();
    let fee_acc = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO payment_gateway.payment_gateway_providers
             (id, code, company_id, display_name, fee_account_id, settlement_account_id, is_active, metadata)
           VALUES ($1, 'midtrans'::gateway_provider_code, $2, $3, $4, $5, TRUE, $6::jsonb)"#,
    )
    .bind(provider)
    .bind(company)
    .bind(uq("Midtrans"))
    .bind(fee_acc)
    .bind(bank)
    .bind(META)
    .execute(pool)
    .await
    .unwrap();

    let txn = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO payment_gateway.gateway_transactions
             (id, company_id, provider_id, provider_code, provider_transaction_id, direction,
              party_type, party_id, gross_amount, fee_amount, net_amount, currency, status, posting_state, metadata)
           VALUES ($1, $2, $3, 'midtrans'::gateway_provider_code, $4, 'receive'::gateway_direction,
                   'customer'::gateway_party_type, $2, $5, $6, $7, 'IDR',
                   'pending'::gateway_transaction_status, 'pending'::gateway_posting_state, $8::jsonb)"#,
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
    txn
}

#[tokio::test]
async fn seam_settle_balances_across_payment_and_gateway() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let bank = Uuid::new_v4();
    let ar = Uuid::new_v4();
    let gross = d("1000000");
    let fee = d("30000");
    let txn = seed_gateway_tx(&pool, company, bank, gross, fee).await;

    // The composition: one shared ledger, the payment write service, and a
    // recorder capturing the seam event the gateway emits on settle.
    let ledger = Arc::new(Ledger::default());
    let payments = PaymentWriteService::with_sink(
        pool.clone(),
        Arc::new(NoopPaymentEvents) as Arc<dyn PaymentEventSink>,
    );
    let recorder = Arc::new(Recorder::default());
    let gateway = GatewayWriteService::with_sink(pool.clone(), recorder.clone() as Arc<dyn GatewayEventSink>);

    // Settle the gateway transaction — posts the fee companion + emits the seam event.
    gateway.settle_transaction(txn, ledger.as_ref()).await.expect("gateway settle");

    // The composition ACL turns the emitted event into a PaymentEntry + settlement post.
    let events = recorder.events.lock().unwrap().clone();
    assert_eq!(events.len(), 1, "exactly one seam event emitted");
    apply_settlement_acl(&events[0], &payments, ledger.as_ref(), bank, ar).await;

    // The three movements reconcile: Bank net = gross − fee; A/R = −gross;
    // the whole ledger sums to zero (double-entry balances), which forces the
    // Fee Expense account to net +fee — i.e. the companion post landed correctly.
    assert_eq!(ledger.net(bank), gross - fee, "Bank nets to gross - fee");
    assert_eq!(ledger.net(ar), -gross, "A/R credited the full gross");
    assert_eq!(ledger.total(), Decimal::ZERO, "the combined ledger balances to zero");
}
