//! The payment-gateway provider abstraction (hand-authored, user-owned) —
//! ADR-003 §2.
//!
//! The module ships ONLY this trait + a registry + two in-process providers
//! (`ManualGatewayProvider` for the legacy operator-keys-`reference_no` flow,
//! `StubGatewayProvider` for tests). Concrete provider SDKs — the real
//! Midtrans/Xendit/DOKU/Stripe HTTP clients + their credentials — are plugged in
//! at the composition/service layer, NEVER in this module. Rationale: keep a
//! stable domain concept decoupled from volatile SDKs and secrets.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::domain::entity::{GatewayProviderCode, GatewayTransactionStatus};

/// Failures a provider can report. Provider SDKs map their own errors onto this.
#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("provider returned not-implemented for this operation")]
    NotImplemented,
    #[error("provider transaction not found: {0}")]
    NotFound(String),
    #[error("provider rejected the request ({code}): {message}")]
    Provider { code: String, message: String },
    #[error("provider transport error: {0}")]
    Transport(String),
}

/// A request to create a charge at a provider.
#[derive(Debug, Clone)]
pub struct CreateChargeRequest {
    pub company_id: Uuid,
    pub amount: Decimal,
    pub currency: String,
    pub reference: String,
    pub description: Option<String>,
}

/// The provider's response to a created charge.
#[derive(Debug, Clone)]
pub struct ChargeCreated {
    pub provider_transaction_id: String,
    pub status: GatewayTransactionStatus,
    /// Checkout redirect / QR URL for redirect-based flows (Midtrans snap, Xendit invoice).
    pub redirect_url: Option<String>,
}

/// A snapshot of a transaction's state at the provider — what a poll or webhook
/// delivers. `fee_amount`/`net_amount` are `None` until the provider reports them
/// (often only at settlement).
#[derive(Debug, Clone)]
pub struct GatewayTxStatus {
    pub status: GatewayTransactionStatus,
    pub gross_amount: Decimal,
    pub fee_amount: Option<Decimal>,
    pub net_amount: Option<Decimal>,
    pub settled_at: Option<DateTime<Utc>>,
}

/// The result of a refund request at the provider.
#[derive(Debug, Clone)]
pub struct RefundResult {
    pub provider_transaction_id: String,
    pub refunded_amount: Decimal,
    pub status: GatewayTransactionStatus,
}

/// The provider port. Real SDK clients implement this; the composition layer
/// registers them into a [`PaymentGatewayRegistry`].
#[async_trait::async_trait]
pub trait PaymentGatewayProvider: Send + Sync {
    async fn create_charge(&self, req: &CreateChargeRequest) -> Result<ChargeCreated, GatewayError>;
    async fn get_status(&self, provider_tx_id: &str) -> Result<GatewayTxStatus, GatewayError>;
    async fn refund(&self, provider_tx_id: &str, amount: Decimal) -> Result<RefundResult, GatewayError>;
    /// Human label for tracing/diagnostics.
    fn name(&self) -> &'static str;
}

/// `code → provider`. The composition layer registers real providers; tests
/// register the stub. `lookup` returns the provider for a code so the write
/// service never branches on provider identity.
#[derive(Default, Clone)]
pub struct PaymentGatewayRegistry {
    providers: Arc<HashMap<GatewayProviderCode, Arc<dyn PaymentGatewayProvider>>>,
}

impl PaymentGatewayRegistry {
    pub fn new() -> Self {
        Self { providers: Arc::new(HashMap::new()) }
    }

    pub fn register(&mut self, code: GatewayProviderCode, provider: Arc<dyn PaymentGatewayProvider>) {
        Arc::make_mut(&mut self.providers).insert(code, provider);
    }

    pub fn lookup(&self, code: &GatewayProviderCode) -> Option<Arc<dyn PaymentGatewayProvider>> {
        self.providers.get(code).cloned()
    }
}

/// The legacy `manual` provider — no real charge is created; an operator keys
/// `reference_no` by hand (the flow backbone-payment ADR-001 shipped with). Its
/// charge is a synthesized id; status stays `pending` until a webhook/operator
/// marks the GatewayTransaction settled.
pub struct ManualGatewayProvider;

#[async_trait::async_trait]
impl PaymentGatewayProvider for ManualGatewayProvider {
    async fn create_charge(&self, req: &CreateChargeRequest) -> Result<ChargeCreated, GatewayError> {
        Ok(ChargeCreated {
            // Synthesized, opaque — the operator later supplies the real reference.
            provider_transaction_id: format!("manual-{}", req.reference),
            status: GatewayTransactionStatus::Pending,
            redirect_url: None,
        })
    }
    async fn get_status(&self, _provider_tx_id: &str) -> Result<GatewayTxStatus, GatewayError> {
        Ok(GatewayTxStatus {
            status: GatewayTransactionStatus::Pending,
            gross_amount: Decimal::ZERO,
            fee_amount: None,
            net_amount: None,
            settled_at: None,
        })
    }
    async fn refund(&self, _provider_tx_id: &str, _amount: Decimal) -> Result<RefundResult, GatewayError> {
        Err(GatewayError::NotImplemented)
    }
    fn name(&self) -> &'static str { "manual" }
}

/// In-memory provider for tests. Stores created charges by id; `settle` lets a
/// test drive a transaction to `settled` with a known fee, then `get_status`
/// reports it — standing in for a real provider + webhook.
pub struct StubGatewayProvider {
    state: Mutex<HashMap<String, GatewayTxStatus>>,
}

impl StubGatewayProvider {
    pub fn new() -> Self {
        Self { state: Mutex::new(HashMap::new()) }
    }

    /// Drive a created charge to settled in a test.
    pub fn settle(
        &self,
        provider_tx_id: &str,
        gross: Decimal,
        fee: Decimal,
        settled_at: DateTime<Utc>,
    ) {
        let mut state = self.state.lock().unwrap();
        state.insert(
            provider_tx_id.to_string(),
            GatewayTxStatus {
                status: GatewayTransactionStatus::Settled,
                gross_amount: gross,
                fee_amount: Some(fee),
                net_amount: Some(gross - fee),
                settled_at: Some(settled_at),
            },
        );
    }
}

impl Default for StubGatewayProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl PaymentGatewayProvider for StubGatewayProvider {
    async fn create_charge(&self, req: &CreateChargeRequest) -> Result<ChargeCreated, GatewayError> {
        let id = format!("stub-{}", Uuid::new_v4());
        self.state.lock().unwrap().insert(
            id.clone(),
            GatewayTxStatus {
                status: GatewayTransactionStatus::Pending,
                gross_amount: req.amount,
                fee_amount: None,
                net_amount: None,
                settled_at: None,
            },
        );
        Ok(ChargeCreated { provider_transaction_id: id, status: GatewayTransactionStatus::Pending, redirect_url: None })
    }
    async fn get_status(&self, provider_tx_id: &str) -> Result<GatewayTxStatus, GatewayError> {
        self.state
            .lock()
            .unwrap()
            .get(provider_tx_id)
            .cloned()
            .ok_or_else(|| GatewayError::NotFound(provider_tx_id.to_string()))
    }
    async fn refund(&self, provider_tx_id: &str, amount: Decimal) -> Result<RefundResult, GatewayError> {
        Ok(RefundResult {
            provider_transaction_id: provider_tx_id.to_string(),
            refunded_amount: amount,
            status: GatewayTransactionStatus::Refunded,
        })
    }
    fn name(&self) -> &'static str { "stub" }
}
