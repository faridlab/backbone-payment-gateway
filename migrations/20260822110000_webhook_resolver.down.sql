-- Migration: drop the bare-webhook target resolver
DROP FUNCTION IF EXISTS payment_gateway.resolve_webhook_target(uuid);
