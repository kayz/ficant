-- R5a: exact immutable DataSource price semantics and typed market-fact provenance.

ALTER TABLE data.sources
    ADD COLUMN price_source_type text;

ALTER TABLE data.sources
    ADD CONSTRAINT sources_price_source_type_check
    CHECK (
        price_source_type IS NULL
        OR price_source_type IN ('REAL_TRADE', 'ACTIVE_QUOTE', 'MODEL_VALUATION')
    );

ALTER TABLE market.quotes
    ADD COLUMN data_source_id core.ulid_text,
    ADD COLUMN data_source_version bigint;

ALTER TABLE market.quotes
    ADD CONSTRAINT quotes_data_source_shape_check
        CHECK ((data_source_id IS NULL) = (data_source_version IS NULL)),
    ADD CONSTRAINT quotes_data_source_fkey
        FOREIGN KEY (tenant_id, data_source_id, data_source_version)
        REFERENCES data.sources (tenant_id, data_source_id, version);

ALTER TABLE market.trades
    ADD COLUMN data_source_id core.ulid_text,
    ADD COLUMN data_source_version bigint;

ALTER TABLE market.trades
    ADD CONSTRAINT trades_data_source_shape_check
        CHECK ((data_source_id IS NULL) = (data_source_version IS NULL)),
    ADD CONSTRAINT trades_data_source_fkey
        FOREIGN KEY (tenant_id, data_source_id, data_source_version)
        REFERENCES data.sources (tenant_id, data_source_id, version);

ALTER TABLE market.valuations
    ADD COLUMN data_source_id core.ulid_text,
    ADD COLUMN data_source_version bigint;

ALTER TABLE market.valuations
    ADD CONSTRAINT valuations_data_source_shape_check
        CHECK ((data_source_id IS NULL) = (data_source_version IS NULL)),
    ADD CONSTRAINT valuations_data_source_fkey
        FOREIGN KEY (tenant_id, data_source_id, data_source_version)
        REFERENCES data.sources (tenant_id, data_source_id, version);
