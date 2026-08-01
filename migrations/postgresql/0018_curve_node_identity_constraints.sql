ALTER TABLE research.curve_node_definitions
    DROP CONSTRAINT curve_node_definitions_tenor_check,
    ADD CONSTRAINT curve_node_definitions_tenor_canonical_check
        CHECK (tenor ~ '^P[1-9][0-9]*[YMD]$'),
    ADD CONSTRAINT curve_node_definitions_family_tenor_key
        UNIQUE (curve_family_id, tenor);
