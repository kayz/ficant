ALTER TABLE market.bonds
    ADD COLUMN coupon_coefficient numeric(28, 0),
    ADD COLUMN coupon_scale integer,
    ADD COLUMN coupon_unit_id core.ulid_text,
    ADD COLUMN coupon_unit_version bigint,
    ADD COLUMN coupon_frequency text,
    ADD COLUMN day_count_convention text,
    ADD COLUMN business_day_convention text,
    ADD CONSTRAINT bonds_pricing_shape_check CHECK (
        (coupon_coefficient IS NULL
         AND coupon_scale IS NULL
         AND coupon_unit_id IS NULL
         AND coupon_unit_version IS NULL
         AND coupon_frequency IS NULL
         AND day_count_convention IS NULL
         AND business_day_convention IS NULL)
        OR
        (coupon_coefficient IS NOT NULL
         AND coupon_scale IS NOT NULL
         AND coupon_unit_id IS NOT NULL
         AND coupon_unit_version IS NOT NULL
         AND coupon_frequency IS NOT NULL
         AND day_count_convention IS NOT NULL
         AND business_day_convention IS NOT NULL
         AND first_issue_date IS NOT NULL
         AND current_issue_date IS NOT NULL
         AND cumulative_issued_coefficient IS NOT NULL
         AND cumulative_issued_scale IS NOT NULL
         AND cumulative_issued_unit_id IS NOT NULL
         AND cumulative_issued_unit_version IS NOT NULL
         AND value_added_tax_status IS NOT NULL
         AND income_tax_status IS NOT NULL
         AND coupon_coefficient >= 0
         AND coupon_scale BETWEEN 0 AND 28
         AND coupon_unit_version > 0
         AND coupon_frequency IN ('annual', 'semiannual')
         AND day_count_convention = 'act_act_bond_isma'
         AND business_day_convention = 'following')
    ),
    ADD CONSTRAINT bonds_coupon_unit_fkey
        FOREIGN KEY (tenant_id, coupon_unit_id, coupon_unit_version)
        REFERENCES market.units (tenant_id, unit_id, version);

ALTER TABLE market.curve_snapshots
    ADD COLUMN visible_at timestamptz,
    ADD COLUMN curve_family_id text,
    ADD CONSTRAINT curve_snapshots_r4d_shape_check CHECK (
        (visible_at IS NULL AND curve_family_id IS NULL)
        OR
        (visible_at IS NOT NULL
         AND visible_at >= as_of
         AND curve_family_id IS NOT NULL
         AND btrim(curve_family_id) = curve_family_id
         AND curve_family_id ~ '^[a-z0-9-]+(\.[a-z0-9-]+){2,}$')
    );
