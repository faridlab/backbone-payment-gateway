//! Gateway golden cases — the pure fee-post oracle (ADR-001 §3).
//!
//! No database: these exercise the pure composer (`compose_fee_post`) and the
//! money invariant directly, proving the fee companion journal balances and that
//! zero-fee / unconfigured-provider settlements correctly skip the companion post.
//! The DB-level exactly-once + end-to-end seam live in `gateway_webhook_probes.rs`
//! and `gateway_seam.rs`.

use backbone_payment_gateway::infrastructure::persistence::{compose_fee_post, compose_fee_reversal, FeeSourceRow};
use backbone_payment_gateway::application::{GatewayWriteService};
use rust_decimal::Decimal;
use uuid::Uuid;

fn src(gross: i64, fee: i64, fee_acc: Option<Uuid>, bank_acc: Option<Uuid>) -> FeeSourceRow {
    FeeSourceRow {
        gateway_transaction_id: Uuid::new_v4(),
        company_id: Uuid::new_v4(),
        provider_transaction_id: "MID-ORDER-123".into(),
        direction: "receive".into(),
        party_type: Some("customer".into()),
        party_id: Some(Uuid::new_v4()),
        gross_amount: Decimal::from(gross),
        fee_amount: Decimal::from(fee),
        net_amount: Decimal::from(gross - fee),
        currency: "IDR".into(),
        reference_no: Some("BANK-REF-9".into()),
        fee_account_id: fee_acc,
        settlement_account_id: bank_acc,
        status: "pending".into(),
        fee_post_id: None,
        payment_entry_id: None,
        provider_code: "midtrans".into(),
    }
}

#[test]
fn ggc1_fee_post_balances_dr_fee_cr_bank() {
    // gross 1,000,000 · fee 30,000 · net 970,000 — the canonical ADR-001 example.
    let fee_acc = Uuid::new_v4();
    let bank_acc = Uuid::new_v4();
    let s = src(1_000_000, 30_000, Some(fee_acc), Some(bank_acc));

    let env = compose_fee_post(&s, chrono::Utc::now().date_naive())
        .expect("a non-zero fee with configured accounts must produce a post");

    // Exactly two lines: Dr Fee Expense · Cr Bank.
    assert_eq!(env.lines.len(), 2);
    assert_eq!(env.lines[0].account_id, fee_acc);
    assert_eq!(env.lines[0].debit, Decimal::from(30_000));
    assert_eq!(env.lines[0].credit, Decimal::ZERO);
    assert_eq!(env.lines[1].account_id, bank_acc);
    assert_eq!(env.lines[1].credit, Decimal::from(30_000));
    assert_eq!(env.lines[1].debit, Decimal::ZERO);

    // Balanced — accounting refuses anything else.
    assert!(env.is_balanced());
    let (dr, cr) = env.totals();
    assert_eq!(dr, Decimal::from(30_000));
    assert_eq!(cr, Decimal::from(30_000));

    // The companion post is keyed on the gateway transaction (idempotent), and
    // tagged so accounting can tell it apart from payment's settlement post.
    assert_eq!(env.source_type, "gateway_fee");
    assert_eq!(env.source_id, s.gateway_transaction_id);
    assert_eq!(env.idempotency_key, s.gateway_transaction_id.to_string());
    assert_eq!(env.posting_type, "original");
}

#[test]
fn ggc2_zero_fee_skips_companion_post() {
    // A zero-fee settlement (e.g. a free/QRIS-flat channel) must NOT post a fee
    // line — payment's settlement post already accounts for the full gross.
    let s = src(500_000, 0, Some(Uuid::new_v4()), Some(Uuid::new_v4()));
    assert!(compose_fee_post(&s, chrono::Utc::now().date_naive()).is_none());
}

#[test]
fn ggc3_unconfigured_provider_skips_companion_post() {
    // Provider exists but its fee/settlement GL accounts aren't configured yet —
    // skip the fee post rather than fail the whole settlement.
    let s = src(1_000_000, 30_000, None, Some(Uuid::new_v4()));
    assert!(compose_fee_post(&s, chrono::Utc::now().date_naive()).is_none());
    let s2 = src(1_000_000, 30_000, Some(Uuid::new_v4()), None);
    assert!(compose_fee_post(&s2, chrono::Utc::now().date_naive()).is_none());
}

#[test]
fn ggc4_net_equals_gross_minus_fee() {
    // The money invariant the gateway guarantees: net = gross − fee.
    assert!(GatewayWriteService::check_money(
        Decimal::from(1_000_000),
        Decimal::from(30_000),
        Decimal::from(970_000),
    )
    .is_ok());

    // A mismatch (e.g. fee silently dropped) must be rejected.
    assert!(GatewayWriteService::check_money(
        Decimal::from(1_000_000),
        Decimal::from(30_000),
        Decimal::from(1_000_000), // forgot to subtract the fee
    )
    .is_err());

    // Negative money is rejected.
    assert!(GatewayWriteService::check_money(
        Decimal::from(-1),
        Decimal::ZERO,
        Decimal::from(-1),
    )
    .is_err());
}

#[test]
fn ggc5_fee_post_carries_party_for_subledger_traceability() {
    // The companion post doesn't touch the A/R control (payment does), but the
    // gross/fee/net are all derivable from the source row the ACL will hand on.
    let s = src(250_000, 5_000, Some(Uuid::new_v4()), Some(Uuid::new_v4()));
    let env = compose_fee_post(&s, chrono::Utc::now().date_naive()).unwrap();
    assert_eq!(env.company_id, s.company_id);
    assert_eq!(env.currency, "IDR");
    // Fee magnitude == both legs of the balanced pair.
    let (dr, cr) = env.totals();
    assert_eq!(dr, s.fee_amount);
    assert_eq!(cr, s.fee_amount);
}

#[test]
fn ggc6_fee_reversal_sign_flips_and_links_original() {
    let original_fee_post = Uuid::new_v4();
    let fee_acc = Uuid::new_v4();
    let bank_acc = Uuid::new_v4();
    let s = FeeSourceRow {
        gateway_transaction_id: Uuid::new_v4(),
        company_id: Uuid::new_v4(),
        provider_transaction_id: "MID-ORDER-REFUND".into(),
        direction: "receive".into(),
        party_type: Some("customer".into()),
        party_id: Some(Uuid::new_v4()),
        gross_amount: Decimal::from(1_000_000),
        fee_amount: Decimal::from(30_000),
        net_amount: Decimal::from(970_000),
        currency: "IDR".into(),
        reference_no: Some("BANK-REF-9".into()),
        fee_account_id: Some(fee_acc),
        settlement_account_id: Some(bank_acc),
        status: "settled".into(),
        fee_post_id: Some(original_fee_post),
        payment_entry_id: Some(Uuid::new_v4()),
        provider_code: "midtrans".into(),
    };
    let env = compose_fee_reversal(&s, chrono::Utc::now().date_naive())
        .expect("non-zero fee + configured accounts must produce a reversal");
    // Sign-flipped: the original was Dr Fee · Cr Bank; the reversal is Dr Bank · Cr Fee.
    assert_eq!(env.lines.len(), 2);
    assert_eq!(env.lines[0].account_id, bank_acc);
    assert_eq!(env.lines[0].debit, Decimal::from(30_000));
    assert_eq!(env.lines[1].account_id, fee_acc);
    assert_eq!(env.lines[1].credit, Decimal::from(30_000));
    assert!(env.is_balanced());
    assert_eq!(env.posting_type, "reversal");
    assert_eq!(env.reverses_post_id, Some(original_fee_post));
    assert_eq!(env.source_type, "gateway_fee");
    assert!(env.idempotency_key.starts_with("reversal:"));
}
