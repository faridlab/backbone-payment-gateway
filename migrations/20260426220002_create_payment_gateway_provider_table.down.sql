-- Down: drop payment_gateway.payment_gateway_providers table
DROP TABLE IF EXISTS payment_gateway.payment_gateway_providers CASCADE;
DROP FUNCTION IF EXISTS payment_gateway.payment_gateway_providers_audit_timestamp() CASCADE;
