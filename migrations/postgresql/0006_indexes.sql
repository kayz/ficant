CREATE INDEX definition_identity_latest_idx
    ON core.definition_identities (tenant_id, definition_id, latest_version);
CREATE INDEX calendar_as_of_idx
    ON market.calendars (tenant_id, calendar_id, effective_from, effective_to);
CREATE INDEX rule_pack_as_of_idx
    ON market.market_rule_packs (tenant_id, rule_pack_id, effective_from, effective_to);
CREATE INDEX instrument_lookup_idx
    ON market.instruments (tenant_id, instrument_id, version);
CREATE INDEX cashflow_window_idx
    ON market.cashflows (tenant_id, instrument_id, instrument_version, fact_time, cashflow_id);
CREATE INDEX quote_window_idx
    ON market.quotes (tenant_id, instrument_id, instrument_version, fact_time, quote_id);
CREATE INDEX trade_window_idx
    ON market.trades (tenant_id, instrument_id, instrument_version, fact_time, trade_id);
CREATE INDEX valuation_window_idx
    ON market.valuations (tenant_id, instrument_id, instrument_version, fact_time, valuation_id);
CREATE INDEX lineage_reverse_idx
    ON research.lineage_edges (tenant_id, target_object_id, target_version, target_content_hash);
CREATE INDEX artifact_hash_idx
    ON research.artifacts (tenant_id, content_hash);
CREATE INDEX journal_event_time_idx
    ON research.run_journal (tenant_id, run_id, occurred_at, sequence);
