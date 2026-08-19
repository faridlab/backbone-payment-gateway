-- Revert the ADR-0014 strict fence re-statement for payment-gateway module.
-- The fence predates this migration (ADR-0008-era), so the honest reverse is to
-- re-state the same live policy, not to disarm the tables: a down that disabled RLS
-- would leave company data unfenced — a posture this module never had.

-- Re-state the pre-existing fence for payment_gateway.gateway_transactions (identical policy; see header).
DROP POLICY IF EXISTS gateway_transactions_company_isolation ON payment_gateway.gateway_transactions;
CREATE POLICY gateway_transactions_company_isolation ON payment_gateway.gateway_transactions
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);

-- Re-state the pre-existing fence for payment_gateway.payment_gateway_providers (identical policy; see header).
DROP POLICY IF EXISTS payment_gateway_providers_company_isolation ON payment_gateway.payment_gateway_providers;
CREATE POLICY payment_gateway_providers_company_isolation ON payment_gateway.payment_gateway_providers
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);

