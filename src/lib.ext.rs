//! The module's extension (hand-written; ADR-0031): what the module adds beside its
//! generated services. The generated module and builder carry `ModuleExt` and
//! `ModuleBuilderExt` and dereference to them, so their fields read as `module.field`.

#[allow(unused_imports)]
use super::*;

/// State the module adds; read through the generated module's `Deref`.
pub struct ModuleExt {
    /// The settlement engine (settle + reverse). Exposed so composition can mount
    /// the webhook router or drive settlement directly.
    pub write_service: Arc<GatewayWriteService>,
    /// Composition-provided fee sink. `None` ⇒ `gateway_webhook_router()` returns
    /// `None` (the webhook cannot post the fee companion journal without one).
    pub fee_sink: Option<Arc<dyn GlPostSink>>,
    /// The verified webhook ingest pipeline, built when composition provided both
    /// I/O ports (credential reader + status re-fetcher) and a fee sink. `None`
    /// ⇒ the bare webhook routes cannot be mounted (they would have no way to
    /// verify or re-fetch — mounting them is refused, not degraded).
    #[cfg(feature = "codecs")]
    pub ingest_service: Option<Arc<WebhookIngestService>>,
}

/// State the builder adds.
pub struct ModuleBuilderExt {
    fee_sink: Option<Arc<dyn GlPostSink>>,
    event_sink: Option<Arc<dyn GatewayEventSink>>,
    #[cfg(feature = "codecs")]
    credential_reader: Option<Arc<dyn CredentialReader>>,
    #[cfg(feature = "codecs")]
    refetcher: Option<Arc<dyn StatusRefetch>>,
}

impl Default for ModuleBuilderExt {
    fn default() -> Self {
        Self {
            fee_sink: None,
            event_sink: None,
            #[cfg(feature = "codecs")]
            credential_reader: None,
            #[cfg(feature = "codecs")]
            refetcher: None,
        }
    }
}

impl ModuleBuilderExt {
    /// Build the extension's state from what the generated build made.
    #[allow(unused_variables, clippy::redundant_clone)]
    pub(crate) fn build(self, parts: &ModuleParts<'_>) -> anyhow::Result<ModuleExt> {
        let db_pool = parts.db_pool.clone();
        // The settlement engine: defaults to a logging event sink when composition
        // hasn't wired a durable bus. The fee sink is held separately so the webhook
        // router is built iff composition provided one (gateway_webhook_router -> None otherwise).
        let event_sink: Arc<dyn GatewayEventSink> = self
            .event_sink
            .unwrap_or_else(|| Arc::new(LoggingGatewaySink));
        let write_service = Arc::new(GatewayWriteService::with_sink(db_pool.clone(), event_sink));
        let fee_sink = self.fee_sink;
        // The verified ingest pipeline exists only when composition wired BOTH
        // I/O ports (credential reader + re-fetcher) AND a fee sink — the bare
        // webhook routes are all-or-nothing, never mounted degraded.
        #[cfg(feature = "codecs")]
        let ingest_service = match (self.credential_reader, self.refetcher, fee_sink.clone()) {
            (Some(reader), Some(refetcher), Some(fee_sink)) => {
                Some(Arc::new(WebhookIngestService::new(
                    db_pool.clone(),
                    write_service.clone(),
                    GatewayCodecRegistry::with_builtin(),
                    reader,
                    refetcher,
                    fee_sink,
                )))
            }
            _ => None,
        };
        Ok(ModuleExt {
            write_service,
            fee_sink,
            #[cfg(feature = "codecs")]
            ingest_service,
        })
    }
}

impl crate::ThisModule {
    /// Build the gateway webhook router (`POST /webhook/settle`), if a fee sink
    /// was configured on the builder. Returns `None` when no fee sink was provided
    /// — the webhook cannot post the fee companion journal without one. The
    /// composition layer mounts this behind its own provider-specific signature
    /// verification.
    pub fn gateway_webhook_router(&self) -> Option<Router> {
        let fee_sink = self.fee_sink.clone()?;
        Some(create_gateway_webhook_routes(WebhookState {
            write_service: self.write_service.clone(),
            fee_sink,
        }))
    }
    /// The provider-config READ surface (GET endpoints on
    /// payment_gateway_providers). Compose with
    /// [`Self::provider_config_write_routes`] so a host can gate writes
    /// behind its own role check while reads ride tenant auth alone.
    pub fn provider_config_read_routes(&self) -> Router {
        use presentation::http::create_payment_gateway_provider_read_routes;
        create_payment_gateway_provider_read_routes(self.payment_gateway_provider_service.clone())
    }
    /// The provider-config WRITE surface (unguarded generic mutations on
    /// payment_gateway_providers). A host MUST wrap this in its own
    /// authorization before mounting — the generic CRUD carries no domain
    /// validation.
    pub fn provider_config_write_routes(&self) -> Router {
        use presentation::http::create_payment_gateway_provider_write_routes;
        create_payment_gateway_provider_write_routes(self.payment_gateway_provider_service.clone())
    }
    /// The gateway-transaction READ surface alone. Provider-fed rows are
    /// never operator-writable over HTTP — a host composes this as the
    /// transaction surface, never the generic writes.
    pub fn transaction_read_routes(&self) -> Router {
        use presentation::http::create_gateway_transaction_read_routes;
        create_gateway_transaction_read_routes(self.gateway_transaction_service.clone())
    }
    /// The verified ingest pipeline, when composition wired its ports. The bare
    /// webhook routes are only mountable through this — no pipeline, no route.
    #[cfg(feature = "codecs")]
    pub fn ingest_service(&self) -> Option<Arc<WebhookIngestService>> {
        self.ingest_service.clone()
    }
}

impl crate::ThisModuleBuilder {
    /// Provide the fee-post sink (composition's accounting client). Required for
    /// [`PaymentGatewayModule::gateway_webhook_router`] to return `Some`; without
    /// it the webhook cannot post the fee companion journal.
    pub fn with_fee_sink(mut self, sink: Arc<dyn GlPostSink>) -> Self {
        self.fee_sink = Some(sink);
        self
    }
    /// Provide the settlement-event sink (e.g. an outbox-backed bus). Defaults to
    /// [`LoggingGatewaySink`] (structured tracing) when not set — fine for tests
    /// and single-process setups; composition wires a durable bus for production.
    pub fn with_event_sink(mut self, sink: Arc<dyn GatewayEventSink>) -> Self {
        self.event_sink = Some(sink);
        self
    }
    /// Provide the credential-store read port (composition's adapter over the
    /// credential service). Required, together with [`Self::with_refetcher`]
    /// and a fee sink, for [`PaymentGatewayModule::ingest_service`] to exist.
    #[cfg(feature = "codecs")]
    pub fn with_credential_reader(mut self, reader: Arc<dyn CredentialReader>) -> Self {
        self.credential_reader = Some(reader);
        self
    }
    /// Provide the provider status re-fetch port (composition's API clients).
    /// Required, together with [`Self::with_credential_reader`] and a fee sink,
    /// for [`PaymentGatewayModule::ingest_service`] to exist.
    #[cfg(feature = "codecs")]
    pub fn with_refetcher(mut self, refetcher: Arc<dyn StatusRefetch>) -> Self {
        self.refetcher = Some(refetcher);
        self
    }
}
