ALTER TABLE market.bonds
    ADD COLUMN first_issue_date date,
    ADD COLUMN current_issue_date date,
    ADD COLUMN cumulative_issued_coefficient numeric(28, 0),
    ADD COLUMN cumulative_issued_scale integer,
    ADD COLUMN cumulative_issued_unit_id core.ulid_text,
    ADD COLUMN cumulative_issued_unit_version bigint,
    ADD COLUMN value_added_tax_status text,
    ADD COLUMN income_tax_status text,
    ADD CONSTRAINT bonds_issuance_shape_check CHECK (
        (first_issue_date IS NULL
         AND current_issue_date IS NULL
         AND cumulative_issued_coefficient IS NULL
         AND cumulative_issued_scale IS NULL
         AND cumulative_issued_unit_id IS NULL
         AND cumulative_issued_unit_version IS NULL
         AND value_added_tax_status IS NULL
         AND income_tax_status IS NULL)
        OR
        (first_issue_date IS NOT NULL
         AND current_issue_date IS NOT NULL
         AND cumulative_issued_coefficient IS NOT NULL
         AND cumulative_issued_scale IS NOT NULL
         AND cumulative_issued_unit_id IS NOT NULL
         AND cumulative_issued_unit_version IS NOT NULL
         AND value_added_tax_status IS NOT NULL
         AND income_tax_status IS NOT NULL
         AND first_issue_date < maturity_date
         AND current_issue_date >= first_issue_date
         AND current_issue_date < maturity_date
         AND cumulative_issued_coefficient > 0
         AND cumulative_issued_scale BETWEEN 0 AND 28
         AND cumulative_issued_unit_version > 0
         AND value_added_tax_status IN ('exempt', 'taxable')
         AND income_tax_status IN ('exempt', 'taxable'))
    ),
    ADD CONSTRAINT bonds_cumulative_issued_unit_fkey
        FOREIGN KEY (tenant_id, cumulative_issued_unit_id, cumulative_issued_unit_version)
        REFERENCES market.units (tenant_id, unit_id, version);
