//! Hand-written GatewayTransaction SQL (user-owned, never regenerated) — the
//! custom methods on [`GatewayTransactionRepository`]. Sibling to the generated
//! newtype file. Holds the SQL per the module's 4-layer rule: services
//! orchestrate, repositories hold SQL. Mirrors backbone-payment's
//! `PaymentEntryRepository` custom methods (which take the caller's pool/conn so
//! a cross-step write commits as one unit).

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use backbone_orm::company_scope;

use crate::infrastructure::persistence::GatewayTransactionRepository;

/// Everything the fee-post builder needs: the transaction's money + party, and
/// the provider's fee/expense and settlement(bank) GL accounts (joined).
pub struct FeeSourceRow {
    pub gateway_transaction_id: Uuid,
    pub company_id: Uuid,
    pub provider_transaction_id: String,
    /// "receive" | "pay".
    pub direction: String,
    /// "customer" | "supplier" | "employee".
    pub party_type: Option<String>,
    pub party_id: Option<Uuid>,
    pub gross_amount: Decimal,
    pub fee_amount: Decimal,
    pub net_amount: Decimal,
    pub currency: String,
    pub reference_no: Option<String>,
    /// Gateway Fee Expense account (Dr). `None` ⇒ provider not yet configured ⇒ fee post skipped.
    pub fee_account_id: Option<Uuid>,
    /// Bank/Cash account net funds land in (Cr). `None` ⇒ skip.
    pub settlement_account_id: Option<Uuid>,
    /// Lifecycle status ("pending"/"authorized"/"captured"/"settled"/"refunded"/"failed") — read so a
    /// redelivery of an already-terminal transaction short-circuits before re-posting the fee.
    pub status: String,
    /// The original fee companion post id — the reversal's `reverses_post_id`. `None` if zero-fee.
    pub fee_post_id: Option<Uuid>,
    /// The PaymentEntry created on settle (set by the composition ACL). `None` if not yet linked.
    pub payment_entry_id: Option<Uuid>,
    /// Denormalized provider code — for the GatewayTransactionRefunded event.
    pub provider_code: String,
}

/// The settled header the `GatewayTransactionSettled` emission reads.
pub struct SettledHeaderRow {
    pub company_id: Uuid,
    pub provider_code: String,
    pub provider_transaction_id: String,
    pub direction: String,
    pub party_type: Option<String>,
    pub party_id: Option<Uuid>,
    pub gross_amount: Decimal,
    pub fee_amount: Decimal,
    pub net_amount: Decimal,
    pub currency: String,
    pub settled_at: Option<DateTime<Utc>>,
    pub reference_no: Option<String>,
}

impl GatewayTransactionRepository {
    /// Read the transaction + its provider's GL accounts (joined). ID-only: fenced
    /// by the caller's `app.company_id` scope (ADR-0008-style RLS). A non-request
    /// caller MUST wrap in `with_company_scope(Some(company_id))`.
    pub async fn fetch_fee_source(
        &self,
        pool: &PgPool,
        gateway_transaction_id: Uuid,
    ) -> Result<Option<FeeSourceRow>, sqlx::Error> {
        let row = company_scope::fetch_optional_row_scoped(
            pool,
            sqlx::query(
                r#"SELECT gt.id, gt.company_id, gt.provider_transaction_id, gt.direction::text AS dir,
                          gt.party_type::text AS pt, gt.party_id, gt.gross_amount, gt.fee_amount,
                          gt.net_amount, gt.currency, gt.reference_no, gt.status::text AS st,
                          gt.fee_post_id, gt.payment_entry_id, gt.provider_code::text AS pcode,
                          p.fee_account_id, p.settlement_account_id
                   FROM payment_gateway.gateway_transactions gt
                   JOIN payment_gateway.payment_gateway_providers p ON p.id = gt.provider_id
                   WHERE gt.id=$1 AND (gt.metadata->>'deleted_at') IS NULL"#,
            )
            .bind(gateway_transaction_id),
        )
        .await?;
        Ok(row.map(|r| FeeSourceRow {
            gateway_transaction_id: r.get("id"),
            company_id: r.get("company_id"),
            provider_transaction_id: r.get("provider_transaction_id"),
            direction: r.get("dir"),
            party_type: r.get("pt"),
            party_id: r.get("party_id"),
            gross_amount: r.get("gross_amount"),
            fee_amount: r.get("fee_amount"),
            net_amount: r.get("net_amount"),
            currency: r.get("currency"),
            reference_no: r.get("reference_no"),
            fee_account_id: r.get("fee_account_id"),
            settlement_account_id: r.get("settlement_account_id"),
            status: r.get("st"),
            fee_post_id: r.get("fee_post_id"),
            payment_entry_id: r.get("payment_entry_id"),
            provider_code: r.get("pcode"),
        }))
    }

    /// Read the header the seam emission needs, on the CALLER'S transaction (so it
    /// reads the row on the same tx as the transition that just settled it).
    pub async fn fetch_settled_header_on(
        &self,
        conn: &mut sqlx::PgConnection,
        gateway_transaction_id: Uuid,
    ) -> Result<SettledHeaderRow, sqlx::Error> {
        let r = sqlx::query(
            r#"SELECT company_id, provider_code::text AS pcode, provider_transaction_id,
                      direction::text AS dir, party_type::text AS pt, party_id, gross_amount,
                      fee_amount, net_amount, currency, settled_at, reference_no
               FROM payment_gateway.gateway_transactions WHERE id=$1"#,
        )
        .bind(gateway_transaction_id)
        .fetch_one(conn)
        .await?;
        Ok(SettledHeaderRow {
            company_id: r.get("company_id"),
            provider_code: r.get("pcode"),
            provider_transaction_id: r.get("provider_transaction_id"),
            direction: r.get("dir"),
            party_type: r.get("pt"),
            party_id: r.get("party_id"),
            gross_amount: r.get("gross_amount"),
            fee_amount: r.get("fee_amount"),
            net_amount: r.get("net_amount"),
            currency: r.get("currency"),
            settled_at: r.get("settled_at"),
            reference_no: r.get("reference_no"),
        })
    }

    /// Perform the `pending|authorized|captured → settled` transition and stamp the
    /// fee post link. Returns rows affected: the caller gates the seam emission on
    /// this being 1, because the seam creates a PaymentEntry + posts the fee — a
    /// double-emit would double-settle. Takes the CALLER'S connection so the
    /// transition and any outbox stage commit as one unit; the caller has already
    /// bound the company on it (`bind_company_on`).
    pub async fn transition_to_settled(
        &self,
        conn: &mut sqlx::PgConnection,
        gateway_transaction_id: Uuid,
        fee_post_id: Option<Uuid>,
        posting_state: &str,
    ) -> Result<u64, sqlx::Error> {
        let res = sqlx::query(
            r#"UPDATE payment_gateway.gateway_transactions
                  SET status='settled'::gateway_transaction_status,
                      settled_at=now(),
                      fee_post_id=$2,
                      posting_state=$3::gateway_posting_state
                WHERE id=$1
                  AND status IN ('pending','authorized','captured')
                  AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(gateway_transaction_id)
        .bind(fee_post_id)
        .bind(posting_state)
        .execute(conn)
        .await?;
        Ok(res.rows_affected())
    }

    /// Perform the `settled → refunded` transition (all-or-nothing reversal). Returns rows affected:
    /// the caller gates the `GatewayTransactionRefunded` emission on this being 1. Same CALLER'S-
    /// connection contract as [`transition_to_settled`].
    pub async fn transition_to_refunded(
        &self,
        conn: &mut sqlx::PgConnection,
        gateway_transaction_id: Uuid,
    ) -> Result<u64, sqlx::Error> {
        let res = sqlx::query(
            r#"UPDATE payment_gateway.gateway_transactions
                  SET status='refunded'::gateway_transaction_status
                WHERE id=$1
                  AND status='settled'::gateway_transaction_status
                  AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(gateway_transaction_id)
        .execute(conn)
        .await?;
        Ok(res.rows_affected())
    }

    /// Resolve a GatewayTransaction id by the dedup key `(provider_code,
    /// provider_transaction_id)` — what a webhook handler looks up first. Scoped by
    /// the caller's company.
    pub async fn find_id_by_provider_tx(
        &self,
        pool: &PgPool,
        provider_code: &str,
        provider_transaction_id: &str,
    ) -> Result<Option<Uuid>, sqlx::Error> {
        company_scope::fetch_optional_scalar_scoped(
            pool,
            sqlx::query_scalar(
                r#"SELECT id FROM payment_gateway.gateway_transactions
                   WHERE provider_code=$1::gateway_provider_code
                     AND provider_transaction_id=$2
                     AND (metadata->>'deleted_at') IS NULL"#,
            )
            .bind(provider_code)
            .bind(provider_transaction_id),
        )
        .await
    }

    /// Mark the fee post failed (accounting rejected). Caller supplies scope; result
    /// deliberately ignored by the caller (the rejection is the error being reported).
    pub async fn mark_fee_failed(
        &self,
        pool: &PgPool,
        gateway_transaction_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        company_scope::execute_scoped(
            pool,
            sqlx::query(
                "UPDATE payment_gateway.gateway_transactions SET posting_state='failed'::gateway_posting_state WHERE id=$1",
            )
            .bind(gateway_transaction_id),
        )
        .await?;
        Ok(())
    }
}

/// The pure fee-post composer (no DB) — factored out so the golden cases test it
/// directly. Returns `None` when there is nothing to post: a zero fee, or the
/// provider's fee/settlement accounts not yet configured (the fee line is simply
/// skipped — payment's settlement post still runs for the gross).
pub fn compose_fee_post(
    src: &FeeSourceRow,
    posting_date: NaiveDate,
) -> Option<crate::application::service::gateway_gl::AccountingPostEnvelope> {
    use crate::application::service::gateway_gl::{AccountingPostEnvelope, GlPostLine};
    if src.fee_amount <= Decimal::ZERO {
        return None;
    }
    let fee_account = src.fee_account_id?;
    let bank_account = src.settlement_account_id?;
    let lines = vec![
        GlPostLine::debit(fee_account, src.fee_amount)
            .with_description("Gateway fee expense"),
        GlPostLine::credit(bank_account, src.fee_amount)
            .with_description("Gateway fee settled to bank"),
    ];
    Some(AccountingPostEnvelope {
        idempotency_key: src.gateway_transaction_id.to_string(),
        company_id: src.company_id,
        branch_id: None,
        source_type: "gateway_fee".into(),
        source_id: src.gateway_transaction_id,
        source_reference: Some(src.provider_transaction_id.clone()),
        posting_date,
        currency: src.currency.clone(),
        posting_type: "original".into(),
        reverses_post_id: None,
        description: Some(format!("Gateway fee ({})", src.provider_transaction_id)),
        lines,
    })
}

/// The pure fee-REVERSAL post composer — the sign-flipped mirror of [`compose_fee_post`],
/// `posting_type = "reversal"`, linked to the original via `reverses_post_id`. Returns `None` when
/// there is nothing to reverse (zero fee, accounts not configured). `reverses_post_id` is `None` if
/// the original settle posted no fee link — accounting creates a standalone reversal in that case.
pub fn compose_fee_reversal(
    src: &FeeSourceRow,
    posting_date: NaiveDate,
) -> Option<crate::application::service::gateway_gl::AccountingPostEnvelope> {
    use crate::application::service::gateway_gl::{AccountingPostEnvelope, GlPostLine};
    if src.fee_amount <= Decimal::ZERO {
        return None;
    }
    let fee_account = src.fee_account_id?;
    let bank_account = src.settlement_account_id?;
    // Sign-flipped: the original was Dr Fee · Cr Bank; the reversal is Dr Bank · Cr Fee.
    let lines = vec![
        GlPostLine::debit(bank_account, src.fee_amount)
            .with_description("Gateway fee reversal — bank restored"),
        GlPostLine::credit(fee_account, src.fee_amount)
            .with_description("Gateway fee reversal — expense reversed"),
    ];
    Some(AccountingPostEnvelope {
        idempotency_key: format!("reversal:{}", src.gateway_transaction_id),
        company_id: src.company_id,
        branch_id: None,
        source_type: "gateway_fee".into(),
        source_id: src.gateway_transaction_id,
        source_reference: Some(src.provider_transaction_id.clone()),
        posting_date,
        currency: src.currency.clone(),
        posting_type: "reversal".into(),
        reverses_post_id: src.fee_post_id,
        description: Some(format!("Gateway fee reversal ({})", src.provider_transaction_id)),
        lines,
    })
}
