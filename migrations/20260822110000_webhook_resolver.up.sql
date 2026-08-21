-- Migration: the bare-webhook target resolver (SECURITY DEFINER)
--
-- A provider callback URL carries no company identity and no authenticated
-- session: POST /webhooks/payment-gateway/{provider}/{provider_id}. The ONLY
-- privileged read that route may make is "which provider config does this id
-- point at" — answered by this narrow function. Everything downstream runs
-- inside with_company_scope(company_id) like any scoped request.
--
-- Deliberate properties:
--   * Non-secret projection: company_id, code, credentials_ref (a POINTER into
--     the credential store, never a secret), status. No amounts, no accounts.
--   * EXECUTE is revoked from PUBLIC. The application role gets it via the
--     composition's rls_app_role.sql script (the module does not know app
--     role names).
--   * SECURITY DEFINER + pinned search_path, so the function reads the
--     provider row without a company scope. The function's owner is the
--     migration runner (the bootstrap superuser in every deployment of this
--     stack), which bypasses the table's FORCE RLS; a non-superuser owner
--     would need the resolver query re-examined before this posture holds.
--   * STABLE: a pure read — callable in read-only scopes, safe to plan once.

CREATE OR REPLACE FUNCTION payment_gateway.resolve_webhook_target(p_provider_id uuid)
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
