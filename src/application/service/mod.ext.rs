// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

#[cfg(feature = "codecs")]
pub mod gateway_codecs;
pub mod gateway_events;
pub mod gateway_gl;
#[cfg(feature = "codecs")]
pub mod gateway_ingest_service;
#[cfg(feature = "provider-sdk")]
pub mod gateway_provider;
pub mod gateway_write_service;
#[cfg(feature = "codecs")]
pub use gateway_codecs::{
    CredentialFetch, CredentialReader, DokuCodec, GatewayCodecRegistry, GatewaySecret,
    ManualNoopCodec, MidtransCodec, NormalizedNotification, NotificationCodec, NotificationEvent,
    ParseError, ProviderTruth, RefetchError, StatusRefetch, VerificationScheme, VerifyError,
    VerifyRequest, XenditCodec, PURPOSE_API_READ, PURPOSE_WEBHOOK_VERIFY,
};
pub use gateway_events::{
    GatewayEvent, GatewayEventSink, GatewayTransactionRefunded, GatewayTransactionSettled,
    LoggingGatewaySink,
};
pub use gateway_gl::{AccountingPostEnvelope, GlPostAck, GlPostLine, GlPostRejected, GlPostSink};
#[cfg(feature = "codecs")]
pub use gateway_ingest_service::{IngestError, IngestOutcome, WebhookIngestService, WebhookTarget};
#[cfg(feature = "provider-sdk")]
pub use gateway_provider::{
    ChargeCreated, CreateChargeRequest, GatewayError as ProviderError, GatewayTxStatus,
    ManualGatewayProvider, PaymentGatewayProvider, PaymentGatewayRegistry, RefundResult,
    StubGatewayProvider,
};
pub use gateway_write_service::{GatewayError, GatewayWriteService, SettleOutcome};
