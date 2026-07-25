//! Normalized webhook ingestion endpoint (hand-authored, user-owned) — ADR-003 §5.
//!
//! Provider-specific ingestion — Midtrans/Xendit signature verification, payload
//! parsing, and the mapping from a provider's native notification to the
//! normalized shape below — lives at the COMPOSITION layer (the service that owns
//! the provider SDKs). This module exposes the provider-agnostic settle endpoint
//! a composition adapter calls once it has validated and normalized a callback.
//!
//! The endpoint locates the GatewayTransaction by the dedup key
//! `(provider_code, provider_transaction_id)` and runs the transition-gated
//! settle: a redelivered webhook finds an already-settled row and is a no-op
//! (`already_settled: true`), never double-posting the fee or double-emitting the
//! seam event.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::application::service::{GlPostSink, GatewayWriteService};

/// A normalized provider notification. The composition adapter produces this from
/// the raw provider payload after signature verification.
#[derive(Debug, Deserialize)]
pub struct WebhookNotification {
    /// "manual" | "midtrans" | "xendit" | "doku" | "stripe".
    pub provider_code: String,
    /// The gateway's own transaction id — the dedup key.
    pub provider_transaction_id: String,
    /// "settled" triggers settlement; any other value is accepted but does not
    /// settle (the row stays in its current lifecycle state).
    #[serde(default)]
    pub status: String,
}

/// The settle result returned to the caller.
#[derive(Debug, Serialize)]
pub struct WebhookResponse {
    pub provider_code: String,
    pub provider_transaction_id: String,
    pub settled: bool,
    pub already_settled: bool,
}

/// State injected into the webhook route: the write service + the fee sink (the
/// fee sink is composition-provided — the module ships no real accounting client).
#[derive(Clone)]
pub struct WebhookState {
    pub write_service: Arc<GatewayWriteService>,
    pub fee_sink: Arc<dyn GlPostSink>,
}

/// `POST /payment-gateway/webhook/settle` — settle a gateway transaction by its
/// provider dedup key. Idempotent: a redelivery is a no-op.
pub async fn settle_webhook(
    State(state): State<WebhookState>,
    Json(notif): Json<WebhookNotification>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if notif.status != "settled" {
        // Not a settlement notification — acknowledge but take no action.
        return Ok((
            StatusCode::OK,
            Json(WebhookResponse {
                provider_code: notif.provider_code,
                provider_transaction_id: notif.provider_transaction_id,
                settled: false,
                already_settled: false,
            }),
        ));
    }
    match state
        .write_service
        .settle_by_provider_tx(&notif.provider_code, &notif.provider_transaction_id, &*state.fee_sink)
        .await
    {
        Ok(outcome) => Ok((
            StatusCode::OK,
            Json(WebhookResponse {
                provider_code: notif.provider_code,
                provider_transaction_id: notif.provider_transaction_id,
                settled: !outcome.already_settled,
                already_settled: outcome.already_settled,
            }),
        )),
        Err(e) => {
            let status = StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            Err((status, e.to_string()))
        }
    }
}

/// Compose the webhook router. The composition layer mounts this with its own
/// fee-sink implementation and sits behind provider-specific verification.
pub fn create_gateway_webhook_routes(state: WebhookState) -> Router {
    Router::new().route("/webhook/settle", post(settle_webhook)).with_state(state)
}
