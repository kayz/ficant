CREATE TABLE research.factor_definitions (
    factor_id text PRIMARY KEY,
    factor_unit_id core.ulid_text NOT NULL,
    factor_unit_version bigint NOT NULL CHECK (factor_unit_version > 0),
    bump_coefficient numeric(28, 0) NOT NULL CHECK (bump_coefficient > 0),
    bump_scale integer NOT NULL CHECK (bump_scale BETWEEN 0 AND 28),
    bump_unit_id core.ulid_text NOT NULL,
    bump_unit_version bigint NOT NULL CHECK (bump_unit_version > 0),
    direction text NOT NULL CHECK (direction IN ('CENTRAL', 'UP', 'DOWN')),
    curve_rebuild text NOT NULL CHECK (curve_rebuild IN ('REBUILD', 'HOLD')),
    second_order text NOT NULL CHECK (second_order IN ('INCLUDE', 'EXCLUDE')),
    content_hash core.sha256_hex NOT NULL UNIQUE,
    CHECK (factor_id ~ '^[a-z0-9-]+(\.[a-z0-9-]+){3}$')
);

CREATE TABLE research.curve_node_definitions (
    curve_node_id text PRIMARY KEY,
    curve_family_id text NOT NULL,
    tenor text NOT NULL,
    factor_unit_id core.ulid_text NOT NULL,
    factor_unit_version bigint NOT NULL CHECK (factor_unit_version > 0),
    content_hash core.sha256_hex NOT NULL UNIQUE,
    CHECK (curve_node_id ~ '^[a-z0-9-]+(\.[a-z0-9-]+){2,}$'),
    CHECK (curve_family_id ~ '^[a-z0-9-]+(\.[a-z0-9-]+){2,}$'),
    CHECK (tenor ~ '^P[0-9]+[YMD]$')
);

CREATE TABLE research.factor_target_bindings (
    factor_id text NOT NULL REFERENCES research.factor_definitions (factor_id),
    target_kind text NOT NULL CHECK (target_kind IN ('INSTRUMENT', 'CURVE_NODE')),
    target_tenant_id core.ulid_text NULL,
    target_owner_id core.ulid_text NULL,
    target_instrument_id core.ulid_text NULL,
    target_instrument_version bigint NULL CHECK (target_instrument_version > 0),
    target_curve_node_id text NULL REFERENCES research.curve_node_definitions (curve_node_id),
    target_curve_node_hash core.sha256_hex NULL,
    content_hash core.sha256_hex NOT NULL UNIQUE,
    CHECK (
        (target_kind = 'INSTRUMENT'
            AND target_tenant_id IS NOT NULL
            AND target_owner_id IS NOT NULL
            AND target_instrument_id IS NOT NULL
            AND target_instrument_version IS NOT NULL
            AND target_curve_node_id IS NULL
            AND target_curve_node_hash IS NULL)
        OR
        (target_kind = 'CURVE_NODE'
            AND target_tenant_id IS NULL
            AND target_owner_id IS NULL
            AND target_instrument_id IS NULL
            AND target_instrument_version IS NULL
            AND target_curve_node_id IS NOT NULL
            AND target_curve_node_hash IS NOT NULL)
    ),
    UNIQUE NULLS NOT DISTINCT (
        factor_id,
        target_kind,
        target_tenant_id,
        target_owner_id,
        target_instrument_id,
        target_instrument_version,
        target_curve_node_id,
        target_curve_node_hash
    )
);

CREATE INDEX factor_target_bindings_target_idx
    ON research.factor_target_bindings (
        target_kind,
        target_tenant_id,
        target_owner_id,
        target_instrument_id,
        target_instrument_version,
        target_curve_node_id
    );
