use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "gateway_direction", rename_all = "snake_case")]
pub enum GatewayDirection {
    Receive,
    Pay,
}

impl std::fmt::Display for GatewayDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Receive => write!(f, "receive"),
            Self::Pay => write!(f, "pay"),
        }
    }
}

impl FromStr for GatewayDirection {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "receive" => Ok(Self::Receive),
            "pay" => Ok(Self::Pay),
            _ => Err(format!("Unknown GatewayDirection variant: {}", s)),
        }
    }
}

impl Default for GatewayDirection {
    fn default() -> Self {
        Self::Receive
    }
}
