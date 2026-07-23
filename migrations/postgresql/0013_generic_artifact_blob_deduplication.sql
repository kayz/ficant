ALTER TABLE research.artifacts
    DROP CONSTRAINT artifacts_tenant_id_kind_content_hash_key;

CREATE UNIQUE INDEX artifacts_non_generic_content_unique
    ON research.artifacts (tenant_id, kind, content_hash)
    WHERE kind <> 'GENERIC';
