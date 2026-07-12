DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM research.signal_sets) THEN
        RAISE EXCEPTION USING
            ERRCODE = 'P0001',
            MESSAGE = 'unsupported legacy research.signal_sets rows; offline export and rebuild required before migration 0007';
    END IF;
END $$;

ALTER TABLE research.signal_sets
    ADD COLUMN artifact_id core.ulid_text;

ALTER TABLE research.signal_sets
    ALTER COLUMN artifact_id SET NOT NULL,
    DROP CONSTRAINT signal_sets_tenant_id_signal_set_id_fkey,
    ADD CONSTRAINT signal_sets_artifact_fkey
        FOREIGN KEY (tenant_id, artifact_id)
        REFERENCES research.artifacts (tenant_id, artifact_id);

CREATE INDEX signal_sets_artifact_lookup_idx
    ON research.signal_sets (tenant_id, artifact_id);
