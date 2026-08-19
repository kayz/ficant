DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM research.artifacts
        WHERE kind NOT IN ('GENERIC', 'SIGNAL_SET')
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = 'P0001',
            MESSAGE = 'unsupported legacy Artifact kind; migrate the authoritative Snapshot or Fact before R6B';
    END IF;
END $$;

ALTER TABLE research.artifacts
    DROP CONSTRAINT artifacts_kind_check,
    ADD CONSTRAINT artifacts_kind_check
        CHECK (kind IN ('GENERIC', 'SIGNAL_SET'));
