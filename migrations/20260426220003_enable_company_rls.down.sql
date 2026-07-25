-- Down: remove the company RLS fence for payment-gateway module

-- Reverse the company RLS fence for payment_gateway.gateway_transactions
DROP POLICY IF EXISTS gateway_transactions_company_isolation ON payment_gateway.gateway_transactions;
ALTER TABLE payment_gateway.gateway_transactions NO FORCE ROW LEVEL SECURITY;
ALTER TABLE payment_gateway.gateway_transactions DISABLE ROW LEVEL SECURITY;

-- Reverse the company RLS fence for payment_gateway.payment_gateway_providers
DROP POLICY IF EXISTS payment_gateway_providers_company_isolation ON payment_gateway.payment_gateway_providers;
ALTER TABLE payment_gateway.payment_gateway_providers NO FORCE ROW LEVEL SECURITY;
ALTER TABLE payment_gateway.payment_gateway_providers DISABLE ROW LEVEL SECURITY;

