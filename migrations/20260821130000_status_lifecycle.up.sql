-- Migration: replace the gateway-provider active boolean with a status enum
-- payment_gateway.payment_gateway_providers carried `is_active BOOLEAN NOT NULL DEFAULT TRUE`;
-- the tree-wide convention is one `status` enum field per lifecycle (see docs/refactoring-schema
-- in the serpa workspace). FALSE rows are written to 'inactive'; TRUE rows ride the new column's
-- DEFAULT 'active' (no UPDATE needed). The enum type is created unqualified so it lands beside the
-- module's other enum types (public), where the generated sqlx type_name resolves.

DO $$ BEGIN
    CREATE TYPE provider_status AS ENUM ('active', 'inactive');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

ALTER TABLE payment_gateway.payment_gateway_providers ADD COLUMN status provider_status NOT NULL DEFAULT 'active';
UPDATE payment_gateway.payment_gateway_providers SET status = 'inactive' WHERE NOT is_active;
ALTER TABLE payment_gateway.payment_gateway_providers DROP COLUMN is_active;

DROP INDEX IF EXISTS payment_gateway.idx_payment_gateway_providers_code_is_active;
CREATE INDEX IF NOT EXISTS idx_payment_gateway_providers_code_status ON payment_gateway.payment_gateway_providers (code, status);
