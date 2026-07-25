use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "gateway_provider_code", rename_all = "snake_case")]
pub enum GatewayProviderCode {
    Manual,
    Midtrans,
    Xendit,
    Doku,
    Stripe,
}

impl std::fmt::Display for GatewayProviderCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Manual => write!(f, "manual"),
            Self::Midtrans => write!(f, "midtrans"),
            Self::Xendit => write!(f, "xendit"),
            Self::Doku => write!(f, "doku"),
            Self::Stripe => write!(f, "stripe"),
        }
    }
}

impl FromStr for GatewayProviderCode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "manual" => Ok(Self::Manual),
            "midtrans" => Ok(Self::Midtrans),
            "xendit" => Ok(Self::Xendit),
            "doku" => Ok(Self::Doku),
            "stripe" => Ok(Self::Stripe),
            _ => Err(format!("Unknown GatewayProviderCode variant: {}", s)),
        }
    }
}

impl Default for GatewayProviderCode {
    fn default() -> Self {
        Self::Manual
    }
}
