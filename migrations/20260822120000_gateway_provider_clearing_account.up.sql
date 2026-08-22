-- Provider configs gain an interim clearing account: a provider notification
-- settles money without naming the paying customer, and the GL fence demands a
-- party on every accounts-receivable control line — so a party-less receipt
-- credits this account until the operator allocates it and the A/R leg lands
-- with the party resolved.
ALTER TABLE payment_gateway.payment_gateway_providers
    ADD COLUMN IF NOT EXISTS clearing_account_id UUID;
