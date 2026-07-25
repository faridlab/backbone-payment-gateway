<!-- 2026-07-25 | repo: module | unit: backbone-payment-gateway | focus: maturity | roster: chair, skeptic, steelman, yagni-business (standing); ddd-bounded-context, contract-seat (module context); domain-expert (invited). skeptic/steelman/chair ran as isolated subagents. -->

# Council — module:backbone-payment-gateway — focus: maturity

## Best call
Wire `GatewayWriteService` construction and the webhook router into `PaymentGatewayModuleBuilder::build()` (add `with_fee_sink` / `with_event_sink` builder methods; expose `gateway_webhook_router()` on `PaymentGatewayModule`) so the engine the ADR calls "Applied 2026-07-24" is actually reachable from the module's public API. Today `build()` ([lib.rs:114-135](src/lib.rs#L114)) yields ONLY two generic-CRUD services; `GatewayWriteService` is constructed only inside `tests/` ([gateway_webhook_probes.rs:116](tests/gateway_webhook_probes.rs#L116),137,180; [gateway_seam.rs:209](tests/gateway_seam.rs#L209)) and `create_gateway_webhook_routes` is exported but never mounted by the module itself. That gap is the maturity blocker — the engine is the proven, three-level-tested asset every seat agrees on, yet no consumer can obtain it from `PaymentGatewayModule`.

- Residual negative value: time ~half a day to add the builder methods + one integration test that mounts the router via the module. You still take: (a) `gateway_provider.rs` remains dead internally with a shadow `GatewayError` enum ([gateway_provider.rs:21-31](src/application/service/gateway_provider.rs#L21) vs [gateway_write_service.rs:50-58](src/application/service/gateway_write_service.rs#L50)) — ~3h to demote later; (b) refund-of-pending returns `unsupported_currency` on the wire ([gateway_write_service.rs:245-247](src/application/service/gateway_write_service.rs#L245)) — misleading, ~30 min; (c) failed/captured/authorized still unreachable states — real domain work, deferred.
- Reversibility: easy — the change is additive builder methods + one module method; delete them and you're back to today.
- What would flip this: a probe across composition repos (`rg 'GatewayWriteService|create_gateway_webhook_routes'` in any consuming service). Zero non-test hits means no composition consumer is on the near roadmap — in that case the Best call flips to **demote `gateway_provider.rs` behind a `provider-sdk` feature flag and amend ADR-001 to "Settlement engine surfaced to composition pending"**, shipping the engine as a manual-construction API only. The probe is cheap; if you have access, run it before this change lands.

## Disagreement map
The real tensions (2–3 max). For each: the crux, and who is on each side.

- **Is the module "complete for its declared seam" (steelman) or "shipping a fiction boundary" (skeptic)?** — Steelman + domain-expert say the exactly-once engine is done and proven at three levels, so the module is complete and the rest is composition's job. Skeptic + ddd-bc say the ADR's "Applied" claim is contradicted by `build()` not yielding the engine and the provider port having zero internal call sites. Crux: whether "complete" requires the engine to be reachable from the module's own public API, or only to exist as a hand-constructed service. The code side has won: build() does not surface the engine, so the steelman's C1 ("composition actually consumes the provider port") is unverifiable AND the engine itself is unwired — two layers of fiction, not one.

- **Keep vs demote the provider abstraction (`gateway_provider.rs`).** — Yagni-business + ddd-bc + skeptic say demote (speculation before any real SDK; creates the second `GatewayError`, misleads readers, dead-state smell). Steelman says keep (intentional hexagonal port per ADR §2; same posture as GlPostSink/GatewayEventSink). Crux: is there a scheduled real-provider integration (Midtrans/Xendit) within the next 1–2 sprints. This is genuinely fact-dependent — the probe above settles it.

- **Is the implicit idempotency contract on `GlPostSink` acceptable?** — Contract-seat flags it as a hazard (fee post fires OUTSIDE the transition tx at [gateway_write_service.rs:171-180](src/application/service/gateway_write_service.rs#L171); exactly-once depends on downstream dedup of `source_id`/`idempotency_key`, which the port doc does not state). Steelman + domain-expert accept it (matches payment's own posture; `idempotency_key = gateway_transaction_id` is deterministic). Crux: does the shared `backbone-gl-posting` crate document the dedup guarantee. If it does, contract-seat's hazard dissolves; if it doesn't, it's a must-fix on a safety-critical path.

## Recommendations (ranked by leverage)
| # | Move | Leverage | Residual negative | Reversibility | Evidence to flip |
|---|------|----------|-------------------|---------------|------------------|
| 1 | Wire `GatewayWriteService` + webhook router into `build()`; expose `gateway_webhook_router()` | Unblocks the seam the ADR claims is applied; resolves the load-bearing ADR-vs-code contradiction | ~0.5 day; provider abstraction still dead; misleading refund wire error remains | Easy (additive) | Probe shows zero composition consumers in 1–2 sprints → demote instead |
| 2 | Fix the misleading wire error: refund-of-pending should return a status error code (e.g. `invalid_status`), not `unsupported_currency` | Removes a real consumer-confusion bug on a safety path; ~30 min | None | Easy | — |
| 3 | Demote `gateway_provider.rs` (trait + registry + Manual + Stub + second `GatewayError`) behind a `provider-sdk` feature flag until a real SDK lands | Removes dead code, duplicate error enum, dead-state smell; ~3h | If a real Midtrans/Xendit integration lands next sprint, you re-introduce it | Easy (feature-gated) | Scheduled provider integration exists in roadmap → keep as-is |
| 4 | Declare the dedup contract on `GlPostSink` explicitly in `gateway_gl.rs` re-export doc (or upstream in `backbone-gl-posting`) | Closes the implicit-contract hazard on a money path; ~1h | None | Easy | Upstream `backbone-gl-posting` already documents it → no-op, withdraw the move |
| 5 | Add `failed` reachability (webhook status mapping + transition) so declined/expired charges land in the record | Real domain fidelity (retry/comms/recon); closes the dead-state gap | Larger; schema + webhook + transition; ~1–2 days | Costly (schema state machine widening) | Business confirms declined-charge tracking is out of scope this quarter → park |

## Maturity scorecard
Score each seated TECHNICAL seat on ITS OWN axis (1–5). One sentence why.

| Seat | Axis | Score (1–5) | One sentence why |
|------|------|-------------|------------------|
| ddd-bounded-context | bounded-context cleanliness | 3 | Cross-module boundary is exemplary (zero Cargo edges, per-table `company_id` RLS fence, shared `GlPostSink`); intra-module is messy — orphaned `PaymentGatewayProvider` port with zero internal call sites and a 6-state enum where only 3 are produced. |
| contract-seat | cross-module contract quality | 3 | Settlement seam payload is strong (`gross/fee/net/party`, deterministic `idempotency_key`, fire-and-forget sink), but three hazards land on a money path: implicit dedup contract on `GlPostSink`, two `GatewayError` enums with the same name, and refund-of-pending masquerading as `unsupported_currency`. |
| domain-expert | domain fidelity for ID gateway ops | 3 | Nails the genuinely hard parts (fee companion journal additive to gross, `(provider_code, provider_transaction_id)` dedup matching how Midtrans/Xendit order IDs actually work, transition-gated exactly-once), but auth/capture is collapsed for card flows, `failed` is never recorded, and there is no payout/batch reconciliation concept. |
| yagni-business | payoff-at-current-scale | 3 | The settlement+fee+reversal engine removes real GL pain the moment any gateway posts a fee, but `gateway_provider.rs` (trait + registry + Manual + Stub + second error) is pure speculation with no present consumer — net-positive engine payoff offset by dead-abstraction cost. |
| steelman | design integrity | 4 | The exactly-once money-movement design across the transition `rows_affected==1` gate is correct and proven at three test levels, but it rests on C1–C3 being true at composition — conditions unverifiable from this repo. |
| skeptic | assumption soundness | 2 | The load-bearing assumption ("module owns settlement; composition owns charge creation", ADR §2) is contradicted by the code: `build()` does not surface the engine, `PaymentGatewayRegistry`/`create_charge` have zero non-test call sites, and the webhook swallows everything except `status=="settled"` — making the ADR's "Applied" status not-yet-true from the module's own surface. |

## Parking lot
Ideas raised but outside this run's focus.

- Auth/capture split for card flows (Midtrans/Xendit `authorize → capture → settle`) — raised by domain-expert, scope: gateway domain state model (schema YAML + transitions).
- `failed`/declined/expired charge recording for retry, comms, and recon — raised by domain-expert + ddd-bc, scope: gateway state machine + webhook status mapping.
- Payout/batch reconciliation (`payout_id`, `batch_id`, T+1..T+7 net settlement) — raised by domain-expert, scope: new entity or extension to GatewayTransaction.
- Production event bus for `GatewayEventSink` (currently `LoggingGatewaySink`) — raised by steelman, scope: composition.
- Reconciliation poller as defense against unreliable webhook delivery — raised by steelman (C3), scope: composition or a gateway jobs module.
- FX / partial refunds / allocation hints — raised by steelman as explicit scope discipline, scope: gateway.
