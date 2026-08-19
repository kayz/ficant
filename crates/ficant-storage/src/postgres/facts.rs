use async_trait::async_trait;
use ficant_application::ApplicationError;
use ficant_application::ApplicationErrorCategory;
use ficant_application::ports::{
    AccessScope, AppendMarketFact, CorrectMarketFact, Cursor, CursorPage, CurveSnapshotMetadata,
    CurveSnapshotMetadataRepository, DefinitionValue, GovernedAppendMarketFact,
    GovernedCorrectMarketFact, GovernedPublishCurveSnapshot, MarketFact, MarketFactKind,
    MarketFactRepository, MarketFactRuleProof, MarketFactRuleProofKind, MarketFactWindow,
    PublishCurveSnapshot, ResolvedMarketFactProof, market_fact_content_hash,
};
use ficant_domain::market::CurveSnapshot;
use ficant_domain::primitives::{MarketTime, Ulid};
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

struct StoredCurveSnapshotRow {
    payload: Vec<u8>,
    owner_id: String,
    as_of: DateTime<Utc>,
    currency_unit_id: String,
    currency_unit_version: i64,
    curve_kind: String,
    calendar_id: String,
    calendar_version: i64,
    rule_pack_id: String,
    rule_pack_version: i64,
    point_schema: String,
    content_hash: String,
    blob_size: i64,
    visible_at: Option<DateTime<Utc>>,
    curve_family_id: Option<String>,
    referenced_blob_size: Option<i64>,
}

type StoredFactRow = (
    DateTime<Utc>,
    DateTime<Utc>,
    String,
    String,
    String,
    String,
    i64,
    Vec<u8>,
);

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for StoredCurveSnapshotRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            payload: row.try_get("payload")?,
            owner_id: row.try_get("owner_id")?,
            as_of: row.try_get("as_of")?,
            currency_unit_id: row.try_get("currency_unit_id")?,
            currency_unit_version: row.try_get("currency_unit_version")?,
            curve_kind: row.try_get("curve_kind")?,
            calendar_id: row.try_get("calendar_id")?,
            calendar_version: row.try_get("calendar_version")?,
            rule_pack_id: row.try_get("rule_pack_id")?,
            rule_pack_version: row.try_get("rule_pack_version")?,
            point_schema: row.try_get("point_schema")?,
            content_hash: row.try_get("content_hash")?,
            blob_size: row.try_get("blob_size")?,
            visible_at: row.try_get("visible_at")?,
            curve_family_id: row.try_get("curve_family_id")?,
            referenced_blob_size: row.try_get("referenced_blob_size")?,
        })
    }
}

#[async_trait]
impl MarketFactRepository for PostgresRepository {
    async fn append_governed_fact(
        &self,
        command: GovernedAppendMarketFact,
    ) -> Result<MarketFact, ApplicationError> {
        let change = command.change_record()?;
        let fact = command.command().fact();
        let tenant = fact.owner().tenant_id().as_str();
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        validate_market_fact_units(&mut transaction, fact, command.command().proof()).await?;
        validate_market_fact_rule(&mut transaction, fact, command.command().rule_proof()).await?;
        let (value, outcome) = persist_market_fact(
            &mut transaction,
            fact,
            command.command().idempotency_key().as_str(),
            command.command().fingerprint().content_hash().as_bytes(),
        )
        .await?;
        persist_governance_outcome(
            &mut transaction,
            tenant,
            &change,
            command.fingerprint().content_hash(),
            outcome,
        )
        .await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value)
    }

    async fn append_governed_correction(
        &self,
        command: GovernedCorrectMarketFact,
    ) -> Result<MarketFact, ApplicationError> {
        let correction = command.command().correction();
        let tenant = correction.owner().tenant_id().as_str();
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let original = validate_market_fact_correction(
            &mut transaction,
            command.command().original_fact_id(),
            correction,
        )
        .await?;
        validate_market_fact_units(&mut transaction, correction, command.command().proof()).await?;
        validate_market_fact_rule(&mut transaction, correction, command.command().rule_proof())
            .await?;
        let (value, outcome) = persist_market_fact(
            &mut transaction,
            correction,
            command.command().idempotency_key().as_str(),
            command.command().fingerprint().content_hash().as_bytes(),
        )
        .await?;
        let change = command.change_record(market_fact_content_hash(&original))?;
        persist_governance_outcome(
            &mut transaction,
            tenant,
            &change,
            command.fingerprint().content_hash(),
            outcome,
        )
        .await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value)
    }

    async fn publish_governed_curve_snapshot(
        &self,
        command: GovernedPublishCurveSnapshot,
    ) -> Result<CurveSnapshot, ApplicationError> {
        let change = command.change_record()?;
        let tenant = command.command().curve().owner().tenant_id().as_str();
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let (value, outcome) = persist_curve_snapshot(&mut transaction, command.command()).await?;
        persist_governance_outcome(
            &mut transaction,
            tenant,
            &change,
            command.fingerprint().content_hash(),
            outcome,
        )
        .await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value)
    }

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
        query_market_fact_window(self, scope, query).await
    }

    async fn publish_curve_snapshot(
        &self,
        command: PublishCurveSnapshot,
    ) -> Result<CurveSnapshot, ApplicationError> {
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let (value, _) = persist_curve_snapshot(&mut transaction, &command).await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value)
    }

    async fn get_curve_snapshot(
        &self,
        scope: &AccessScope,
        curve_snapshot_id: Ulid,
    ) -> Result<Option<CurveSnapshot>, ApplicationError> {
        Ok(load_curve_snapshot_metadata(self, scope, curve_snapshot_id)
            .await?
            .map(|metadata| metadata.snapshot().clone()))
    }

    async fn get_curve_snapshot_at(
        &self,
        scope: &AccessScope,
        curve_snapshot_id: Ulid,
        knowledge_at: &MarketTime,
    ) -> Result<Option<CurveSnapshot>, ApplicationError> {
        Ok(
            load_curve_snapshot_metadata_at(self, scope, curve_snapshot_id, knowledge_at)
                .await?
                .map(|metadata| metadata.snapshot().clone()),
        )
    }
}

async fn query_market_fact_window(
    repository: &PostgresRepository,
    scope: &AccessScope,
    query: MarketFactWindow,
) -> Result<CursorPage<MarketFact>, ApplicationError> {
    query.authorize_scope(scope)?;
    let cursor = query
        .page()
        .cursor()
        .map(|value| parse_fact_cursor(value.opaque_value()))
        .transpose()?;
    let expected_binding =
        crate::s3::content_addressed::hash_hex(query.cursor_binding().content_hash());
    let cursor = cursor
        .map(|(binding, time, kind, id)| {
            if binding != expected_binding {
                return Err(invalid());
            }
            Ok((time, kind, id))
        })
        .transpose()?;
    let rows = fetch_market_fact_rows(repository, scope, &query, cursor).await?;
    let page_len = usize::try_from(query.page().limit()).map_err(|_| invalid())?;
    let has_more = rows.len() > page_len;
    let page_rows = rows.into_iter().take(page_len).collect::<Vec<_>>();
    let next_cursor = if has_more {
        let (time, _, kind, id, _, _, _, _) = page_rows.last().ok_or_else(storage_error)?;
        Some(Cursor::issue(
            repository.cursor_codec(),
            scope,
            format!(
                "{}.{}.{}.{}.{}",
                expected_binding,
                time.timestamp(),
                time.timestamp_subsec_nanos(),
                kind,
                id
            ),
        )?)
    } else {
        None
    };
    let items = page_rows
        .into_iter()
        .map(
            |(time, visible_at, kind, id, owner_id, instrument_id, version, payload)| {
                let fact = decode_fact(&payload)?;
                validate_fact_row(
                    &fact,
                    &kind,
                    &id,
                    &owner_id,
                    &instrument_id,
                    version,
                    time,
                    visible_at,
                    scope,
                    &query,
                )?;
                Ok(fact)
            },
        )
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CursorPage::new(items, next_cursor))
}

async fn fetch_market_fact_rows(
    repository: &PostgresRepository,
    scope: &AccessScope,
    query: &MarketFactWindow,
    cursor: Option<(DateTime<Utc>, String, String)>,
) -> Result<Vec<StoredFactRow>, ApplicationError> {
    let (has_cursor, cursor_time, cursor_kind, cursor_id) = cursor.map_or(
        (
            false,
            DateTime::<Utc>::UNIX_EPOCH,
            String::new(),
            String::new(),
        ),
        |(time, kind, id)| (true, time, kind, id),
    );
    sqlx::query_as(
        "WITH facts AS (
           SELECT fact_time, fact_time AS visible_at, '1'::text AS kind,
                  cashflow_id::text AS fact_id,
                  owner_id::text, instrument_id::text, instrument_version, payload
           FROM market.cashflows WHERE tenant_id=$1 AND instrument_id=$2
             AND instrument_version=$3 AND fact_time BETWEEN $4 AND $5
             AND owner_id::text=ANY($6::text[])
           UNION ALL SELECT fact_time, received_at, '2', quote_id::text, owner_id::text,
                   instrument_id::text, instrument_version, payload FROM market.quotes
           WHERE tenant_id=$1 AND instrument_id=$2 AND instrument_version=$3
             AND fact_time BETWEEN $4 AND $5 AND owner_id::text=ANY($6::text[])
           UNION ALL SELECT fact_time, fact_time, '3', trade_id::text, owner_id::text,
                  instrument_id::text, instrument_version, payload FROM market.trades
           WHERE tenant_id=$1 AND instrument_id=$2 AND instrument_version=$3
             AND fact_time BETWEEN $4 AND $5 AND owner_id::text=ANY($6::text[])
           UNION ALL SELECT fact_time, fact_time, '4', valuation_id::text, owner_id::text,
                  instrument_id::text, instrument_version, payload FROM market.valuations
           WHERE tenant_id=$1 AND instrument_id=$2 AND instrument_version=$3
             AND fact_time BETWEEN $4 AND $5 AND owner_id::text=ANY($6::text[]))
         SELECT fact_time,visible_at,kind,fact_id,owner_id,instrument_id,instrument_version,payload
         FROM facts
         WHERE visible_at <= $7 AND (NOT $8 OR (fact_time,kind,fact_id)>($9,$10,$11))
         ORDER BY fact_time,kind,fact_id LIMIT $12",
    )
    .bind(scope.tenant_id().as_str())
    .bind(query.instrument().id().as_str())
    .bind(version_i64(query.instrument().version().get())?)
    .bind(query.from().instant())
    .bind(query.to().instant())
    .bind(owner_strings(scope))
    .bind(query.knowledge_at().instant())
    .bind(has_cursor)
    .bind(cursor_time)
    .bind(cursor_kind)
    .bind(cursor_id)
    .bind(i64::from(query.page().limit()) + 1)
    .fetch_all(repository.pool())
    .await
    .map_err(map_sqlx_error)
}

async fn load_curve_snapshot_metadata_at(
    repository: &PostgresRepository,
    scope: &AccessScope,
    curve_snapshot_id: Ulid,
    knowledge_at: &MarketTime,
) -> Result<Option<CurveSnapshotMetadata>, ApplicationError> {
    let owners = owner_strings(scope);
    let row: Option<StoredCurveSnapshotRow> = sqlx::query_as(
        "SELECT curve.payload, curve.owner_id::text, curve.as_of,
                curve.currency_unit_id::text, curve.currency_unit_version,
                curve.curve_kind, curve.calendar_id::text, curve.calendar_version,
                curve.rule_pack_id::text, curve.rule_pack_version, curve.point_schema,
                curve.content_hash::text, curve.blob_size, curve.visible_at,
                curve.curve_family_id, blob.blob_size AS referenced_blob_size
         FROM market.curve_snapshots curve
         LEFT JOIN storage.blobs blob
           ON blob.tenant_id=curve.tenant_id AND blob.content_hash=curve.content_hash
         WHERE curve.tenant_id=$1 AND curve.curve_snapshot_id=$2
           AND curve.owner_id::text = ANY($3::text[])
           AND curve.visible_at IS NOT NULL AND curve.visible_at <= $4",
    )
    .bind(scope.tenant_id().as_str())
    .bind(curve_snapshot_id.as_str())
    .bind(&owners)
    .bind(knowledge_at.instant())
    .fetch_optional(repository.pool())
    .await
    .map_err(map_sqlx_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let curve = decode_curve_snapshot(&row.payload)?;
    validate_curve_snapshot_row(&row, &curve, scope, &curve_snapshot_id)?;
    let visible_at = curve.visible_at().ok_or_else(storage_error)?;
    if visible_at.instant() > knowledge_at.instant() {
        return Err(storage_error());
    }
    curve_snapshot_metadata(&row, curve).map(Some)
}

async fn persist_curve_snapshot(
    transaction: &mut Transaction<'_, Postgres>,
    command: &PublishCurveSnapshot,
) -> Result<(CurveSnapshot, IdempotencyOutcome), ApplicationError> {
    let curve = command.curve();
    command.scope().authorize(curve.owner())?;
    let tenant = curve.owner().tenant_id().as_str();
    let outcome = lock_idempotency(
        transaction,
        tenant,
        "curve-snapshot:publish:v1",
        command.idempotency_key().as_str(),
        command.fingerprint().content_hash().as_bytes(),
        curve.id().as_str(),
    )
    .await?;
    if outcome == IdempotencyOutcome::Replay {
        verify_curve_snapshot_replay(transaction, command).await?;
        return Ok((curve.clone(), outcome));
    }
    let existing: Option<(Vec<u8>,)> = sqlx::query_as(
        "SELECT fingerprint FROM market.curve_snapshots
         WHERE tenant_id = $1 AND curve_snapshot_id = $2
         FOR UPDATE",
    )
    .bind(tenant)
    .bind(curve.id().as_str())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    if existing.is_some() {
        return Err(application_error(
            ApplicationErrorCategory::ImmutableViolation,
            false,
        ));
    }
    publish_blob_reference(
        transaction,
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
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    insert_lineage(transaction, tenant, curve.id().as_str(), curve.lineage()).await?;
    Ok((curve.clone(), outcome))
}

async fn verify_curve_snapshot_replay(
    transaction: &mut Transaction<'_, Postgres>,
    command: &PublishCurveSnapshot,
) -> Result<(), ApplicationError> {
    let curve = command.curve();
    let persisted: Option<(Vec<u8>, String, i64)> = sqlx::query_as(
        "SELECT payload, content_hash::text, blob_size FROM market.curve_snapshots
         WHERE tenant_id=$1 AND curve_snapshot_id=$2 AND owner_id=$3 FOR SHARE",
    )
    .bind(curve.owner().tenant_id().as_str())
    .bind(curve.id().as_str())
    .bind(curve.owner().owner_id().as_str())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let Some((payload, content_hash, blob_size)) = persisted else {
        return Err(storage_error());
    };
    if decode_curve_snapshot(&payload)? != *curve
        || content_hash != crate::s3::content_addressed::hash_hex(curve.content_hash())
        || u64::try_from(blob_size).ok() != Some(command.declared_blob_size())
    {
        return Err(application_error(
            ApplicationErrorCategory::ImmutableViolation,
            false,
        ));
    }
    Ok(())
}

async fn persist_governance_outcome(
    transaction: &mut Transaction<'_, Postgres>,
    tenant: &str,
    change: &ficant_domain::governance::FoundationChangeRecord,
    fingerprint: &ficant_domain::primitives::ContentHash,
    outcome: IdempotencyOutcome,
) -> Result<(), ApplicationError> {
    match outcome {
        IdempotencyOutcome::Fresh => {
            super::governance::insert_change(transaction, tenant, change).await
        }
        IdempotencyOutcome::Replay => {
            super::governance::verify_change_replay(
                transaction,
                tenant,
                change.operation(),
                &change.resource().canonical_ref(),
                fingerprint,
            )
            .await
        }
    }
}

#[async_trait]
impl CurveSnapshotMetadataRepository for PostgresRepository {
    async fn get_curve_snapshot_metadata(
        &self,
        scope: &AccessScope,
        curve_snapshot_id: Ulid,
    ) -> Result<Option<CurveSnapshotMetadata>, ApplicationError> {
        load_curve_snapshot_metadata(self, scope, curve_snapshot_id).await
    }
}

async fn load_curve_snapshot_metadata(
    repository: &PostgresRepository,
    scope: &AccessScope,
    curve_snapshot_id: Ulid,
) -> Result<Option<CurveSnapshotMetadata>, ApplicationError> {
    let owners = owner_strings(scope);
    let row: Option<StoredCurveSnapshotRow> = sqlx::query_as(
        "SELECT curve.payload, curve.owner_id::text, curve.as_of,
                curve.currency_unit_id::text, curve.currency_unit_version,
                curve.curve_kind, curve.calendar_id::text, curve.calendar_version,
                curve.rule_pack_id::text, curve.rule_pack_version, curve.point_schema,
                curve.content_hash::text, curve.blob_size, curve.visible_at,
                curve.curve_family_id, blob.blob_size AS referenced_blob_size
         FROM market.curve_snapshots curve
         LEFT JOIN storage.blobs blob
           ON blob.tenant_id=curve.tenant_id AND blob.content_hash=curve.content_hash
         WHERE curve.tenant_id=$1 AND curve.curve_snapshot_id=$2
           AND curve.owner_id::text = ANY($3::text[])",
    )
    .bind(scope.tenant_id().as_str())
    .bind(curve_snapshot_id.as_str())
    .bind(&owners)
    .fetch_optional(repository.pool())
    .await
    .map_err(map_sqlx_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let curve = decode_curve_snapshot(&row.payload)?;
    validate_curve_snapshot_row(&row, &curve, scope, &curve_snapshot_id)?;
    curve_snapshot_metadata(&row, curve).map(Some)
}

fn curve_snapshot_metadata(
    row: &StoredCurveSnapshotRow,
    curve: CurveSnapshot,
) -> Result<CurveSnapshotMetadata, ApplicationError> {
    let Some(referenced_blob_size) = row.referenced_blob_size else {
        return Err(lineage_error());
    };
    if referenced_blob_size != row.blob_size {
        return Err(storage_error());
    }
    let blob_size = u64::try_from(row.blob_size).map_err(|_| storage_error())?;
    CurveSnapshotMetadata::new(curve, blob_size)
}

fn validate_curve_snapshot_row(
    row: &StoredCurveSnapshotRow,
    curve: &CurveSnapshot,
    scope: &AccessScope,
    requested_id: &Ulid,
) -> Result<(), ApplicationError> {
    let visible_matches = match (&row.visible_at, curve.visible_at()) {
        (None, None) => true,
        (Some(stored), Some(decoded)) => *stored == decoded.instant(),
        _ => false,
    };
    let valid = curve.id() == requested_id
        && curve.owner().tenant_id() == scope.tenant_id()
        && curve.owner().owner_id().as_str() == row.owner_id
        && curve.as_of().instant() == row.as_of
        && curve.currency().unit_id().as_str() == row.currency_unit_id
        && u64::try_from(row.currency_unit_version).ok() == Some(curve.currency().version().get())
        && curve.curve_kind() == row.curve_kind
        && curve.calendar().id().as_str() == row.calendar_id
        && u64::try_from(row.calendar_version).ok() == Some(curve.calendar().version().get())
        && curve.rule_pack().id().as_str() == row.rule_pack_id
        && u64::try_from(row.rule_pack_version).ok() == Some(curve.rule_pack().version().get())
        && curve.point_schema() == row.point_schema
        && crate::s3::content_addressed::hash_hex(curve.content_hash()) == row.content_hash
        && visible_matches
        && curve.curve_family_id() == row.curve_family_id.as_deref();
    if valid { Ok(()) } else { Err(storage_error()) }
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
        let (value, _) = persist_market_fact(
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
        let _ = validate_market_fact_correction(
            &mut transaction,
            command.original_fact_id(),
            command.correction(),
        )
        .await?;
        validate_market_fact_units(&mut transaction, command.correction(), command.proof()).await?;
        validate_market_fact_rule(&mut transaction, command.correction(), command.rule_proof())
            .await?;
        let (value, _) = persist_market_fact(
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

async fn validate_market_fact_correction(
    transaction: &mut Transaction<'_, Postgres>,
    original_fact_id: &Ulid,
    correction: &MarketFact,
) -> Result<MarketFact, ApplicationError> {
    let original = load_market_fact(
        transaction,
        correction.owner().tenant_id().as_str(),
        original_fact_id,
        correction,
    )
    .await?
    .ok_or_else(lineage_error)?;
    if original.id() != original_fact_id || !same_fact_stream(&original, correction) {
        return Err(lineage_error());
    }
    Ok(original)
}

async fn load_market_fact(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    fact_id: &Ulid,
    fact_kind: &MarketFact,
) -> Result<Option<MarketFact>, ApplicationError> {
    let payload: Option<Vec<u8>> = match fact_kind {
        MarketFact::Cashflow(_) => sqlx::query_scalar(
            "SELECT payload FROM market.cashflows
             WHERE tenant_id=$1 AND cashflow_id=$2 FOR SHARE",
        )
        .bind(tenant_id)
        .bind(fact_id.as_str())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?,
        MarketFact::Quote(_) => sqlx::query_scalar(
            "SELECT payload FROM market.quotes
             WHERE tenant_id=$1 AND quote_id=$2 FOR SHARE",
        )
        .bind(tenant_id)
        .bind(fact_id.as_str())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?,
        MarketFact::Trade(_) => sqlx::query_scalar(
            "SELECT payload FROM market.trades
             WHERE tenant_id=$1 AND trade_id=$2 FOR SHARE",
        )
        .bind(tenant_id)
        .bind(fact_id.as_str())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?,
        MarketFact::Valuation(_) => sqlx::query_scalar(
            "SELECT payload FROM market.valuations
             WHERE tenant_id=$1 AND valuation_id=$2 FOR SHARE",
        )
        .bind(tenant_id)
        .bind(fact_id.as_str())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?,
    };
    payload.map(|payload| decode_fact(&payload)).transpose()
}

fn same_fact_stream(original: &MarketFact, correction: &MarketFact) -> bool {
    if original.owner() != correction.owner()
        || fact_instrument(original) != fact_instrument(correction)
        || correction.source_revision() <= original.source_revision()
    {
        return false;
    }
    match (original, correction) {
        (MarketFact::Cashflow(left), MarketFact::Cashflow(right)) => {
            same_source(left.source(), right.source())
        }
        (MarketFact::Quote(left), MarketFact::Quote(right)) => {
            same_source(left.source(), right.source())
        }
        (MarketFact::Trade(left), MarketFact::Trade(right)) => {
            same_source(left.source(), right.source())
        }
        (MarketFact::Valuation(left), MarketFact::Valuation(right)) => {
            same_source(left.source(), right.source())
        }
        _ => false,
    }
}

fn same_source(
    left: &ficant_domain::market::FactSource,
    right: &ficant_domain::market::FactSource,
) -> bool {
    left.source_id() == right.source_id()
        && left.external_id() == right.external_id()
        && left.data_source() == right.data_source()
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
) -> Result<(MarketFact, IdempotencyOutcome), ApplicationError> {
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
        let persisted = load_market_fact(transaction, tenant_id, fact.id(), fact)
            .await?
            .ok_or_else(storage_error)?;
        if persisted != *fact {
            return Err(application_error(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            ));
        }
        return Ok((persisted, outcome));
    }
    insert_fact(transaction, fact).await?;
    Ok((fact.clone(), outcome))
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

#[allow(clippy::too_many_arguments)]
fn validate_fact_row(
    fact: &MarketFact,
    stored_kind: &str,
    stored_id: &str,
    stored_owner_id: &str,
    stored_instrument_id: &str,
    stored_instrument_version: i64,
    stored_time: DateTime<Utc>,
    stored_visible_at: DateTime<Utc>,
    scope: &AccessScope,
    query: &MarketFactWindow,
) -> Result<(), ApplicationError> {
    let instrument = fact_instrument(fact);
    let valid = fact_kind_code(fact) == stored_kind
        && fact.id().as_str() == stored_id
        && fact.owner().tenant_id() == scope.tenant_id()
        && fact.owner().owner_id().as_str() == stored_owner_id
        && instrument == query.instrument()
        && instrument.id().as_str() == stored_instrument_id
        && u64::try_from(stored_instrument_version).ok() == Some(instrument.version().get())
        && database_time_matches(fact_time(fact), stored_time)
        && database_time_matches(fact_visible_time(fact), stored_visible_at)
        && fact_visible_time(fact) <= query.knowledge_at().instant();
    if valid { Ok(()) } else { Err(storage_error()) }
}

const fn fact_kind_code(fact: &MarketFact) -> &'static str {
    match fact {
        MarketFact::Cashflow(_) => "1",
        MarketFact::Quote(_) => "2",
        MarketFact::Trade(_) => "3",
        MarketFact::Valuation(_) => "4",
    }
}

fn fact_instrument(fact: &MarketFact) -> &ficant_domain::primitives::VersionRef {
    match fact {
        MarketFact::Cashflow(value) => value.bond(),
        MarketFact::Quote(value) => value.instrument(),
        MarketFact::Trade(value) => value.instrument(),
        MarketFact::Valuation(value) => value.instrument(),
    }
}

fn fact_time(fact: &MarketFact) -> DateTime<Utc> {
    match fact {
        MarketFact::Cashflow(value) => value.payment_time().instant(),
        MarketFact::Quote(value) => value.observed_at().instant(),
        MarketFact::Trade(value) => value.executed_at().instant(),
        MarketFact::Valuation(value) => value.valuation_at().instant(),
    }
}

fn fact_visible_time(fact: &MarketFact) -> DateTime<Utc> {
    match fact {
        MarketFact::Quote(value) => value.received_at().instant(),
        _ => fact_time(fact),
    }
}

fn database_time_matches(decoded: DateTime<Utc>, stored: DateTime<Utc>) -> bool {
    decoded.timestamp() == stored.timestamp()
        && decoded.timestamp_subsec_micros() == stored.timestamp_subsec_micros()
}

const fn fact_kind(fact: &MarketFact) -> MarketFactKind {
    match fact {
        MarketFact::Cashflow(_) => MarketFactKind::Cashflow,
        MarketFact::Quote(_) => MarketFactKind::Quote,
        MarketFact::Trade(_) => MarketFactKind::Trade,
        MarketFact::Valuation(_) => MarketFactKind::Valuation,
    }
}

fn parse_fact_cursor(
    value: &str,
) -> Result<(String, DateTime<Utc>, String, String), ApplicationError> {
    let mut fields = value.split('.');
    let binding = fields
        .next()
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(invalid)?;
    let seconds = fields
        .next()
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(invalid)?;
    let nanos = fields
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(invalid)?;
    let timestamp = DateTime::<Utc>::from_timestamp(seconds, nanos).ok_or_else(invalid)?;
    let kind = fields
        .next()
        .filter(|value| matches!(*value, "1" | "2" | "3" | "4"));
    let id = fields.next().and_then(|value| Ulid::new(value).ok());
    match (kind, id, fields.next()) {
        (Some(kind), Some(id), None) => Ok((
            binding.to_ascii_lowercase(),
            timestamp,
            kind.to_owned(),
            id.as_str().to_owned(),
        )),
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
