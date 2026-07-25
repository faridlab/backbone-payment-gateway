use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "gateway_transaction_status", rename_all = "snake_case")]
pub enum GatewayTransactionStatus {
    Pending,
    Authorized,
    Captured,
    Settled,
    Refunded,
    Failed,
}

impl std::fmt::Display for GatewayTransactionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Authorized => write!(f, "authorized"),
            Self::Captured => write!(f, "captured"),
            Self::Settled => write!(f, "settled"),
            Self::Refunded => write!(f, "refunded"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl FromStr for GatewayTransactionStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pending" => Ok(Self::Pending),
            "authorized" => Ok(Self::Authorized),
            "captured" => Ok(Self::Captured),
            "settled" => Ok(Self::Settled),
            "refunded" => Ok(Self::Refunded),
            "failed" => Ok(Self::Failed),
            _ => Err(format!("Unknown GatewayTransactionStatus variant: {}", s)),
        }
    }
}

impl Default for GatewayTransactionStatus {
    fn default() -> Self {
        Self::Pending
    }
}
