DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM research.data_snapshots AS snapshot
        LEFT JOIN storage.blobs AS manifest
          ON manifest.tenant_id = snapshot.tenant_id
         AND manifest.content_hash = snapshot.manifest_hash
        WHERE manifest.content_hash IS NULL
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = 'P0001',
            MESSAGE = 'legacy research.data_snapshots rows lack durable manifest blob references; offline export and rebuild required before migration 0008';
    END IF;
END $$;

ALTER TABLE research.data_snapshots
    ADD CONSTRAINT data_snapshots_manifest_blob_fkey
        FOREIGN KEY (tenant_id, manifest_hash)
        REFERENCES storage.blobs (tenant_id, content_hash);

CREATE INDEX data_snapshots_manifest_blob_idx
    ON research.data_snapshots (tenant_id, manifest_hash);
