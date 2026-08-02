use async_trait::async_trait;
use ficant_application::ApplicationError;
use ficant_application::ApplicationErrorCategory;
use ficant_application::ports::{
    AccessScope, AppendMarketFact, CorrectMarketFact, Cursor, CursorPage, CurveSnapshotMetadata,
    CurveSnapshotMetadataRepository, DefinitionValue, MarketFact, MarketFactKind,
    MarketFactRepository, MarketFactRuleProof, MarketFactRuleProofKind, MarketFactWindow,
    PublishCurveSnapshot, ResolvedMarketFactProof,
};
use ficant_domain::market::CurveSnapshot;
use ficant_domain::primitives::Ulid;
use ficant_domain::{ContentAddressed, Lineaged, VersionedDefinition};
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{Postgres, Transaction};

use super::PostgresRepository;
use super::codec::{
    decode_curve_snapshot, decode_definition, decode_fact, encode_curve_snapshot, encode_fact,
};
use super::common::{
    IdempotencyOutcome, application_error, insert_lineage, lock_idempotency, map_sqlx_error,
    publish_blob_reference,
};

#[async_trait]
impl MarketFactRepository for PostgresRepository {
    async fn append_fact(&self, command: AppendMarketFact) -> Result<MarketFact, ApplicationError> {
        self.append_market_fact(command).await
    }

    async fn append_correction(
        &self,
        command: CorrectMarketFact,
    ) -> Result<MarketFact, ApplicationError> {
        self.correct_market_fact(command).await
    }

    async fn query_instrument_window(
        &self,
        scope: &AccessScope,
        query: MarketFactWindow,
    ) -> Result<CursorPage<MarketFact>, ApplicationError> {
        query.authorize_scope(scope)?;
        let owners = owner_strings(scope);
        let cursor = query
            .page()
            .cursor()
            .map(|value| parse_fact_cursor(value.opaque_value()))
            .transpose()?;
        let (has_cursor, cursor_time, cursor_kind, cursor_id) = cursor.map_or(
            (
                false,
                DateTime::<Utc>::UNIX_EPOCH,
                String::new(),
                String::new(),
            ),
            |(time, kind, id)| (true, time, kind, id),
        );
        let limit = i64::from(query.page().limit()) + 1;
        let rows: Vec<(DateTime<Utc>, String, String, Vec<u8>)> = sqlx::query_as(
            "WITH facts AS (
                SELECT fact_time, '1'::text AS kind, cashflow_id::text AS fact_id, payload
                FROM market.cashflows
                WHERE tenant_id = $1 AND instrument_id = $2 AND instrument_version = $3
                  AND fact_time BETWEEN $4 AND $5 AND owner_id::text = ANY($6::text[])
                UNION ALL
                SELECT fact_time, '2', quote_id::text, payload FROM market.quotes
                WHERE tenant_id = $1 AND instrument_id = $2 AND instrument_version = $3
                  AND fact_time BETWEEN $4 AND $5 AND owner_id::text = ANY($6::text[])
                UNION ALL
                SELECT fact_time, '3', trade_id::text, payload FROM market.trades
                WHERE tenant_id = $1 AND instrument_id = $2 AND instrument_version = $3
                  AND fact_time BETWEEN $4 AND $5 AND owner_id::text = ANY($6::text[])
                UNION ALL
                SELECT fact_time, '4', valuation_id::text, payload FROM market.valuations
                WHERE tenant_id = $1 AND instrument_id = $2 AND instrument_version = $3
                  AND fact_time BETWEEN $4 AND $5 AND owner_id::text = ANY($6::text[])
             )
             SELECT fact_time, kind, fact_id, payload FROM facts
             WHERE NOT $7 OR (fact_time, kind, fact_id) > ($8, $9, $10)
             ORDER BY fact_time, kind, fact_id
             LIMIT $11",
        )
        .bind(scope.tenant_id().as_str())
        .bind(query.instrument().id().as_str())
        .bind(version_i64(query.instrument().version().get())?)
        .bind(query.from().instant())
        .bind(query.to().instant())
        .bind(&owners)
        .bind(has_cursor)
        .bind(cursor_time)
        .bind(&cursor_kind)
        .bind(&cursor_id)
        .bind(limit)
        .fetch_all(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        let page_len = usize::try_from(query.page().limit()).map_err(|_| invalid())?;
        let has_more = rows.len() > page_len;
        let page_rows = rows.into_iter().take(page_len).collect::<Vec<_>>();
        let next_cursor = if has_more {
            let (time, kind, id, _) = page_rows.last().ok_or_else(storage_error)?;
            Some(Cursor::issue(
                self.cursor_codec(),
                scope,
                format!("{}.{}.{}", time.timestamp_micros(), kind, id),
            )?)
        } else {
            None
        };
        let items = page_rows
            .into_iter()
            .map(|(_, _, _, payload)| decode_fact(&payload))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CursorPage::new(items, next_cursor))
    }

    async fn publish_curve_snapshot(
        &self,
        command: PublishCurveSnapshot,
    ) -> Result<CurveSnapshot, ApplicationError> {
        let curve = command.curve();
        command.scope().authorize(curve.owner())?;
        let tenant = curve.owner().tenant_id().as_str();
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let outcome = lock_idempotency(
            &mut transaction,
            tenant,
            "curve-snapshot:publish:v1",
            command.idempotency_key().as_str(),
            command.fingerprint().content_hash().as_bytes(),
            curve.id().as_str(),
        )
        .await?;
        if outcome == IdempotencyOutcome::Replay {
            transaction.commit().await.map_err(map_sqlx_error)?;
            return Ok(curve.clone());
        }
        let existing: Option<(Vec<u8>,)> = sqlx::query_as(
            "SELECT fingerprint FROM market.curve_snapshots
             WHERE tenant_id = $1 AND curve_snapshot_id = $2
             FOR UPDATE",
        )
        .bind(tenant)
        .bind(curve.id().as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        if existing.is_some() {
            return Err(application_error(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            ));
        }
        publish_blob_reference(
            &mut transaction,
            tenant,
            curve.content_hash(),
            command.declared_blob_size(),
        )
        .await?;
        sqlx::query(
            "INSERT INTO market.curve_snapshots
             (tenant_id, curve_snapshot_id, owner_id, as_of,
              currency_unit_id, currency_unit_version, curve_kind,
              calendar_id, calendar_version, rule_pack_id, rule_pack_version,
              point_schema, content_hash, blob_size, idempotency_key, fingerprint, payload,
              visible_at, curve_family_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                     $15, $16, $17, $18, $19)",
        )
        .bind(tenant)
        .bind(curve.id().as_str())
        .bind(curve.owner().owner_id().as_str())
        .bind(curve.as_of().instant())
        .bind(curve.currency().unit_id().as_str())
        .bind(version_i64(curve.currency().version().get())?)
        .bind(curve.curve_kind())
        .bind(curve.calendar().id().as_str())
        .bind(version_i64(curve.calendar().version().get())?)
        .bind(curve.rule_pack().id().as_str())
        .bind(version_i64(curve.rule_pack().version().get())?)
        .bind(curve.point_schema())
        .bind(crate::s3::content_addressed::hash_hex(curve.content_hash()))
        .bind(version_i64(command.declared_blob_size())?)
        .bind(command.idempotency_key().as_str())
        .bind(command.fingerprint().content_hash().as_bytes().as_slice())
        .bind(encode_curve_snapshot(curve))
        .bind(
            curve
                .visible_at()
                .map(ficant_domain::primitives::MarketTime::instant),
        )
        .bind(curve.curve_family_id())
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        insert_lineage(
            &mut transaction,
            tenant,
            curve.id().as_str(),
            curve.lineage(),
        )
        .await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(curve.clone())
    }

    async fn get_curve_snapshot(
        &self,
        scope: &AccessScope,
        curve_snapshot_id: Ulid,
    ) -> Result<Option<CurveSnapshot>, ApplicationError> {
        let owners = owner_strings(scope);
        let payload: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT payload FROM market.curve_snapshots
             WHERE tenant_id = $1 AND curve_snapshot_id = $2
               AND owner_id::text = ANY($3::text[])",
        )
        .bind(scope.tenant_id().as_str())
        .bind(curve_snapshot_id.as_str())
        .bind(&owners)
        .fetch_optional(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        payload
            .map(|bytes| decode_curve_snapshot(&bytes))
            .transpose()
    }
}

#[async_trait]
impl CurveSnapshotMetadataRepository for PostgresRepository {
    async fn get_curve_snapshot_metadata(
        &self,
        scope: &AccessScope,
        curve_snapshot_id: Ulid,
    ) -> Result<Option<CurveSnapshotMetadata>, ApplicationError> {
        let owners = owner_strings(scope);
        let row: Option<(Vec<u8>, i64)> = sqlx::query_as(
            "SELECT payload, blob_size FROM market.curve_snapshots
             WHERE tenant_id = $1 AND curve_snapshot_id = $2
               AND owner_id::text = ANY($3::text[])",
        )
        .bind(scope.tenant_id().as_str())
        .bind(curve_snapshot_id.as_str())
        .bind(&owners)
        .fetch_optional(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        row.map(|(payload, size)| {
            let size = u64::try_from(size).map_err(|_| {
                application_error(ApplicationErrorCategory::ValidationFailed, false)
            })?;
            CurveSnapshotMetadata::new(decode_curve_snapshot(&payload)?, size)
        })
        .transpose()
    }
}

impl PostgresRepository {
    /// Appends one immutable market fact.
    ///
    /// # Errors
    ///
    /// Returns a classified application error on idempotency, lineage, or storage conflict.
    pub async fn append_market_fact(
        &self,
        command: AppendMarketFact,
    ) -> Result<MarketFact, ApplicationError> {
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        validate_market_fact_units(&mut transaction, command.fact(), command.proof()).await?;
        validate_market_fact_rule(&mut transaction, command.fact(), command.rule_proof()).await?;
        let value = persist_market_fact(
            &mut transaction,
            command.fact(),
            command.idempotency_key().as_str(),
            command.fingerprint().content_hash().as_bytes(),
        )
        .await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value)
    }

    /// Appends an immutable correction linked to the original fact.
    ///
    /// # Errors
    ///
    /// Returns a classified application error when the original lineage is unavailable.
    pub async fn correct_market_fact(
        &self,
        command: CorrectMarketFact,
    ) -> Result<MarketFact, ApplicationError> {
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        validate_market_fact_units(&mut transaction, command.correction(), command.proof()).await?;
        validate_market_fact_rule(&mut transaction, command.correction(), command.rule_proof())
            .await?;
        let value = persist_market_fact(
            &mut transaction,
            command.correction(),
            command.idempotency_key().as_str(),
            command.fingerprint().content_hash().as_bytes(),
        )
        .await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value)
    }
}

pub(crate) async fn validate_market_fact_rule(
    transaction: &mut Transaction<'_, Postgres>,
    fact: &MarketFact,
    proof: &MarketFactRuleProof,
) -> Result<(), ApplicationError> {
    if proof.kind() == MarketFactRuleProofKind::NoRule
        && matches!(
            fact,
            MarketFact::Cashflow(_) | MarketFact::Quote(_) | MarketFact::Trade(_)
        )
    {
        return Ok(());
    }
    let Some(binding) = proof.valuation() else {
        return Err(lineage_error());
    };
    let MarketFact::Valuation(valuation) = fact else {
        return Err(lineage_error());
    };
    if binding.tenant_id() != valuation.owner().tenant_id()
        || binding.fact_id() != valuation.id()
        || binding.rule_pack() != valuation.rule_pack()
        || binding.subject() != valuation.valuation_at()
    {
        return Err(lineage_error());
    }
    let row: Option<(DateTime<Utc>, DateTime<Utc>, Vec<u8>)> = sqlx::query_as(
        "SELECT effective_from, effective_to, payload FROM market.market_rule_packs
         WHERE tenant_id=$1 AND rule_pack_id=$2 AND version=$3 FOR SHARE",
    )
    .bind(binding.tenant_id().as_str())
    .bind(binding.rule_pack().id().as_str())
    .bind(version_i64(binding.rule_pack().version().get())?)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let Some((from, to, payload)) = row else {
        return Err(lineage_error());
    };
    let DefinitionValue::MarketRulePack(rule) = decode_definition(&payload)? else {
        return Err(lineage_error());
    };
    if rule.identity() != binding.rule_pack().id().as_str()
        || rule.version() != binding.rule_pack().version().get()
        || rule.owner().tenant_id() != binding.tenant_id()
    {
        return Err(lineage_error());
    }
    let subject = binding.subject().instant();
    if from > subject || subject >= to {
        return Err(invalid());
    }
    if from != rule.effective().from().instant()
        || to != rule.effective().to().instant()
        || rule.effective().from() != binding.effective_from()
        || rule.effective().to() != binding.effective_to()
    {
        return Err(lineage_error());
    }
    Ok(())
}

pub(crate) async fn validate_market_fact_units(
    transaction: &mut Transaction<'_, Postgres>,
    fact: &MarketFact,
    proof: &ResolvedMarketFactProof,
) -> Result<(), ApplicationError> {
    if proof.tenant_id() != fact.owner().tenant_id()
        || proof.fact_id() != fact.id()
        || proof.kind() != fact_kind(fact)
    {
        return Err(invalid());
    }

    for binding in proof.bindings() {
        let persisted: Option<(String, String, i32, i32, Vec<u8>)> = sqlx::query_as(
            "SELECT owner_id::text, dimension, scale, precision, payload
             FROM market.units
             WHERE tenant_id = $1 AND unit_id = $2 AND version = $3
             FOR SHARE",
        )
        .bind(proof.tenant_id().as_str())
        .bind(binding.unit().unit_id().as_str())
        .bind(version_i64(binding.unit().version().get())?)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
        let Some((owner_id, dimension, scale, precision, payload)) = persisted else {
            return Err(invalid());
        };
        let DefinitionValue::Unit(unit) = decode_definition(&payload)? else {
            return Err(invalid());
        };
        if unit.identity() != binding.unit().unit_id().as_str()
            || unit.version() != binding.unit().version().get()
            || unit.owner().tenant_id() != proof.tenant_id()
            || unit.owner().owner_id().as_str() != owner_id
            || unit.dimension() != binding.dimension()
            || unit.scale() != binding.scale()
            || unit.precision() != binding.precision()
            || dimension != binding.dimension()
            || u32::try_from(scale).ok() != Some(binding.scale())
            || u32::try_from(precision).ok() != Some(binding.precision())
        {
            return Err(invalid());
        }
    }
    Ok(())
}

pub(crate) async fn persist_market_fact(
    transaction: &mut Transaction<'_, Postgres>,
    fact: &MarketFact,
    idempotency_key: &str,
    fingerprint: &[u8],
) -> Result<MarketFact, ApplicationError> {
    let tenant_id = fact.owner().tenant_id().as_str();
    let fact_id = fact.id().as_str();
    let outcome = lock_idempotency(
        transaction,
        tenant_id,
        "market-fact:write:v1",
        idempotency_key,
        fingerprint,
        fact_id,
    )
    .await?;
    if outcome == IdempotencyOutcome::Replay {
        return Ok(fact.clone());
    }
    insert_fact(transaction, fact).await?;
    Ok(fact.clone())
}

// Keeping all immutable fact variants together makes the SQL mapping auditable against the enum.
#[allow(clippy::too_many_lines)]
async fn insert_fact(
    transaction: &mut Transaction<'_, Postgres>,
    fact: &MarketFact,
) -> Result<(), ApplicationError> {
    let payload = encode_fact(fact);
    match fact {
        MarketFact::Cashflow(value) => {
            sqlx::query(
                "INSERT INTO market.cashflows
                 (tenant_id, cashflow_id, owner_id, instrument_id, instrument_version,
                  fact_time, source_id, external_id, source_revision, supersedes_id, payload)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
            )
            .bind(value.owner().tenant_id().as_str())
            .bind(value.id().as_str())
            .bind(value.owner().owner_id().as_str())
            .bind(value.bond().id().as_str())
            .bind(version_i64(value.bond().version().get())?)
            .bind(value.payment_time().instant())
            .bind(value.source().source_id())
            .bind(value.source().external_id())
            .bind(version_i64(value.source().source_revision())?)
            .bind(
                value
                    .supersedes_id()
                    .map(ficant_domain::primitives::Ulid::as_str),
            )
            .bind(payload)
            .execute(&mut **transaction)
            .await
            .map_err(map_sqlx_error)?;
        }
        MarketFact::Quote(value) => {
            let data_source = value.source().data_source();
            sqlx::query(
                "INSERT INTO market.quotes
                 (tenant_id, quote_id, owner_id, instrument_id, instrument_version,
                  fact_time, received_at, source_id, external_id, source_revision,
                  supersedes_id, payload, data_source_id, data_source_version)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
            )
            .bind(value.owner().tenant_id().as_str())
            .bind(value.id().as_str())
            .bind(value.owner().owner_id().as_str())
            .bind(value.instrument().id().as_str())
            .bind(version_i64(value.instrument().version().get())?)
            .bind(value.observed_at().instant())
            .bind(value.received_at().instant())
            .bind(value.source().source_id())
            .bind(value.source().external_id())
            .bind(version_i64(value.source().source_revision())?)
            .bind(
                value
                    .supersedes_id()
                    .map(ficant_domain::primitives::Ulid::as_str),
            )
            .bind(payload)
            .bind(data_source.map(|reference| reference.id().as_str()))
            .bind(
                data_source
                    .map(|reference| version_i64(reference.version().get()))
                    .transpose()?,
            )
            .execute(&mut **transaction)
            .await
            .map_err(map_sqlx_error)?;
        }
        MarketFact::Trade(value) => {
            let data_source = value.source().data_source();
            sqlx::query(
                "INSERT INTO market.trades
                 (tenant_id, trade_id, owner_id, instrument_id, instrument_version,
                  fact_time, source_id, external_id, source_revision, supersedes_id, payload,
                  data_source_id, data_source_version)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
            )
            .bind(value.owner().tenant_id().as_str())
            .bind(value.id().as_str())
            .bind(value.owner().owner_id().as_str())
            .bind(value.instrument().id().as_str())
            .bind(version_i64(value.instrument().version().get())?)
            .bind(value.executed_at().instant())
            .bind(value.source().source_id())
            .bind(value.source().external_id())
            .bind(version_i64(value.source().source_revision())?)
            .bind(
                value
                    .supersedes_id()
                    .map(ficant_domain::primitives::Ulid::as_str),
            )
            .bind(payload)
            .bind(data_source.map(|reference| reference.id().as_str()))
            .bind(
                data_source
                    .map(|reference| version_i64(reference.version().get()))
                    .transpose()?,
            )
            .execute(&mut **transaction)
            .await
            .map_err(map_sqlx_error)?;
        }
        MarketFact::Valuation(value) => {
            let data_source = value.source().data_source();
            sqlx::query(
                "INSERT INTO market.valuations
                 (tenant_id, valuation_id, owner_id, instrument_id, instrument_version,
                  fact_time, source_id, external_id, source_revision, supersedes_id,
                  rule_pack_id, rule_pack_version, payload, data_source_id, data_source_version)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
            )
            .bind(value.owner().tenant_id().as_str())
            .bind(value.id().as_str())
            .bind(value.owner().owner_id().as_str())
            .bind(value.instrument().id().as_str())
            .bind(version_i64(value.instrument().version().get())?)
            .bind(value.valuation_at().instant())
            .bind(value.source().source_id())
            .bind(value.source().external_id())
            .bind(version_i64(value.source().source_revision())?)
            .bind(
                value
                    .supersedes_id()
                    .map(ficant_domain::primitives::Ulid::as_str),
            )
            .bind(value.rule_pack().id().as_str())
            .bind(version_i64(value.rule_pack().version().get())?)
            .bind(payload)
            .bind(data_source.map(|reference| reference.id().as_str()))
            .bind(
                data_source
                    .map(|reference| version_i64(reference.version().get()))
                    .transpose()?,
            )
            .execute(&mut **transaction)
            .await
            .map_err(map_sqlx_error)?;
        }
    }
    Ok(())
}

fn version_i64(value: u64) -> Result<i64, ApplicationError> {
    i64::try_from(value).map_err(|_| {
        ficant_application::map_domain_error(ficant_domain::DomainErrorCode::InvalidValue)
    })
}

fn owner_strings(scope: &AccessScope) -> Vec<String> {
    scope
        .allowed_owner_ids()
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect()
}

const fn fact_kind(fact: &MarketFact) -> MarketFactKind {
    match fact {
        MarketFact::Cashflow(_) => MarketFactKind::Cashflow,
        MarketFact::Quote(_) => MarketFactKind::Quote,
        MarketFact::Trade(_) => MarketFactKind::Trade,
        MarketFact::Valuation(_) => MarketFactKind::Valuation,
    }
}

fn parse_fact_cursor(value: &str) -> Result<(DateTime<Utc>, String, String), ApplicationError> {
    let mut fields = value.split('.');
    let micros = fields
        .next()
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(DateTime::<Utc>::from_timestamp_micros)
        .ok_or_else(invalid)?;
    let kind = fields
        .next()
        .filter(|value| matches!(*value, "1" | "2" | "3" | "4"));
    let id = fields.next().and_then(|value| Ulid::new(value).ok());
    match (kind, id, fields.next()) {
        (Some(kind), Some(id), None) => Ok((micros, kind.to_owned(), id.as_str().to_owned())),
        _ => Err(invalid()),
    }
}

fn invalid() -> ApplicationError {
    application_error(ApplicationErrorCategory::ValidationFailed, false)
}

fn storage_error() -> ApplicationError {
    application_error(ApplicationErrorCategory::StorageUnavailable, false)
}

fn lineage_error() -> ApplicationError {
    application_error(ApplicationErrorCategory::LineageIncomplete, false)
}
