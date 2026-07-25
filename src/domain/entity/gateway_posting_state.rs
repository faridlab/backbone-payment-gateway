use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "gateway_posting_state", rename_all = "snake_case")]
pub enum GatewayPostingState {
    Pending,
    Posted,
    Failed,
}

impl std::fmt::Display for GatewayPostingState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Posted => write!(f, "posted"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl FromStr for GatewayPostingState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pending" => Ok(Self::Pending),
            "posted" => Ok(Self::Posted),
            "failed" => Ok(Self::Failed),
            _ => Err(format!("Unknown GatewayPostingState variant: {}", s)),
        }
    }
}

impl Default for GatewayPostingState {
    fn default() -> Self {
        Self::Pending
    }
}
