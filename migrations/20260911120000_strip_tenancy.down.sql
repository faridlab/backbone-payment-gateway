-- Hand-authored (user-owned). Not regenerated.
--
-- Best-effort restore sketch for the tenancy strip (ADR-0029). This is a breaking module
-- release against dev-stage databases: the down re-adds the company_id column as nullable
-- with the company-leading indexes in their pre-strip shapes, but restores NO data and
-- recreates NO RLS policy: rows written after the strip (or after the decorator re-keyed
-- them) carry org_unit_id only. The composing service's tenancy decorator remains the live
-- fence; the <table>_company_isolation policies are NOT recreated here. The webhook
-- resolver regains its pre-strip projection shape. Treat this down as a schema-shape
-- sketch for archaeology, not a usable rollback.

ALTER TABLE payment_gateway.gateway_transactions        ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE payment_gateway.payment_gateway_providers   ADD COLUMN IF NOT EXISTS company_id uuid;

-- ── gateway_transactions ───────────────────────────────────────────────────────
CREATE INDEX IF NOT EXISTS idx_gateway_transactions_company_id_status
    ON payment_gateway.gateway_transactions (company_id, status);

-- ── payment_gateway_providers ──────────────────────────────────────────────────
CREATE UNIQUE INDEX IF NOT EXISTS idx_payment_gateway_providers_company_id_code
    ON payment_gateway.payment_gateway_providers (company_id, code) WHERE (metadata->>'deleted_at') IS NULL;

-- ── the bare-webhook target resolver (pre-strip projection) ────────────────────
DROP FUNCTION IF EXISTS payment_gateway.resolve_webhook_target(p_provider_id uuid);

CREATE FUNCTION payment_gateway.resolve_webhook_target(p_provider_id uuid)
RETURNS TABLE (company_id uuid, code text, credentials_ref text, status text)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = payment_gateway, public
AS $$
    SELECT p.company_id,
           p.code::text,
           p.credentials_ref,
           p.status::text
      FROM payment_gateway.payment_gateway_providers p
     WHERE p.id = p_provider_id
       AND (p.metadata->>'deleted_at') IS NULL
$$;

REVOKE ALL ON FUNCTION payment_gateway.resolve_webhook_target(uuid) FROM PUBLIC;
