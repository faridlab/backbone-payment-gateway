//! Provider codec + verified-ingest probes (feature `codecs`) — ADR-0021's
//! verification world and ADR-0022's money gate against a REAL Postgres
//! (ADR-001 §5). Requires DATABASE_URL pointing at a DB with the
//! `payment_gateway` schema migrated, including `resolve_webhook_target`.
//!
//! PGC-0 DOKU's digest construction matches the pinned doc vector (computed
//!      out-of-band in python, never by this crate's own code).
//! PGC-1 a validly signed DOKU settlement settles exactly once: one fee post,
//!      one seam event, raw_payload stamped.
//! PGC-2 a bad signature ⇒ 401 and ZERO writes (full-table snapshot).
//! PGC-3 a missing credential ⇒ 503 fail-closed, zero writes.
//! PGC-4 Midtrans: the payload is untrusted — a garbage signature and a lying
//!      amount still settle correctly from the re-fetched truth.
//! PGC-5 authority/row gross mismatch ⇒ 422, no write (not even the stamp).
//! PGC-6 pending notifications are acknowledged-and-ignored (both schemes).
//! PGC-7 redelivery is idempotent (no second fee post, no second event).
//! PGC-8 re-fetch transport error ⇒ 503, zero writes (provider retries).
//! PGC-9 every builtin codec's scheme matches the route declaration table, and
//!      a mismatched declaration is refused in-pipeline.
//! PGW-1 `settle_by_provider_tx_verified` rejects bad money before any write.

#![cfg(feature = "codecs")]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::http::{HeaderMap, HeaderValue};
use backbone_payment_gateway::application::service::gateway_codecs::{
    CredentialFetch, CredentialReader, DokuCodec, GatewayCodecRegistry, GatewaySecret,
    NotificationCodec, NotificationEvent, ProviderTruth, RefetchError, StatusRefetch,
    VerificationScheme, VerifyError, VerifyRequest,
};
use backbone_payment_gateway::application::service::gateway_events::{
    GatewayEvent, GatewayEventSink,
};
use backbone_payment_gateway::application::service::gateway_gl::{
    AccountingPostEnvelope, GlPostAck, GlPostRejected, GlPostSink,
};
use backbone_payment_gateway::application::service::gateway_ingest_service::WebhookIngestService;
use backbone_payment_gateway::application::service::gateway_write_service::GatewayWriteService;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use sha2::Sha256;
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

// ─────────────────────────────────────────────────────────────────────────────
// Fakes for the two composition ports
// ─────────────────────────────────────────────────────────────────────────────

/// Credential-store fake: one secret per account_ref, or an always-fail mode.
struct FakeCreds {
    secrets: HashMap<String, String>,
    fail: bool,
}
impl FakeCreds {
    fn with(account_ref: &str, secret: &str) -> Self {
        Self {
            secrets: [(account_ref.to_string(), secret.to_string())].into(),
            fail: false,
        }
    }
    fn failing() -> Self {
        Self {
            secrets: HashMap::new(),
            fail: true,
        }
    }
}
#[async_trait::async_trait]
impl CredentialReader for FakeCreds {
    async fn read_secret(
        &self,
        _company_id: Uuid,
        account_ref: &str,
        _purpose: &str,
    ) -> Result<GatewaySecret, CredentialFetch> {
        if self.fail {
            return Err(CredentialFetch {
                code: "credential_not_found".into(),
                message: "no credential issued for this provider".into(),
            });
        }
        self.secrets
            .get(account_ref)
            .cloned()
            .map(GatewaySecret::new)
            .ok_or_else(|| CredentialFetch {
                code: "credential_not_found".into(),
                message: format!("no credential for account_ref {account_ref}"),
            })
    }
}

/// Re-fetch fake: one fixed answer for any transaction.
struct StubRefetch {
    answer: Result<ProviderTruth, RefetchError>,
}
impl StubRefetch {
    fn settled(gross: Decimal, fee: Option<Decimal>) -> Self {
        Self {
            answer: Ok(ProviderTruth {
                settled: true,
                gross,
                fee,
            }),
        }
    }
    fn not_settled() -> Self {
        Self {
            answer: Ok(ProviderTruth {
                settled: false,
                gross: Decimal::ZERO,
                fee: None,
            }),
        }
    }
    fn failing(e: RefetchError) -> Self {
        Self { answer: Err(e) }
    }
}
#[async_trait::async_trait]
impl StatusRefetch for StubRefetch {
    async fn fetch(
        &self,
        _company_id: Uuid,
        _provider_code: &str,
        _provider_transaction_id: &str,
    ) -> Result<ProviderTruth, RefetchError> {
        self.answer.clone()
    }
}

#[derive(Clone)]
struct OkFee {
    post: Uuid,
    journal: Uuid,
    calls: Arc<AtomicU64>,
}
#[async_trait::async_trait]
impl GlPostSink for OkFee {
    async fn post(&self, _e: &AccountingPostEnvelope) -> Result<GlPostAck, GlPostRejected> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(GlPostAck {
            post_id: self.post,
            journal_id: self.journal,
            idempotent_reuse: false,
        })
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
fn settled_events(rec: &Recorder) -> usize {
    rec.events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, GatewayEvent::GatewayTransactionSettled(_)))
        .count()
}

// ─────────────────────────────────────────────────────────────────────────────
// Seeding + pipeline wiring
// ─────────────────────────────────────────────────────────────────────────────

/// Insert a provider config; return its id. One per (company, code) — the table
/// enforces that unique.
async fn seed_provider(
    pool: &PgPool,
    company: Uuid,
    code: &str,
    credentials_ref: Option<&str>,
) -> Uuid {
    let provider = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO payment_gateway.payment_gateway_providers
             (id, code, company_id, display_name, fee_account_id, settlement_account_id,
              credentials_ref, status, metadata)
           VALUES ($1, $3::gateway_provider_code, $2, $4, $5, $6, $7, 'active'::provider_status, $8::jsonb)"#,
    )
    .bind(provider)
    .bind(company)
    .bind(code)
    .bind(uq("Provider"))
    .bind(Uuid::new_v4()) // fee_account_id
    .bind(Uuid::new_v4()) // settlement_account_id
    .bind(credentials_ref)
    .bind(META)
    .execute(pool)
    .await
    .unwrap();
    provider
}

/// Insert a pending gateway transaction under an existing provider.
async fn seed_txn(
    pool: &PgPool,
    company: Uuid,
    provider: Uuid,
    code: &str,
    provider_txn_id: &str,
    gross: Decimal,
    fee: Decimal,
) -> Uuid {
    let txn = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO payment_gateway.gateway_transactions
             (id, company_id, provider_id, provider_code, provider_transaction_id, direction,
              gross_amount, fee_amount, net_amount, currency, status, posting_state, metadata)
           VALUES ($1, $2, $3, $5::gateway_provider_code, $4, 'receive'::gateway_direction,
                   $6, $7, $8, 'IDR', 'pending'::gateway_transaction_status,
                   'pending'::gateway_posting_state, $9::jsonb)"#,
    )
    .bind(txn)
    .bind(company)
    .bind(provider)
    .bind(provider_txn_id)
    .bind(code)
    .bind(gross)
    .bind(fee)
    .bind(gross - fee)
    .bind(META)
    .execute(pool)
    .await
    .unwrap();
    txn
}

/// Insert a provider config + a pending gateway transaction; return both ids.
async fn seed_pending(
    pool: &PgPool,
    company: Uuid,
    code: &str,
    credentials_ref: Option<&str>,
    provider_txn_id: &str,
    gross: Decimal,
    fee: Decimal,
) -> (Uuid, Uuid) {
    let provider = seed_provider(pool, company, code, credentials_ref).await;
    let txn = seed_txn(pool, company, provider, code, provider_txn_id, gross, fee).await;
    (provider, txn)
}

/// The full ingest stack with fakes wired; returns the fee-call counter and the
/// event recorder alongside the service.
fn make_ingest(
    pool: &PgPool,
    creds: FakeCreds,
    refetch: StubRefetch,
) -> (WebhookIngestService, Arc<AtomicU64>, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    let write = Arc::new(GatewayWriteService::with_sink(
        pool.clone(),
        recorder.clone() as Arc<dyn GatewayEventSink>,
    ));
    let calls = Arc::new(AtomicU64::new(0));
    let fee = Arc::new(OkFee {
        post: Uuid::new_v4(),
        journal: Uuid::new_v4(),
        calls: calls.clone(),
    });
    let svc = WebhookIngestService::new(
        pool.clone(),
        write,
        GatewayCodecRegistry::with_builtin(),
        Arc::new(creds),
        Arc::new(refetch),
        fee,
    );
    (svc, calls, recorder)
}

/// Company-scoped snapshot — the zero-writes proof. Counts this tenant's rows,
/// settled rows, and stamped rows: any write the pipeline could have made on
/// this test's behalf lands inside its company. (Scoping is required because
/// the probe tests run concurrently against one shared DB.)
#[derive(Debug, PartialEq, Clone, Copy)]
struct Snapshot {
    txns: i64,
    settled: i64,
    stamped: i64,
}
async fn snap(pool: &PgPool, company: Uuid) -> Snapshot {
    let (txns, settled, stamped): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*), \
                count(*) FILTER (WHERE status = 'settled'), \
                count(*) FILTER (WHERE raw_payload IS NOT NULL) \
         FROM payment_gateway.gateway_transactions WHERE company_id = $1",
    )
    .bind(company)
    .fetch_one(pool)
    .await
    .unwrap();
    Snapshot {
        txns,
        settled,
        stamped,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DOKU payload helpers
// ─────────────────────────────────────────────────────────────────────────────

fn target_for(slug: &str, provider_id: Uuid) -> String {
    format!("/webhooks/payment-gateway/{slug}/{provider_id}")
}

/// Sign exactly as DOKU documents: HMAC-SHA256 over
/// `client_id ":" request_id ":" lowercase(timestamp) ":" lowercase(target) ":" raw_body`.
fn doku_sign(
    secret: &str,
    client_id: &str,
    request_id: &str,
    timestamp: &str,
    target: &str,
    body: &[u8],
) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(
        format!(
            "{}:{}:{}:{}:",
            client_id,
            request_id,
            timestamp.to_lowercase(),
            target.to_lowercase()
        )
        .as_bytes(),
    );
    mac.update(body);
    let out = mac.finalize().into_bytes();
    out.iter().map(|b| format!("{b:02x}")).collect()
}

fn doku_headers(client_id: &str, request_id: &str, timestamp: &str, signature: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("Client-Id", HeaderValue::from_str(client_id).unwrap());
    h.insert("Request-Id", HeaderValue::from_str(request_id).unwrap());
    h.insert(
        "Request-Timestamp",
        HeaderValue::from_str(timestamp).unwrap(),
    );
    h.insert("Signature", HeaderValue::from_str(signature).unwrap());
    h
}

fn doku_body(txn_id: &str, amount: i64, status: &str) -> String {
    format!(
        r#"{{"order":{{"amount":{amount},"currency":"IDR"}},"transaction":{{"id":"{txn_id}","status":"{status}"}}}}"#
    )
}

fn midtrans_body(txn_id: &str, order_id: &str, gross_amount: i64, status: &str) -> String {
    format!(
        r#"{{"transaction_id":"{txn_id}","order_id":"{order_id}","gross_amount":{gross_amount},"transaction_status":"{status}","signature_key":"garbage-not-verified-by-design"}}"#
    )
}

async fn row_status(pool: &PgPool, txn: Uuid) -> String {
    sqlx::query_scalar("SELECT status::text FROM payment_gateway.gateway_transactions WHERE id=$1")
        .bind(txn)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ─────────────────────────────────────────────────────────────────────────────
// PGC-0 — the pinned construction
// ─────────────────────────────────────────────────────────────────────────────

/// The signature vector is PINNED: inputs and expected digest were computed
/// out-of-band (python, hashlib) from DOKU's documented component string. If
/// the codec's construction ever drifts from the docs, this is the probe that
/// fires — every other DOKU probe derives its signatures the same way the
/// codec verifies them, so only this one pins the construction itself.
#[test]
fn pgc0_doku_signature_construction_matches_docs() {
    const SECRET: &str = "SK-DOKU-TEST";
    const CLIENT_ID: &str = "MCH-0123-4567-89";
    const REQUEST_ID: &str = "8f14e45f-ceea-467f-abc7-4a1b9d3f2e10";
    const TIMESTAMP: &str = "2026-08-22T04:05:06Z";
    const TARGET: &str = "/webhooks/payment-gateway/doku/1b0e0f2a-9c3d-4e5f-8a7b-6c5d4e3f2a1b";
    const BODY: &str = r#"{"order":{"amount":1000000,"currency":"IDR"},"transaction":{"id":"DOKU-TXN-778812","status":"SETTLE"}}"#;
    const EXPECTED: &str = "6f2868e45fde3ef97fa8195f4ca324d0be593e803ca0bbed8b7a0148b0085941";

    // Cross-check the pinned vector with an independent in-test HMAC so a
    // transcription typo in EXPECTED can't pass silently either.
    assert_eq!(
        doku_sign(
            SECRET,
            CLIENT_ID,
            REQUEST_ID,
            TIMESTAMP,
            TARGET,
            BODY.as_bytes()
        ),
        EXPECTED
    );

    let codec = DokuCodec;
    let headers = doku_headers(CLIENT_ID, REQUEST_ID, TIMESTAMP, EXPECTED);
    let req = VerifyRequest {
        headers: &headers,
        request_target: TARGET,
    };
    codec
        .verify(BODY.as_bytes(), &req, &GatewaySecret::new(SECRET.into()))
        .expect("pinned vector must verify");

    // Tampered body byte ⇒ mismatch.
    let tampered = doku_body("DOKU-TXN-778812", 999999, "SETTLE");
    let err = codec
        .verify(
            tampered.as_bytes(),
            &req,
            &GatewaySecret::new(SECRET.into()),
        )
        .unwrap_err();
    assert!(matches!(err, VerifyError::Mismatch), "got {err:?}");

    // Wrong secret ⇒ mismatch.
    let err = codec
        .verify(
            BODY.as_bytes(),
            &req,
            &GatewaySecret::new("SK-DOKU-OTHER".into()),
        )
        .unwrap_err();
    assert!(matches!(err, VerifyError::Mismatch), "got {err:?}");

    // Non-hex signature ⇒ malformed (refused, never loosely compared).
    let bad_hex = doku_headers(CLIENT_ID, REQUEST_ID, TIMESTAMP, &"z".repeat(64));
    let err = codec
        .verify(
            BODY.as_bytes(),
            &VerifyRequest {
                headers: &bad_hex,
                request_target: TARGET,
            },
            &GatewaySecret::new(SECRET.into()),
        )
        .unwrap_err();
    assert!(
        matches!(err, VerifyError::MalformedSignature),
        "got {err:?}"
    );

    // Missing digest header (signature itself well-formed) ⇒ MissingHeaders,
    // not a weaker digest over what remains.
    let mut missing = doku_headers(CLIENT_ID, REQUEST_ID, TIMESTAMP, EXPECTED);
    missing.remove("client-id");
    let err = codec
        .verify(
            BODY.as_bytes(),
            &VerifyRequest {
                headers: &missing,
                request_target: TARGET,
            },
            &GatewaySecret::new(SECRET.into()),
        )
        .unwrap_err();
    assert!(
        matches!(err, VerifyError::MissingHeaders(ref h) if h.as_slice() == ["client-id"]),
        "got {err:?}"
    );

    // And the parse side of the pinned body.
    let n = codec.parse(BODY.as_bytes()).expect("parse pinned body");
    assert_eq!(n.provider_code, "doku");
    assert_eq!(n.provider_transaction_id, "DOKU-TXN-778812");
    assert_eq!(
        n.event,
        NotificationEvent::SettleHint {
            gross_hint: d("1000000")
        }
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// PGC-1..3 — the DOKU fail-closed ladder
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn pgc1_doku_valid_signature_settles_exactly_once() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let txn_id = uq("DOKU-TXN");
    let (provider, txn) = seed_pending(
        &pool,
        company,
        "doku",
        Some("doku-cred-1"),
        &txn_id,
        d("1000000"),
        d("30000"),
    )
    .await;

    let (svc, calls, recorder) = make_ingest(
        &pool,
        FakeCreds::with("doku-cred-1", "SK-DOKU-TEST"),
        StubRefetch::not_settled(), // HmacRawBody never consults the refetcher
    );

    let body = doku_body(&txn_id, 1000000, "SETTLE");
    let target = target_for("doku", provider);
    let sig = doku_sign(
        "SK-DOKU-TEST",
        "pgc-client",
        "pgc-request",
        "2026-08-22T04:05:06Z",
        &target,
        body.as_bytes(),
    );
    let headers = doku_headers("pgc-client", "pgc-request", "2026-08-22T04:05:06Z", &sig);
    let req = VerifyRequest {
        headers: &headers,
        request_target: &target,
    };

    let out = svc
        .ingest(
            "doku",
            provider,
            VerificationScheme::HmacRawBody,
            body.as_bytes(),
            &req,
        )
        .await
        .expect("valid signed settle must settle");
    assert!(
        out.settled && !out.already_settled && !out.ignored,
        "got {out:?}"
    );

    assert_eq!(row_status(&pool, txn).await, "settled");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "fee companion posted once");
    assert_eq!(settled_events(&recorder), 1, "exactly one seam event");

    // The verified bytes are stamped for audit. jsonb::text may reorder keys,
    // so assert on the transaction id VALUE, not the payload layout.
    let stamped: Option<String> = sqlx::query_scalar(
        "SELECT raw_payload->'transaction'->>'id' FROM payment_gateway.gateway_transactions WHERE id=$1",
    )
    .bind(txn)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        stamped.as_deref(),
        Some(txn_id.as_str()),
        "raw_payload must carry the original body"
    );
}

#[tokio::test]
async fn pgc2_doku_bad_signature_401_zero_writes() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let txn_id = uq("DOKU-TXN");
    let (provider, _txn) = seed_pending(
        &pool,
        company,
        "doku",
        Some("doku-cred-1"),
        &txn_id,
        d("1000000"),
        d("30000"),
    )
    .await;

    let (svc, calls, recorder) = make_ingest(
        &pool,
        FakeCreds::with("doku-cred-1", "SK-DOKU-TEST"),
        StubRefetch::not_settled(),
    );

    let before = snap(&pool, company).await;
    let body = doku_body(&txn_id, 1000000, "SETTLE");
    let target = target_for("doku", provider);
    // Well-formed 64-hex signature with the wrong bytes.
    let headers = doku_headers(
        "pgc-client",
        "pgc-request",
        "2026-08-22T04:05:06Z",
        &"0".repeat(64),
    );
    let req = VerifyRequest {
        headers: &headers,
        request_target: &target,
    };

    let err = svc
        .ingest(
            "doku",
            provider,
            VerificationScheme::HmacRawBody,
            body.as_bytes(),
            &req,
        )
        .await
        .unwrap_err();
    assert_eq!(err.http_status(), 401, "got {err:?}");
    assert_eq!(err.code(), "signature_verification_failed");

    // ZERO writes: no state flip, no stamp, no fee post, no event.
    assert_eq!(
        snap(&pool, company).await,
        before,
        "full-table snapshot must be unchanged"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(settled_events(&recorder), 0);
}

#[tokio::test]
async fn pgc3_doku_missing_credential_503_fail_closed() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let txn_id = uq("DOKU-TXN");
    // Provider config carries NO credentials_ref — nothing to verify with.
    let (provider, _txn) = seed_pending(
        &pool,
        company,
        "doku",
        None,
        &txn_id,
        d("1000000"),
        d("30000"),
    )
    .await;

    let (svc, calls, recorder) =
        make_ingest(&pool, FakeCreds::failing(), StubRefetch::not_settled());

    let before = snap(&pool, company).await;
    let body = doku_body(&txn_id, 1000000, "SETTLE");
    let target = target_for("doku", provider);
    let headers = doku_headers(
        "pgc-client",
        "pgc-request",
        "2026-08-22T04:05:06Z",
        &"0".repeat(64),
    );
    let req = VerifyRequest {
        headers: &headers,
        request_target: &target,
    };

    let err = svc
        .ingest(
            "doku",
            provider,
            VerificationScheme::HmacRawBody,
            body.as_bytes(),
            &req,
        )
        .await
        .unwrap_err();
    assert_eq!(err.http_status(), 503, "got {err:?}");
    assert_eq!(err.code(), "credential_unavailable");

    assert_eq!(
        snap(&pool, company).await,
        before,
        "fail-closed: zero writes without a credential"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(settled_events(&recorder), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// PGC-4..6 — re-fetch authority, money gate, pending
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn pgc4_midtrans_refetch_authoritative_settle() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    // Rows are keyed by the ORDER id — the id the status API accepts.
    let txn_id = uq("MID-TXN");
    let order_id = uq("MID-ORDER");
    let (provider, txn) = seed_pending(
        &pool,
        company,
        "midtrans",
        None,
        &order_id,
        d("1000000"),
        d("30000"),
    )
    .await;

    let (svc, calls, recorder) = make_ingest(
        &pool,
        FakeCreds::failing(), // ApiRefetch never reads the webhook credential
        StubRefetch::settled(d("1000000"), None), // truth: settled, no fee reported
    );

    // The payload LIES about the amount and carries a garbage signature — both
    // are irrelevant by design: the re-fetch is the only authority. The
    // per-attempt transaction_id differs from the keyed order id on purpose.
    let body = midtrans_body(&txn_id, &order_id, 999999, "settlement");
    let target = target_for("midtrans", provider);
    let headers = HeaderMap::new(); // no headers needed for ApiRefetch
    let req = VerifyRequest {
        headers: &headers,
        request_target: &target,
    };

    let out = svc
        .ingest(
            "midtrans",
            provider,
            VerificationScheme::ApiRefetch,
            body.as_bytes(),
            &req,
        )
        .await
        .expect("truth from the refetch must settle");
    assert!(out.settled && !out.ignored, "got {out:?}");

    assert_eq!(row_status(&pool, txn).await, "settled");
    // Authority reported no fee ⇒ the row's recorded fee (30000) stands.
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "fee companion posted from the row fee"
    );
    assert_eq!(settled_events(&recorder), 1);
    // The gross booked is the authority's 1000000 — the payload's 999999 never landed.
    let gross: Decimal = sqlx::query_scalar(
        "SELECT gross_amount FROM payment_gateway.gateway_transactions WHERE id=$1",
    )
    .bind(txn)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(gross, d("1000000"));
}

#[tokio::test]
async fn pgc5_amount_mismatch_422_no_write() {
    let pool = pool().await;
    let company = Uuid::new_v4();

    // (a) HmacRawBody: the VERIFIED body disagrees with the recorded row.
    let doku_txn = uq("DOKU-TXN");
    let (doku_provider, doku_row) = seed_pending(
        &pool,
        company,
        "doku",
        Some("doku-cred-1"),
        &doku_txn,
        d("1000000"),
        d("30000"),
    )
    .await;
    let (svc, calls, recorder) = make_ingest(
        &pool,
        FakeCreds::with("doku-cred-1", "SK-DOKU-TEST"),
        StubRefetch::not_settled(),
    );
    let before = snap(&pool, company).await;

    let body = doku_body(&doku_txn, 999999, "SETTLE"); // 1 rupiah short of the row
    let target = target_for("doku", doku_provider);
    let sig = doku_sign(
        "SK-DOKU-TEST",
        "pgc-client",
        "pgc-request",
        "2026-08-22T04:05:06Z",
        &target,
        body.as_bytes(),
    );
    let headers = doku_headers("pgc-client", "pgc-request", "2026-08-22T04:05:06Z", &sig);
    let req = VerifyRequest {
        headers: &headers,
        request_target: &target,
    };
    let err = svc
        .ingest(
            "doku",
            doku_provider,
            VerificationScheme::HmacRawBody,
            body.as_bytes(),
            &req,
        )
        .await
        .unwrap_err();
    assert_eq!(err.http_status(), 422, "got {err:?}");
    assert_eq!(err.code(), "amount_mismatch");
    assert_eq!(
        row_status(&pool, doku_row).await,
        "pending",
        "mismatch leaves the row untouched"
    );
    assert_eq!(
        snap(&pool, company).await,
        before,
        "not even the raw_payload stamp may land on mismatch"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(settled_events(&recorder), 0);

    // (b) ApiRefetch: the re-fetched truth disagrees with the recorded row.
    let mid_txn = uq("MID-TXN");
    let mid_order = uq("MID-ORDER");
    let (mid_provider, mid_row) = seed_pending(
        &pool,
        company,
        "midtrans",
        None,
        &mid_order,
        d("1000000"),
        d("30000"),
    )
    .await;
    let (svc, calls, recorder) = make_ingest(
        &pool,
        FakeCreds::failing(),
        StubRefetch::settled(d("999999"), None),
    );
    let before = snap(&pool, company).await;
    let body = midtrans_body(&mid_txn, &mid_order, 1000000, "settlement");
    let target = target_for("midtrans", mid_provider);
    let headers = HeaderMap::new();
    let req = VerifyRequest {
        headers: &headers,
        request_target: &target,
    };
    let err = svc
        .ingest(
            "midtrans",
            mid_provider,
            VerificationScheme::ApiRefetch,
            body.as_bytes(),
            &req,
        )
        .await
        .unwrap_err();
    assert_eq!(err.http_status(), 422, "got {err:?}");
    assert_eq!(err.code(), "amount_mismatch");
    assert_eq!(row_status(&pool, mid_row).await, "pending");
    assert_eq!(snap(&pool, company).await, before);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(settled_events(&recorder), 0);
}

#[tokio::test]
async fn pgc6_pending_notification_acknowledged_ignored() {
    let pool = pool().await;
    let company = Uuid::new_v4();

    // (a) DOKU: validly signed PENDING — acknowledged, ignored, no write.
    let doku_txn = uq("DOKU-TXN");
    let (doku_provider, doku_row) = seed_pending(
        &pool,
        company,
        "doku",
        Some("doku-cred-1"),
        &doku_txn,
        d("1000000"),
        d("30000"),
    )
    .await;
    let (svc, calls, recorder) = make_ingest(
        &pool,
        FakeCreds::with("doku-cred-1", "SK-DOKU-TEST"),
        StubRefetch::not_settled(),
    );
    let body = doku_body(&doku_txn, 1000000, "PENDING");
    let target = target_for("doku", doku_provider);
    let sig = doku_sign(
        "SK-DOKU-TEST",
        "pgc-client",
        "pgc-request",
        "2026-08-22T04:05:06Z",
        &target,
        body.as_bytes(),
    );
    let headers = doku_headers("pgc-client", "pgc-request", "2026-08-22T04:05:06Z", &sig);
    let req = VerifyRequest {
        headers: &headers,
        request_target: &target,
    };
    let out = svc
        .ingest(
            "doku",
            doku_provider,
            VerificationScheme::HmacRawBody,
            body.as_bytes(),
            &req,
        )
        .await
        .expect("pending is a valid notification");
    assert!(out.ignored && !out.settled, "got {out:?}");
    assert_eq!(out.reason.as_deref(), Some("payment not yet settled"));
    assert_eq!(row_status(&pool, doku_row).await, "pending");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(settled_events(&recorder), 0);

    // (b) Midtrans: authority reports the transaction not settled.
    let mid_txn = uq("MID-TXN");
    let mid_order = uq("MID-ORDER");
    let (mid_provider, mid_row) = seed_pending(
        &pool,
        company,
        "midtrans",
        None,
        &mid_order,
        d("1000000"),
        d("30000"),
    )
    .await;
    let (svc, calls, recorder) =
        make_ingest(&pool, FakeCreds::failing(), StubRefetch::not_settled());
    let body = midtrans_body(&mid_txn, &mid_order, 1000000, "settlement");
    let target = target_for("midtrans", mid_provider);
    let headers = HeaderMap::new();
    let req = VerifyRequest {
        headers: &headers,
        request_target: &target,
    };
    let out = svc
        .ingest(
            "midtrans",
            mid_provider,
            VerificationScheme::ApiRefetch,
            body.as_bytes(),
            &req,
        )
        .await
        .expect("not-settled is a valid answer");
    assert!(out.ignored && !out.settled, "got {out:?}");
    assert_eq!(row_status(&pool, mid_row).await, "pending");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(settled_events(&recorder), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// PGC-7..9 — idempotence, transport, scheme table
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn pgc7_redelivery_is_idempotent() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let txn_id = uq("DOKU-TXN");
    let (provider, txn) = seed_pending(
        &pool,
        company,
        "doku",
        Some("doku-cred-1"),
        &txn_id,
        d("1000000"),
        d("30000"),
    )
    .await;

    let (svc, calls, recorder) = make_ingest(
        &pool,
        FakeCreds::with("doku-cred-1", "SK-DOKU-TEST"),
        StubRefetch::not_settled(),
    );

    let body = doku_body(&txn_id, 1000000, "SETTLE");
    let target = target_for("doku", provider);
    let sig = doku_sign(
        "SK-DOKU-TEST",
        "pgc-client",
        "pgc-request",
        "2026-08-22T04:05:06Z",
        &target,
        body.as_bytes(),
    );
    let headers = doku_headers("pgc-client", "pgc-request", "2026-08-22T04:05:06Z", &sig);
    let req = VerifyRequest {
        headers: &headers,
        request_target: &target,
    };

    let first = svc
        .ingest(
            "doku",
            provider,
            VerificationScheme::HmacRawBody,
            body.as_bytes(),
            &req,
        )
        .await
        .unwrap();
    assert!(first.settled && !first.already_settled);

    // The provider redelivers the same notification verbatim.
    let second = svc
        .ingest(
            "doku",
            provider,
            VerificationScheme::HmacRawBody,
            body.as_bytes(),
            &req,
        )
        .await
        .unwrap();
    assert!(
        !second.settled && second.already_settled && !second.ignored,
        "got {second:?}"
    );

    assert_eq!(row_status(&pool, txn).await, "settled");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "no second fee post");
    assert_eq!(settled_events(&recorder), 1, "no second seam event");

    // One row, one stamp — the redelivery created nothing.
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payment_gateway.gateway_transactions WHERE provider_transaction_id=$1",
    )
    .bind(&txn_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn pgc8_fetcher_transport_error_503_no_write() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let txn_id = uq("MID-TXN");
    let order_id = uq("MID-ORDER");
    let (provider, txn) = seed_pending(
        &pool,
        company,
        "midtrans",
        None,
        &order_id,
        d("1000000"),
        d("30000"),
    )
    .await;

    let (svc, calls, recorder) = make_ingest(
        &pool,
        FakeCreds::failing(),
        StubRefetch::failing(RefetchError::Transport("connect timeout after 10s".into())),
    );

    let before = snap(&pool, company).await;
    let body = midtrans_body(&txn_id, &order_id, 1000000, "settlement");
    let target = target_for("midtrans", provider);
    let headers = HeaderMap::new();
    let req = VerifyRequest {
        headers: &headers,
        request_target: &target,
    };

    let err = svc
        .ingest(
            "midtrans",
            provider,
            VerificationScheme::ApiRefetch,
            body.as_bytes(),
            &req,
        )
        .await
        .unwrap_err();
    assert_eq!(err.http_status(), 503, "got {err:?}");
    assert_eq!(err.code(), "refetch_transport");

    assert_eq!(row_status(&pool, txn).await, "pending");
    assert_eq!(
        snap(&pool, company).await,
        before,
        "a transient outage must not write anything"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(settled_events(&recorder), 0);
}

#[test]
fn pgc9_scheme_declarations_match_routes() {
    // The declaration table the serpa composition mounts — one entry per
    // provider slug, exactly as the route doc headers declare them.
    let declared: &[(&str, VerificationScheme)] = &[
        ("doku", VerificationScheme::HmacRawBody),
        ("manual", VerificationScheme::ApiRefetch),
        ("midtrans", VerificationScheme::ApiRefetch),
        ("xendit", VerificationScheme::ApiRefetch),
    ];

    let registry = GatewayCodecRegistry::with_builtin();
    let mut codes = registry.codes();
    codes.sort_unstable();
    assert_eq!(
        codes,
        vec!["doku", "manual", "midtrans", "xendit"],
        "registry surface is exactly the declaration table"
    );

    for (slug, scheme) in declared {
        let codec = registry
            .lookup(slug)
            .unwrap_or_else(|| panic!("codec for {slug}"));
        assert_eq!(codec.code(), *slug);
        assert_eq!(
            &codec.scheme(),
            scheme,
            "codec for {slug} re-schemed itself"
        );
    }
}

#[tokio::test]
async fn pgc9_scheme_mismatch_refused_in_pipeline() {
    // The route declared ApiRefetch for a provider whose codec is HmacRawBody —
    // refused before verification, before any read that matters.
    let pool = pool().await;
    let company = Uuid::new_v4();
    let txn_id = uq("DOKU-TXN");
    let (provider, txn) = seed_pending(
        &pool,
        company,
        "doku",
        Some("doku-cred-1"),
        &txn_id,
        d("1000000"),
        d("30000"),
    )
    .await;

    let (svc, calls, recorder) = make_ingest(
        &pool,
        FakeCreds::with("doku-cred-1", "SK-DOKU-TEST"),
        StubRefetch::not_settled(),
    );
    let before = snap(&pool, company).await;

    let body = doku_body(&txn_id, 1000000, "SETTLE");
    let target = target_for("doku", provider);
    let headers = HeaderMap::new();
    let req = VerifyRequest {
        headers: &headers,
        request_target: &target,
    };
    let err = svc
        .ingest(
            "doku",
            provider,
            VerificationScheme::ApiRefetch,
            body.as_bytes(),
            &req,
        ) // WRONG declaration
        .await
        .unwrap_err();
    assert_eq!(err.http_status(), 500, "got {err:?}");
    assert_eq!(err.code(), "scheme_mismatch");

    assert_eq!(row_status(&pool, txn).await, "pending");
    assert_eq!(snap(&pool, company).await, before);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(settled_events(&recorder), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// PGW-1 — the money gate, directly
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn pgw1_settle_by_provider_tx_verified_rejects_bad_money() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let recorder = Arc::new(Recorder::default());
    let svc =
        GatewayWriteService::with_sink(pool.clone(), recorder.clone() as Arc<dyn GatewayEventSink>);
    let calls = Arc::new(AtomicU64::new(0));
    let fee = OkFee {
        post: Uuid::new_v4(),
        journal: Uuid::new_v4(),
        calls: calls.clone(),
    };

    // One provider, three transactions (company+code is unique on the table).
    let provider = seed_provider(&pool, company, "midtrans", None).await;

    // (a) authority gross disagrees with the recorded row ⇒ invalid_money,
    //     row untouched, nothing stamped.
    let t1 = uq("MID-TXN");
    let row1 = seed_txn(
        &pool,
        company,
        provider,
        "midtrans",
        &t1,
        d("1000000"),
        d("30000"),
    )
    .await;
    let err = svc
        .settle_by_provider_tx_verified(company, "midtrans", &t1, d("999999"), None, None, &fee)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "invalid_money", "got {err:?}");
    assert_eq!(row_status(&pool, row1).await, "pending");

    // (b) fee exceeding gross ⇒ negative net ⇒ invalid_money before any write.
    let t2 = uq("MID-TXN");
    let row2 = seed_txn(
        &pool,
        company,
        provider,
        "midtrans",
        &t2,
        d("1000000"),
        d("30000"),
    )
    .await;
    let err = svc
        .settle_by_provider_tx_verified(
            company,
            "midtrans",
            &t2,
            d("1000000"),
            Some(d("1500000")),
            None,
            &fee,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), "invalid_money", "got {err:?}");
    assert_eq!(row_status(&pool, row2).await, "pending");

    // Neither failure stamped anything.
    for row in [row1, row2] {
        let stamped: bool = sqlx::query_scalar(
            "SELECT raw_payload IS NOT NULL FROM payment_gateway.gateway_transactions WHERE id=$1",
        )
        .bind(row)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!stamped, "row {row} must not be stamped on a money failure");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(settled_events(&recorder), 0);

    // (c) authority reports no fee ⇒ the row's fee stands, net re-derived, settle OK.
    let t3 = uq("MID-TXN");
    let row3 = seed_txn(
        &pool,
        company,
        provider,
        "midtrans",
        &t3,
        d("1000000"),
        d("30000"),
    )
    .await;
    let out = svc
        .settle_by_provider_tx_verified(company, "midtrans", &t3, d("1000000"), None, None, &fee)
        .await
        .expect("row-fee fallback must settle");
    assert!(!out.already_settled);
    assert_eq!(row_status(&pool, row3).await, "settled");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "fee companion posted from the row fee"
    );
    assert_eq!(settled_events(&recorder), 1);
}
