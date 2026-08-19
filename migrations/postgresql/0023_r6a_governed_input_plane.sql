-- Pre-R6A Subject rows have no tenant/owner evidence and cannot be safely inferred. Refuse to
-- invent an authority during migration; an operator must export/re-import those rows with an
-- explicit OwnerRef before applying R6A.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM core.subject_versions LIMIT 1)
       OR EXISTS (SELECT 1 FROM core.subject_state_snapshots LIMIT 1) THEN
        RAISE EXCEPTION
            'R6A requires explicit tenant/owner migration for existing Subject/SubjectState rows';
    END IF;
END
$$;

CREATE TABLE core.subject_identities (
    tenant_id core.ulid_text NOT NULL,
    subject_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    latest_version bigint NOT NULL DEFAULT 0 CHECK (latest_version >= 0),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, subject_id),
    UNIQUE (tenant_id, subject_id, owner_id)
);

ALTER TABLE core.subject_versions
    ADD COLUMN tenant_id core.ulid_text NOT NULL,
    ADD COLUMN owner_id core.ulid_text NOT NULL,
    ADD CONSTRAINT subject_versions_scoped_version_unique
        UNIQUE (tenant_id, subject_id, version, owner_id),
    ADD CONSTRAINT subject_versions_owner_identity_fk
        FOREIGN KEY (tenant_id, subject_id, owner_id)
        REFERENCES core.subject_identities (tenant_id, subject_id, owner_id);

ALTER TABLE core.subject_state_snapshots
    DROP CONSTRAINT subject_state_snapshots_subject_id_subject_version_fkey,
    ADD COLUMN tenant_id core.ulid_text NOT NULL,
    ADD COLUMN owner_id core.ulid_text NOT NULL,
    ADD CONSTRAINT subject_state_scoped_subject_fk
        FOREIGN KEY (tenant_id, subject_id, subject_version, owner_id)
        REFERENCES core.subject_versions (tenant_id, subject_id, version, owner_id);

CREATE INDEX subject_versions_owner_lookup_idx ON core.subject_versions
    (tenant_id, owner_id, subject_id, version);
CREATE INDEX subject_state_owner_lookup_idx ON core.subject_state_snapshots
    (tenant_id, owner_id, snapshot_id, visible_at);

CREATE TABLE data.source_authorization_identities (
    tenant_id core.ulid_text NOT NULL,
    authorization_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    latest_version bigint NOT NULL DEFAULT 0 CHECK (latest_version >= 0),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, authorization_id)
);

CREATE TABLE data.source_authorizations (
    tenant_id core.ulid_text NOT NULL,
    authorization_id core.ulid_text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    owner_id core.ulid_text NOT NULL,
    data_source_id core.ulid_text NOT NULL,
    data_source_version bigint NOT NULL CHECK (data_source_version > 0),
    data_source_hash core.sha256_hex NOT NULL,
    import_interface text NOT NULL CHECK (import_interface IN ('CANONICAL_QUOTE_SNAPSHOT')),
    canonical_schema_id text NOT NULL CHECK (canonical_schema_id ~ '^ficant\.[A-Za-z0-9_.-]+\.v1$'),
    canonical_schema_hash core.sha256_hex NOT NULL,
    effective_from timestamptz NOT NULL,
    effective_from_timezone text NOT NULL CHECK (btrim(effective_from_timezone) = effective_from_timezone AND effective_from_timezone <> ''),
    effective_from_local_date date NOT NULL,
    effective_to timestamptz NOT NULL,
    effective_to_timezone text NOT NULL CHECK (btrim(effective_to_timezone) = effective_to_timezone AND effective_to_timezone <> ''),
    effective_to_local_date date NOT NULL,
    state text NOT NULL CHECK (state IN ('ACTIVE', 'REVOKED')),
    supersedes_id core.ulid_text,
    supersedes_version bigint CHECK (supersedes_version > 0),
    mapping_id core.ulid_text NOT NULL,
    mapping_hash core.sha256_hex NOT NULL,
    content_hash core.sha256_hex NOT NULL,
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, authorization_id, version),
    UNIQUE (tenant_id, content_hash),
    FOREIGN KEY (tenant_id, authorization_id)
        REFERENCES data.source_authorization_identities (tenant_id, authorization_id),
    FOREIGN KEY (tenant_id, data_source_id, data_source_version)
        REFERENCES data.sources (tenant_id, data_source_id, version),
    FOREIGN KEY (tenant_id, supersedes_id, supersedes_version)
        REFERENCES data.source_authorizations (tenant_id, authorization_id, version),
    CHECK (effective_from < effective_to),
    CHECK ((supersedes_id IS NULL) = (supersedes_version IS NULL)),
    CHECK (
        (version = 1 AND supersedes_id IS NULL AND state = 'ACTIVE')
        OR (version > 1 AND supersedes_id = authorization_id AND supersedes_version = version - 1)
    )
);

CREATE INDEX source_authorizations_lookup_idx ON data.source_authorizations
    (tenant_id, owner_id, data_source_id, data_source_version, import_interface, version);

CREATE TABLE core.foundation_change_records (
    tenant_id core.ulid_text NOT NULL,
    record_id core.ulid_text NOT NULL,
    actor_id core.ulid_text NOT NULL,
    owner_id core.ulid_text NOT NULL,
    active_role text NOT NULL CHECK (active_role IN ('PLATFORM_ADMIN', 'RESEARCHER')),
    operation text NOT NULL CHECK (operation IN (
        'data-source.register', 'data-source-authorization.publish',
        'market-definition.append', 'market-fact.append', 'market-fact.correct',
        'curve-snapshot.publish', 'data-snapshot.import-canonical-quotes',
        'universe-snapshot.publish', 'subject.register', 'subject-state.publish',
        'position-snapshot.publish', 'data-health-threshold.configure'
    )),
    resource_kind text NOT NULL,
    resource_id core.ulid_text NOT NULL,
    resource_version bigint CHECK (resource_version > 0),
    resource_ref text NOT NULL CHECK (btrim(resource_ref) = resource_ref AND resource_ref <> ''),
    before_hash core.sha256_hex,
    after_hash core.sha256_hex NOT NULL,
    reason text NOT NULL CHECK (btrim(reason) = reason AND reason <> ''),
    request_fingerprint core.sha256_hex NOT NULL,
    occurred_at timestamptz NOT NULL,
    occurred_timezone text NOT NULL CHECK (btrim(occurred_timezone) = occurred_timezone AND occurred_timezone <> ''),
    occurred_local_date date NOT NULL,
    authorization_id core.ulid_text,
    authorization_version bigint CHECK (authorization_version > 0),
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, record_id),
    UNIQUE (tenant_id, request_fingerprint, operation, resource_ref),
    FOREIGN KEY (tenant_id, authorization_id, authorization_version)
        REFERENCES data.source_authorizations (tenant_id, authorization_id, version),
    CHECK ((authorization_id IS NULL) = (authorization_version IS NULL)),
    CHECK (
        (operation = 'data-snapshot.import-canonical-quotes'
         AND active_role = 'RESEARCHER' AND authorization_id IS NOT NULL)
        OR
        (operation <> 'data-snapshot.import-canonical-quotes'
         AND active_role = 'PLATFORM_ADMIN' AND authorization_id IS NULL)
    )
);

CREATE TABLE core.foundation_change_sources (
    tenant_id core.ulid_text NOT NULL,
    record_id core.ulid_text NOT NULL,
    source_ordinal integer NOT NULL CHECK (source_ordinal >= 0),
    uri text NOT NULL CHECK (btrim(uri) = uri AND uri <> ''),
    sha256 core.sha256_hex NOT NULL,
    PRIMARY KEY (tenant_id, record_id, source_ordinal),
    FOREIGN KEY (tenant_id, record_id)
        REFERENCES core.foundation_change_records (tenant_id, record_id)
);

CREATE INDEX foundation_change_query_idx ON core.foundation_change_records
    (tenant_id, occurred_at, record_id);
CREATE INDEX foundation_change_resource_idx ON core.foundation_change_records
    (tenant_id, resource_ref, occurred_at);
CREATE INDEX foundation_change_actor_idx ON core.foundation_change_records
    (tenant_id, actor_id, occurred_at);
