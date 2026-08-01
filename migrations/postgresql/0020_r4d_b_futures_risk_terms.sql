ALTER TABLE market.futures_contracts
    ADD COLUMN product_code text,
    ADD COLUMN price_unit_id core.ulid_text,
    ADD COLUMN price_unit_version bigint,
    ADD CONSTRAINT futures_contracts_risk_terms_shape_check CHECK (
        (product_code IS NULL
         AND price_unit_id IS NULL
         AND price_unit_version IS NULL)
        OR
        (product_code IS NOT NULL
         AND btrim(product_code) = product_code
         AND product_code ~ '^[A-Z][A-Z0-9-]*$'
         AND price_unit_id IS NOT NULL
         AND price_unit_version IS NOT NULL
         AND price_unit_version > 0)
    ),
    ADD CONSTRAINT futures_contracts_price_unit_fkey
        FOREIGN KEY (tenant_id, price_unit_id, price_unit_version)
        REFERENCES market.units (tenant_id, unit_id, version);
