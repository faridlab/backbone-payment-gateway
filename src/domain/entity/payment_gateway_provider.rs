use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use super::GatewayProviderCode;
use super::AuditMetadata;

/// Strongly-typed ID for PaymentGatewayProvider
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PaymentGatewayProviderId(pub Uuid);

impl PaymentGatewayProviderId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for PaymentGatewayProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PaymentGatewayProviderId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for PaymentGatewayProviderId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<PaymentGatewayProviderId> for Uuid {
    fn from(id: PaymentGatewayProviderId) -> Self { id.0 }
}

impl AsRef<Uuid> for PaymentGatewayProviderId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for PaymentGatewayProviderId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PaymentGatewayProvider {
    pub id: Uuid,
    pub code: GatewayProviderCode,
    pub company_id: Uuid,
    pub display_name: String,
    pub credentials_ref: Option<String>,
    pub fee_account_id: Option<Uuid>,
    pub settlement_account_id: Option<Uuid>,
    pub is_active: bool,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl PaymentGatewayProvider {
    /// Create a builder for PaymentGatewayProvider
    pub fn builder() -> PaymentGatewayProviderBuilder {
        PaymentGatewayProviderBuilder::default()
    }

    /// Create a new PaymentGatewayProvider with required fields
    pub fn new(code: GatewayProviderCode, company_id: Uuid, display_name: String, is_active: bool) -> Self {
        Self {
            id: Uuid::new_v4(),
            code,
            company_id,
            display_name,
            credentials_ref: None,
            fee_account_id: None,
            settlement_account_id: None,
            is_active,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> PaymentGatewayProviderId {
        PaymentGatewayProviderId(self.id)
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


    // ==========================================================
    // Fluent Setters (with_* for optional fields)
    // ==========================================================

    /// Set the credentials_ref field (chainable)
    pub fn with_credentials_ref(mut self, value: String) -> Self {
        self.credentials_ref = Some(value);
        self
    }

    /// Set the fee_account_id field (chainable)
    pub fn with_fee_account_id(mut self, value: Uuid) -> Self {
        self.fee_account_id = Some(value);
        self
    }

    /// Set the settlement_account_id field (chainable)
    pub fn with_settlement_account_id(mut self, value: Uuid) -> Self {
        self.settlement_account_id = Some(value);
        self
    }

    // ==========================================================
    // Partial Update
    // ==========================================================

    /// Apply partial updates from a map of field name to JSON value
    pub fn apply_patch(&mut self, fields: std::collections::HashMap<String, serde_json::Value>) {
        for (key, value) in fields {
            match key.as_str() {
                "code" => {
                    if let Ok(v) = serde_json::from_value(value) { self.code = v; }
                }
                "company_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.company_id = v; }
                }
                "display_name" => {
                    if let Ok(v) = serde_json::from_value(value) { self.display_name = v; }
                }
                "credentials_ref" => {
                    if let Ok(v) = serde_json::from_value(value) { self.credentials_ref = v; }
                }
                "fee_account_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.fee_account_id = v; }
                }
                "settlement_account_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.settlement_account_id = v; }
                }
                "is_active" => {
                    if let Ok(v) = serde_json::from_value(value) { self.is_active = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for PaymentGatewayProvider {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "PaymentGatewayProvider"
    }
}

impl backbone_core::PersistentEntity for PaymentGatewayProvider {
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

impl backbone_orm::EntityRepoMeta for PaymentGatewayProvider {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("company_id".to_string(), "uuid".to_string());
        m.insert("fee_account_id".to_string(), "uuid".to_string());
        m.insert("settlement_account_id".to_string(), "uuid".to_string());
        m.insert("code".to_string(), "gateway_provider_code".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &["display_name"]
    }
    fn company_field() -> Option<&'static str> {
        Some("company_id")
    }
}

/// Builder for PaymentGatewayProvider entity
///
/// Provides a fluent API for constructing PaymentGatewayProvider instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct PaymentGatewayProviderBuilder {
    code: Option<GatewayProviderCode>,
    company_id: Option<Uuid>,
    display_name: Option<String>,
    credentials_ref: Option<String>,
    fee_account_id: Option<Uuid>,
    settlement_account_id: Option<Uuid>,
    is_active: Option<bool>,
}

impl PaymentGatewayProviderBuilder {
    /// Set the code field (required)
    pub fn code(mut self, value: GatewayProviderCode) -> Self {
        self.code = Some(value);
        self
    }

    /// Set the company_id field (required)
    pub fn company_id(mut self, value: Uuid) -> Self {
        self.company_id = Some(value);
        self
    }

    /// Set the display_name field (required)
    pub fn display_name(mut self, value: String) -> Self {
        self.display_name = Some(value);
        self
    }

    /// Set the credentials_ref field (optional)
    pub fn credentials_ref(mut self, value: String) -> Self {
        self.credentials_ref = Some(value);
        self
    }

    /// Set the fee_account_id field (optional)
    pub fn fee_account_id(mut self, value: Uuid) -> Self {
        self.fee_account_id = Some(value);
        self
    }

    /// Set the settlement_account_id field (optional)
    pub fn settlement_account_id(mut self, value: Uuid) -> Self {
        self.settlement_account_id = Some(value);
        self
    }

    /// Set the is_active field (default: `true`)
    pub fn is_active(mut self, value: bool) -> Self {
        self.is_active = Some(value);
        self
    }

    /// Build the PaymentGatewayProvider entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<PaymentGatewayProvider, String> {
        let code = self.code.ok_or_else(|| "code is required".to_string())?;
        let company_id = self.company_id.ok_or_else(|| "company_id is required".to_string())?;
        let display_name = self.display_name.ok_or_else(|| "display_name is required".to_string())?;

        Ok(PaymentGatewayProvider {
            id: Uuid::new_v4(),
            code,
            company_id,
            display_name,
            credentials_ref: self.credentials_ref,
            fee_account_id: self.fee_account_id,
            settlement_account_id: self.settlement_account_id,
            is_active: self.is_active.unwrap_or(true),
            metadata: AuditMetadata::default(),
        })
    }
}
