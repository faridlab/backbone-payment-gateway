-- Down: restore the gateway-provider active boolean from the status enum
-- Only 'inactive' rows are written back to FALSE; 'active' rows ride the boolean DEFAULT TRUE.

ALTER TABLE payment_gateway.payment_gateway_providers ADD COLUMN is_active BOOLEAN NOT NULL DEFAULT TRUE;
UPDATE payment_gateway.payment_gateway_providers SET is_active = FALSE WHERE status = 'inactive';
ALTER TABLE payment_gateway.payment_gateway_providers DROP COLUMN status;

DROP INDEX IF EXISTS idx_payment_gateway_providers_code_status;
CREATE INDEX IF NOT EXISTS idx_payment_gateway_providers_code_is_active ON payment_gateway.payment_gateway_providers (code, is_active);

DROP TYPE IF EXISTS provider_status;
