use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use rust_decimal::Decimal;

use super::GatewayProviderCode;
use super::GatewayDirection;
use super::GatewayPartyType;
use super::GatewayTransactionStatus;
use super::GatewayPostingState;
use super::AuditMetadata;

/// Strongly-typed ID for GatewayTransaction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GatewayTransactionId(pub Uuid);

impl GatewayTransactionId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for GatewayTransactionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for GatewayTransactionId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for GatewayTransactionId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<GatewayTransactionId> for Uuid {
    fn from(id: GatewayTransactionId) -> Self { id.0 }
}

impl AsRef<Uuid> for GatewayTransactionId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for GatewayTransactionId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct GatewayTransaction {
    pub id: Uuid,
    pub company_id: Uuid,
    pub provider_id: Uuid,
    pub provider_code: GatewayProviderCode,
    pub provider_transaction_id: String,
    pub direction: GatewayDirection,
    pub party_type: Option<GatewayPartyType>,
    pub party_id: Option<Uuid>,
    pub gross_amount: Decimal,
    pub fee_amount: Decimal,
    pub net_amount: Decimal,
    pub currency: String,
    pub status: GatewayTransactionStatus,
    pub posting_state: GatewayPostingState,
    pub payment_entry_id: Option<Uuid>,
    pub fee_post_id: Option<Uuid>,
    pub settled_at: Option<DateTime<Utc>>,
    pub reference_no: Option<String>,
    pub raw_payload: Option<serde_json::Value>,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl GatewayTransaction {
    /// Create a builder for GatewayTransaction
    pub fn builder() -> GatewayTransactionBuilder {
        GatewayTransactionBuilder::default()
    }

    /// Create a new GatewayTransaction with required fields
    pub fn new(company_id: Uuid, provider_id: Uuid, provider_code: GatewayProviderCode, provider_transaction_id: String, direction: GatewayDirection, gross_amount: Decimal, fee_amount: Decimal, net_amount: Decimal, currency: String, status: GatewayTransactionStatus, posting_state: GatewayPostingState) -> Self {
        Self {
            id: Uuid::new_v4(),
            company_id,
            provider_id,
            provider_code,
            provider_transaction_id,
            direction,
            party_type: None,
            party_id: None,
            gross_amount,
            fee_amount,
            net_amount,
            currency,
            status,
            posting_state,
            payment_entry_id: None,
            fee_post_id: None,
            settled_at: None,
            reference_no: None,
            raw_payload: None,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> GatewayTransactionId {
        GatewayTransactionId(self.id)
    }

    /// Get when this entity was created
    pub fn created_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.created_at.as_ref()
    }

    /// Get when this entity was last updated
    pub fn updated_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.updated_at.as_ref()
    }

    /// Check if this entity is soft deleted
    pub fn is_deleted(&self) -> bool {
        self.metadata.deleted_at.is_some()
    }

    /// Check if this entity is active (not deleted)
    pub fn is_active(&self) -> bool {
        self.metadata.deleted_at.is_none()
    }

    /// Get when this entity was deleted
    pub fn deleted_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.deleted_at.as_ref()
    }

    /// Get who created this entity
    pub fn created_by(&self) -> Option<&Uuid> {
        self.metadata.created_by.as_ref()
    }

    /// Get who last updated this entity
    pub fn updated_by(&self) -> Option<&Uuid> {
        self.metadata.updated_by.as_ref()
    }

    /// Get who deleted this entity
    pub fn deleted_by(&self) -> Option<&Uuid> {
        self.metadata.deleted_by.as_ref()
    }

    /// Get the current status
    pub fn status(&self) -> &GatewayTransactionStatus {
        &self.status
    }


    // ==========================================================
    // Fluent Setters (with_* for optional fields)
    // ==========================================================

    /// Set the party_type field (chainable)
    pub fn with_party_type(mut self, value: GatewayPartyType) -> Self {
        self.party_type = Some(value);
        self
    }

    /// Set the party_id field (chainable)
    pub fn with_party_id(mut self, value: Uuid) -> Self {
        self.party_id = Some(value);
        self
    }

    /// Set the payment_entry_id field (chainable)
    pub fn with_payment_entry_id(mut self, value: Uuid) -> Self {
        self.payment_entry_id = Some(value);
        self
    }

    /// Set the fee_post_id field (chainable)
    pub fn with_fee_post_id(mut self, value: Uuid) -> Self {
        self.fee_post_id = Some(value);
        self
    }

    /// Set the settled_at field (chainable)
    pub fn with_settled_at(mut self, value: DateTime<Utc>) -> Self {
        self.settled_at = Some(value);
        self
    }

    /// Set the reference_no field (chainable)
    pub fn with_reference_no(mut self, value: String) -> Self {
        self.reference_no = Some(value);
        self
    }

    /// Set the raw_payload field (chainable)
    pub fn with_raw_payload(mut self, value: serde_json::Value) -> Self {
        self.raw_payload = Some(value);
        self
    }

    // ==========================================================
    // Partial Update
    // ==========================================================

    /// Apply partial updates from a map of field name to JSON value
    pub fn apply_patch(&mut self, fields: std::collections::HashMap<String, serde_json::Value>) {
        for (key, value) in fields {
            match key.as_str() {
                "company_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.company_id = v; }
                }
                "provider_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.provider_id = v; }
                }
                "provider_code" => {
                    if let Ok(v) = serde_json::from_value(value) { self.provider_code = v; }
                }
                "provider_transaction_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.provider_transaction_id = v; }
                }
                "direction" => {
                    if let Ok(v) = serde_json::from_value(value) { self.direction = v; }
                }
                "party_type" => {
                    if let Ok(v) = serde_json::from_value(value) { self.party_type = v; }
                }
                "party_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.party_id = v; }
                }
                "gross_amount" => {
                    if let Ok(v) = serde_json::from_value(value) { self.gross_amount = v; }
                }
                "fee_amount" => {
                    if let Ok(v) = serde_json::from_value(value) { self.fee_amount = v; }
                }
                "net_amount" => {
                    if let Ok(v) = serde_json::from_value(value) { self.net_amount = v; }
                }
                "currency" => {
                    if let Ok(v) = serde_json::from_value(value) { self.currency = v; }
                }
                "status" => {
                    if let Ok(v) = serde_json::from_value(value) { self.status = v; }
                }
                "posting_state" => {
                    if let Ok(v) = serde_json::from_value(value) { self.posting_state = v; }
                }
                "payment_entry_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.payment_entry_id = v; }
                }
                "fee_post_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.fee_post_id = v; }
                }
                "settled_at" => {
                    if let Ok(v) = serde_json::from_value(value) { self.settled_at = v; }
                }
                "reference_no" => {
                    if let Ok(v) = serde_json::from_value(value) { self.reference_no = v; }
                }
                "raw_payload" => {
                    if let Ok(v) = serde_json::from_value(value) { self.raw_payload = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for GatewayTransaction {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "GatewayTransaction"
    }
}

impl backbone_core::PersistentEntity for GatewayTransaction {
    fn entity_id(&self) -> String {
        self.id.to_string()
    }
    fn set_entity_id(&mut self, id: String) {
        if let Ok(uuid) = uuid::Uuid::parse_str(&id) {
            self.id = uuid;
        }
    }
    fn created_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.created_at
    }
    fn set_created_at(&mut self, ts: chrono::DateTime<chrono::Utc>) {
        self.metadata.created_at = Some(ts);
    }
    fn updated_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.updated_at
    }
    fn set_updated_at(&mut self, ts: chrono::DateTime<chrono::Utc>) {
        self.metadata.updated_at = Some(ts);
    }
    fn deleted_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.deleted_at
    }
    fn set_deleted_at(&mut self, ts: Option<chrono::DateTime<chrono::Utc>>) {
        self.metadata.deleted_at = ts;
    }
}

impl backbone_orm::EntityRepoMeta for GatewayTransaction {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("company_id".to_string(), "uuid".to_string());
        m.insert("provider_id".to_string(), "uuid".to_string());
        m.insert("party_id".to_string(), "uuid".to_string());
        m.insert("payment_entry_id".to_string(), "uuid".to_string());
        m.insert("fee_post_id".to_string(), "uuid".to_string());
        m.insert("provider_code".to_string(), "gateway_provider_code".to_string());
        m.insert("direction".to_string(), "gateway_direction".to_string());
        m.insert("party_type".to_string(), "gateway_party_type".to_string());
        m.insert("status".to_string(), "gateway_transaction_status".to_string());
        m.insert("posting_state".to_string(), "gateway_posting_state".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &["provider_transaction_id", "currency"]
    }
    fn company_field() -> Option<&'static str> {
        Some("company_id")
    }
}

/// Builder for GatewayTransaction entity
///
/// Provides a fluent API for constructing GatewayTransaction instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct GatewayTransactionBuilder {
    company_id: Option<Uuid>,
    provider_id: Option<Uuid>,
    provider_code: Option<GatewayProviderCode>,
    provider_transaction_id: Option<String>,
    direction: Option<GatewayDirection>,
    party_type: Option<GatewayPartyType>,
    party_id: Option<Uuid>,
    gross_amount: Option<Decimal>,
    fee_amount: Option<Decimal>,
    net_amount: Option<Decimal>,
    currency: Option<String>,
    status: Option<GatewayTransactionStatus>,
    posting_state: Option<GatewayPostingState>,
    payment_entry_id: Option<Uuid>,
    fee_post_id: Option<Uuid>,
    settled_at: Option<DateTime<Utc>>,
    reference_no: Option<String>,
    raw_payload: Option<serde_json::Value>,
}

impl GatewayTransactionBuilder {
    /// Set the company_id field (required)
    pub fn company_id(mut self, value: Uuid) -> Self {
        self.company_id = Some(value);
        self
    }

    /// Set the provider_id field (required)
    pub fn provider_id(mut self, value: Uuid) -> Self {
        self.provider_id = Some(value);
        self
    }

    /// Set the provider_code field (required)
    pub fn provider_code(mut self, value: GatewayProviderCode) -> Self {
        self.provider_code = Some(value);
        self
    }

    /// Set the provider_transaction_id field (required)
    pub fn provider_transaction_id(mut self, value: String) -> Self {
        self.provider_transaction_id = Some(value);
        self
    }

    /// Set the direction field (default: `GatewayDirection::default()`)
    pub fn direction(mut self, value: GatewayDirection) -> Self {
        self.direction = Some(value);
        self
    }

    /// Set the party_type field (optional)
    pub fn party_type(mut self, value: GatewayPartyType) -> Self {
        self.party_type = Some(value);
        self
    }

    /// Set the party_id field (optional)
    pub fn party_id(mut self, value: Uuid) -> Self {
        self.party_id = Some(value);
        self
    }

    /// Set the gross_amount field (required)
    pub fn gross_amount(mut self, value: Decimal) -> Self {
        self.gross_amount = Some(value);
        self
    }

    /// Set the fee_amount field (default: `Decimal::from(0)`)
    pub fn fee_amount(mut self, value: Decimal) -> Self {
        self.fee_amount = Some(value);
        self
    }

    /// Set the net_amount field (required)
    pub fn net_amount(mut self, value: Decimal) -> Self {
        self.net_amount = Some(value);
        self
    }

    /// Set the currency field (default: `"IDR".to_string()`)
    pub fn currency(mut self, value: String) -> Self {
        self.currency = Some(value);
        self
    }

    /// Set the status field (default: `GatewayTransactionStatus::default()`)
    pub fn status(mut self, value: GatewayTransactionStatus) -> Self {
        self.status = Some(value);
        self
    }

    /// Set the posting_state field (default: `GatewayPostingState::default()`)
    pub fn posting_state(mut self, value: GatewayPostingState) -> Self {
        self.posting_state = Some(value);
        self
    }

    /// Set the payment_entry_id field (optional)
    pub fn payment_entry_id(mut self, value: Uuid) -> Self {
        self.payment_entry_id = Some(value);
        self
    }

    /// Set the fee_post_id field (optional)
    pub fn fee_post_id(mut self, value: Uuid) -> Self {
        self.fee_post_id = Some(value);
        self
    }

    /// Set the settled_at field (optional)
    pub fn settled_at(mut self, value: DateTime<Utc>) -> Self {
        self.settled_at = Some(value);
        self
    }

    /// Set the reference_no field (optional)
    pub fn reference_no(mut self, value: String) -> Self {
        self.reference_no = Some(value);
        self
    }

    /// Set the raw_payload field (optional)
    pub fn raw_payload(mut self, value: serde_json::Value) -> Self {
        self.raw_payload = Some(value);
        self
    }

    /// Build the GatewayTransaction entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<GatewayTransaction, String> {
        let company_id = self.company_id.ok_or_else(|| "company_id is required".to_string())?;
        let provider_id = self.provider_id.ok_or_else(|| "provider_id is required".to_string())?;
        let provider_code = self.provider_code.ok_or_else(|| "provider_code is required".to_string())?;
        let provider_transaction_id = self.provider_transaction_id.ok_or_else(|| "provider_transaction_id is required".to_string())?;
        let gross_amount = self.gross_amount.ok_or_else(|| "gross_amount is required".to_string())?;
        let net_amount = self.net_amount.ok_or_else(|| "net_amount is required".to_string())?;

        Ok(GatewayTransaction {
            id: Uuid::new_v4(),
            company_id,
            provider_id,
            provider_code,
            provider_transaction_id,
            direction: self.direction.unwrap_or(GatewayDirection::default()),
            party_type: self.party_type,
            party_id: self.party_id,
            gross_amount,
            fee_amount: self.fee_amount.unwrap_or(Decimal::from(0)),
            net_amount,
            currency: self.currency.unwrap_or("IDR".to_string()),
            status: self.status.unwrap_or(GatewayTransactionStatus::default()),
            posting_state: self.posting_state.unwrap_or(GatewayPostingState::default()),
            payment_entry_id: self.payment_entry_id,
            fee_post_id: self.fee_post_id,
            settled_at: self.settled_at,
            reference_no: self.reference_no,
            raw_payload: self.raw_payload,
            metadata: AuditMetadata::default(),
        })
    }
}
