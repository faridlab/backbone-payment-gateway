// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

#[cfg(feature = "codecs")]
use application::service::{
    CredentialReader, GatewayCodecRegistry, StatusRefetch, WebhookIngestService,
};
use application::service::{GatewayEventSink, GatewayWriteService, GlPostSink, LoggingGatewaySink};
use presentation::http::{create_gateway_webhook_routes, WebhookState};
