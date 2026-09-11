-- Hand-authored (user-owned). Not regenerated.
--
-- Strip every company-fence artifact from the payment_gateway tables (ADR-0029): the
-- module is tenant-agnostic; org scoping is installed by the COMPOSING service's
-- tenancy decorator, never by the module. Dropped here, per table: the
-- company-leading indexes, the <table>_company_isolation RLS policy, and the
-- company_id column itself.
--
-- Ordering guard (the decorator must run FIRST on any database with data): the module
-- never moves tenancy data. A table is safe to strip when EITHER
--   a) it carries org_unit_id with no NULLs — the decorator backfilled it from company_id —
--      or b) it is empty (a fresh database: the earlier chain files created it empty).
-- Otherwise the strip RAISEs, naming the decorator step, rather than dropping a column
-- that still holds the only tenancy key. The file is re-runnable (every drop is IF EXISTS
-- and the tracker has no checksums), so a failed run retries cleanly after the decorator
-- lands.
--
-- RLS enable/force flags are deliberately NOT touched: the decorator owns those now.
--
-- The bare-webhook target resolver (payment_gateway.resolve_webhook_target) projected
-- the provider row's company_id so the ingest pipeline could wrap the settle in a
-- company scope. With the column gone the projection loses that field — the resolver
-- keeps its one privileged job (which provider config does this id point at) and the
-- ingest pipeline runs on whatever scope the composing service binds. The return-type
-- change requires DROP + CREATE, not CREATE OR REPLACE; the EXECUTE grant posture is
-- re-stated (revoked from PUBLIC, granted to the app role by the composition's
-- grant script, which the module does not know).

DO $$
DECLARE
    t text;
    has_org boolean;
    org_nulls bigint;
    total bigint;
    offenders text := '';
BEGIN
    FOREACH t IN ARRAY ARRAY['gateway_transactions', 'payment_gateway_providers']
    LOOP
        IF to_regclass(format('payment_gateway.%I', t)) IS NULL THEN
            CONTINUE; -- chain not fully applied on this database; nothing to strip
        END IF;

        SELECT EXISTS (
                   SELECT 1 FROM information_schema.columns
                   WHERE table_schema = 'payment_gateway' AND table_name = t AND column_name = 'org_unit_id'
               )
        INTO has_org;

        EXECUTE format('SELECT count(*) FROM payment_gateway.%I', t) INTO total;

        IF has_org THEN
            EXECUTE format(
                'SELECT count(*) FROM payment_gateway.%I WHERE org_unit_id IS NULL', t)
            INTO org_nulls;
        ELSE
            org_nulls := total; -- no org column: every row's only tenancy key is company_id
        END IF;

        IF has_org AND org_nulls = 0 THEN
            CONTINUE; -- decorator backfilled: safe
        END IF;
        IF total = 0 THEN
            CONTINUE; -- empty table (fresh database): safe
        END IF;
        offenders := offenders || format(' payment_gateway.%s (%s rows, %s rows not covered by org_unit_id);', t, total, org_nulls);
    END LOOP;

    IF offenders <> '' THEN
        RAISE EXCEPTION 'refusing to strip company_id — these tables are not yet covered by the tenancy decorator:%. Apply the composing service''s tenancy decorator (it backfills org_unit_id from company_id) and re-run; it is the only step that moves tenancy data.', offenders;
    END IF;
END $$;

-- ── gateway_transactions ───────────────────────────────────────────────────────
DROP INDEX IF EXISTS payment_gateway.idx_gateway_transactions_company_id_status;
DROP POLICY IF EXISTS gateway_transactions_company_isolation ON payment_gateway.gateway_transactions;
ALTER TABLE payment_gateway.gateway_transactions DROP COLUMN IF EXISTS company_id;

-- ── payment_gateway_providers ──────────────────────────────────────────────────
-- The (company_id, code) unique is NOT re-based tenant-free: the provider code is a
-- per-unit grain (two org units may each configure the same gateway), so a module-level
-- (code)-only unique would wrongly forbid that. The per-unit form is re-declared
-- org-scoped by the composing service's tenancy decorator.
DROP INDEX IF EXISTS payment_gateway.idx_payment_gateway_providers_company_id_code;
DROP POLICY IF EXISTS payment_gateway_providers_company_isolation ON payment_gateway.payment_gateway_providers;
ALTER TABLE payment_gateway.payment_gateway_providers DROP COLUMN IF EXISTS company_id;

-- ── the bare-webhook target resolver ───────────────────────────────────────────
-- Same deliberate properties as the chain file that created it: non-secret projection
-- (code, credentials_ref pointer, status), EXECUTE revoked from PUBLIC, SECURITY
-- DEFINER with a pinned search_path so the read needs no request scope, STABLE.
DROP FUNCTION IF EXISTS payment_gateway.resolve_webhook_target(p_provider_id uuid);

CREATE FUNCTION payment_gateway.resolve_webhook_target(p_provider_id uuid)
RETURNS TABLE (code text, credentials_ref text, status text)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = payment_gateway, public
AS $$
    SELECT p.code::text,
           p.credentials_ref,
           p.status::text
      FROM payment_gateway.payment_gateway_providers p
     WHERE p.id = p_provider_id
       AND (p.metadata->>'deleted_at') IS NULL
$$;

REVOKE ALL ON FUNCTION payment_gateway.resolve_webhook_target(uuid) FROM PUBLIC;
