-- Down: drop enum types for payment-gateway module
DROP TYPE IF EXISTS gateway_provider_code CASCADE;
DROP TYPE IF EXISTS gateway_posting_state CASCADE;
DROP TYPE IF EXISTS gateway_transaction_status CASCADE;
DROP TYPE IF EXISTS gateway_party_type CASCADE;
DROP TYPE IF EXISTS gateway_direction CASCADE;
