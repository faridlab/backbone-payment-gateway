-- Down: drop the declared foreign keys for payment-gateway module
ALTER TABLE IF EXISTS payment_gateway.gateway_transactions DROP CONSTRAINT IF EXISTS fk_gateway_transactions_provider_id;
