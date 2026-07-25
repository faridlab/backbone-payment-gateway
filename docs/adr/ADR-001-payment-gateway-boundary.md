# ADR-001: backbone-payment-gateway — the external money boundary (provider abstraction, fee line, settlement seam)

**Status**: Accepted — Applied 2026-07-24
**Updated**: 2026-07-25 — §6 resolved (the shared GL-posting contract shipped as `backbone-gl-posting` v0.1.0 / framework v2.7.5; the gateway now uses the shared `AccountingPostEnvelope`/`GlPostSink`); the outbox `company_id` fence relocated from a payment backfill into `backbone-outbox` `multi_tenant` (framework v2.7.4).
**Deciders**: Farid (owner), build session 2026-07-24
**Related**: payment ADR-001 (settlement boundary), payment ADR-002 (settlement seam), the outbox `company_id` fence (`backbone-outbox` `multi_tenant`, framework v2.7.4 — was payment's ADR-0011 backfill), `backbone-gl-posting` (framework v2.7.5)

## Context

`backbone-payment` settles money that has already moved, but it has no concept of HOW money arrived — its only gateway surface is a hand-keyed `reference_no`. Real Indonesian checkout needs Midtrans / Xendit / DOKU / QRIS gateway integration, which carries the two problems payment ADR-001 deliberately parked: **(a)** the gateway FEE introduces a new posting shape, and **(b)** provider webhooks redeliver and need idempotent dedup. A gateway is also a churning, secret-laden integration boundary — folding it into payment would muddy the settlement bounded context. This ADR records the new bounded context that fills that hole.

## Decision

1. **New bounded context `backbone-payment-gateway`.** It owns the provider abstraction, the `GatewayTransaction` record + state machine, webhook ingestion, the fee GL line, and ONE settlement seam event into payment. Zero normal Cargo edges — same serialized-envelope + ACL shape as the billing seam (payment ADR-002). It owns NO masters of its own (party / account / company are logical FKs), exactly like payment. Each table carries its own `company_id` FORCE RLS fence (mirrors payment's allocation/outbox fences).

2. **Concrete providers live at the composition layer, NOT in the module.** The module ships only the `PaymentGatewayProvider` trait + a `PaymentGatewayRegistry` (code → provider) + an in-process `ManualGatewayProvider` (the legacy operator-keys-`reference_no` flow) and a `StubGatewayProvider` (tests). Real Midtrans / Xendit / DOKU / Stripe HTTP clients + credentials are plugged in by the service / composition project. Rationale: keep a stable domain concept decoupled from volatile SDKs and secrets.

3. **The fee is a companion journal, not a mutation of payment's post.** Payment's 2-line settlement post (`Dr Bank · Cr A/R` at gross — unchanged) settles the subledger for the GROSS. The gateway posts a SECOND, independent journal (`source_type = "gateway_fee"`): `Dr Gateway Fee Expense · Cr Bank` for the fee. Net Bank = gross − fee = net; both posts are independently idempotent on `source_id` and independently deduped by accounting. `backbone-payment` is NOT regenerated — the fee gap closes purely additively.

   Example — customer pays IDR 1,000,000, gateway fee IDR 30,000, net IDR 970,000 to bank:

   | Journal | Dr | Cr |
   |---|---|---|
   | PaymentEntry settlement (payment, unchanged) | Bank 1,000,000 | A/R [customer] 1,000,000 |
   | Gateway fee post (gateway, new) | Gateway Fee Expense 30,000 | Bank 30,000 |
   | **Net effect** | Bank +970,000 · Fee Exp +30,000 · A/R −1,000,000 | ✓ balances |

4. **The seam: gateway settles → ACL creates a PaymentEntry.** On a gateway transaction's `pending → settled` transition the gateway emits `GatewayTransactionSettled { gross, fee, net, party, … }` through a `GatewayEventSink`. A composition ACL consumes it and (a) creates a `PaymentEntry` (`paid_amount = gross`) which runs payment's existing settlement post + `PaymentSettled` → billing drawdown, and (b) posts the gateway-fee companion journal. The gateway carries NO invoice allocations — reconciliation to invoices stays payment's job, so an unmapped settlement lands as on-account (payment's existing path).

5. **Webhook idempotency = dedup key + transition-gated emission.** The webhook handler resolves the `GatewayTransaction` by `(provider_code, provider_transaction_id)` — the natural dedup key payment ADR-002 §3 flagged as missing for `apply_settlement`. The `pending → settled` UPDATE uses `rows_affected == 1` to gate the emit, so a redelivered webhook posts the fee and emits the seam event exactly once. The gateway's own `status` transition is the durable record; crash-survival via a shared outbox is the composition layer's concern (the event sink is fire-and-forget in-process today, mirroring payment's current state).

6. **Gateway posts its fee through the shared GL-posting sink.** The fee companion journal is an `AccountingPostEnvelope` (`source_type = "gateway_fee"`) emitted through the shared `GlPostSink` port — now in `backbone-gl-posting` (framework v2.7.5), the single contract payment / selling / inventory / billing also use, so there is zero per-producer duplication and no Cargo edge into accounting. (Originally the gateway carried its own `GatewayFeePostEnvelope` / `GatewayFeeSink` copies to avoid that edge; phase 2 of the dedup replaced them with the shared types.) The composition ACL implements `GlPostSink` by forwarding to accounting, exactly as payment does. Gateway-owned enums (`GatewayPostingState`, `GatewayPartyType`, …) stay gateway-local for the same reason — no schema-level edge.

## Consequences

- **Payment is untouched** — no regeneration, no schema change, no new Cargo edge. The fee gap closes purely additively, and payment's settlement post stays the single owner of the A/R / A-P clearing shape.
- **Gateway is independently composable**: it needs only a Postgres pool, a `GlPostSink` (shared, `backbone-gl-posting`), and a `GatewayEventSink`. The provider SDKs register into a `PaymentGatewayRegistry` at the composition layer.
- **Proven at the unit level** by `tests/gateway_golden_cases.rs` (the pure fee-post composer: gross/fee/net invariant, zero-fee skips the companion post, the envelope balances with `Dr Fee · Cr Bank`). The transition-gated exactly-once and the end-to-end seam (gateway settle → PaymentEntry + fee post → billing drawdown, all three journals balancing) are exercised by `tests/gateway_webhook_probes.rs` and `tests/gateway_seam.rs`, which require a live Postgres + `backbone-payment` as a dev-dependency (same shape as payment's `settlement_seam.rs`).
- **Reversal interplay is deferred.** A gateway refund must reverse BOTH the PaymentEntry and the fee companion post. `GatewayTransactionRefunded` is parked (all-or-nothing, mirroring payment ADR-001 §5 reversal): the `GatewayTransaction` carries `status = refunded`, but the multi-post reversal wiring is a follow-up.
- **Deferred:** multi-currency net / FX on fees, partial refunds, gateway-side allocation hints, real provider SDKs, and the production event-bus ownership of the ACL. (The shared-contracts-crate promotion in §6 is **done** — `backbone-gl-posting` v0.1.0.)

## Parking lot (explicitly out of scope)

Concrete provider SDKs (Midtrans / Xendit / DOKU / QRIS), production event-bus ownership of the ACL, gateway refund / reversal multi-post wiring, fee FX revaluation, gateway-side allocation hints, and the on-account reconciliation UI.
