-- Down: drop payment_gateway.gateway_transactions table
DROP TABLE IF EXISTS payment_gateway.gateway_transactions CASCADE;
DROP FUNCTION IF EXISTS payment_gateway.gateway_transactions_audit_timestamp() CASCADE;
