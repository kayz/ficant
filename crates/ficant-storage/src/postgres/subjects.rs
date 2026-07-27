use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ficant_application::ports::SubjectRepository;
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_domain::primitives::{DecimalValue, Ulid, UnitRef, Version, VersionRef};
use ficant_domain::subject::{
    AccessSet, ConstraintSetRef, FundingTier, LimitCeiling, Subject, SubjectRecord,
    SubjectStateSnapshot, SubjectVersion, TaxTreatment,
};

use super::PostgresRepository;
use super::common::{application_error, map_sqlx_error};

#[async_trait]
impl SubjectRepository for PostgresRepository {
    async fn register_subject(
        &self,
        value: SubjectRecord,
    ) -> Result<SubjectRecord, ApplicationError> {
        let subject_id = value.subject().id().as_str();
        let version = value.version().reference().version().get();
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;

        if let Some(existing) =
            read_subject_in_transaction(&mut transaction, subject_id, version).await?
        {
            if existing == value {
                transaction.commit().await.map_err(map_sqlx_error)?;
                return Ok(existing);
            }
            return Err(application_error(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            ));
        }

        let latest: Option<i64> = sqlx::query_scalar(
            "SELECT max(version) FROM core.subject_versions WHERE subject_id = $1",
        )
        .bind(subject_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        let expected = version
            .checked_sub(1)
            .ok_or_else(|| application_error(ApplicationErrorCategory::VersionConflict, false))?;
        if latest.unwrap_or(0) != i64::try_from(expected).map_err(|_| invalid())? {
            return Err(application_error(
                ApplicationErrorCategory::VersionConflict,
                true,
            ));
        }

        insert_subject(&mut transaction, &value).await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value)
    }

    async fn get_subject(
        &self,
        reference: VersionRef,
    ) -> Result<Option<SubjectRecord>, ApplicationError> {
        read_subject(self, reference.id().as_str(), reference.version().get()).await
    }

    async fn register_subject_state(
        &self,
        value: SubjectStateSnapshot,
    ) -> Result<SubjectStateSnapshot, ApplicationError> {
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        if let Some(existing) =
            read_state_in_transaction(&mut transaction, value.id().as_str()).await?
        {
            if existing == value {
                transaction.commit().await.map_err(map_sqlx_error)?;
                return Ok(existing);
            }
            return Err(application_error(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            ));
        }

        sqlx::query(
            "INSERT INTO core.subject_state_snapshots
             (snapshot_id, subject_id, subject_version,
              net_capital_coefficient, net_capital_scale,
              net_capital_unit_id, net_capital_unit_version,
              observed_at, visible_at, market_timezone)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(value.id().as_str())
        .bind(value.subject_ref().id().as_str())
        .bind(version_i64(value.subject_ref().version().get())?)
        .bind(value.net_capital().coefficient())
        .bind(i32::try_from(value.net_capital().scale()).map_err(|_| invalid())?)
        .bind(value.net_capital().unit().unit_id().as_str())
        .bind(version_i64(value.net_capital().unit().version().get())?)
        .bind(value.observed_at())
        .bind(value.visible_at())
        .bind(value.market_timezone())
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;

        for ceiling in value.limit_ceilings() {
            sqlx::query(
                "INSERT INTO core.subject_state_limit_ceilings
                 (snapshot_id, limit_code, coefficient, scale, unit_id, unit_version)
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(value.id().as_str())
            .bind(ceiling.limit_code())
            .bind(ceiling.ceiling().coefficient())
            .bind(i32::try_from(ceiling.ceiling().scale()).map_err(|_| invalid())?)
            .bind(ceiling.ceiling().unit().unit_id().as_str())
            .bind(version_i64(ceiling.ceiling().unit().version().get())?)
            .execute(&mut *transaction)
            .await
            .map_err(map_sqlx_error)?;
        }
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value)
    }

    async fn get_subject_state(
        &self,
        snapshot_id: Ulid,
        knowledge_at: DateTime<Utc>,
    ) -> Result<Option<SubjectStateSnapshot>, ApplicationError> {
        let row: Option<StateRow> = sqlx::query_as(
            "SELECT subject_id::text, subject_version,
                    net_capital_coefficient, net_capital_scale,
                    net_capital_unit_id::text, net_capital_unit_version,
                    observed_at, visible_at, market_timezone
             FROM core.subject_state_snapshots
             WHERE snapshot_id = $1 AND visible_at <= $2",
        )
        .bind(snapshot_id.as_str())
        .bind(knowledge_at)
        .fetch_optional(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        match row {
            Some(row) => Ok(Some(decode_state(&snapshot_id, row, self).await?)),
            None => Ok(None),
        }
    }
}

async fn insert_subject(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    value: &SubjectRecord,
) -> Result<(), ApplicationError> {
    let constraint = value.version().constraint_set_ref().map(|reference| {
        (
            reference.reference().id().as_str(),
            reference.reference().version().get(),
        )
    });
    sqlx::query(
        "INSERT INTO core.subject_versions
         (subject_id, version, display_name, market_codes, tool_codes,
          funding_tier, value_added_tax_profile, income_tax_profile,
          assessment_mechanism, liability_profile,
          constraint_set_id, constraint_set_version)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(value.subject().id().as_str())
    .bind(version_i64(value.version().reference().version().get())?)
    .bind(value.subject().display_name())
    .bind(value.version().access_set().market_codes())
    .bind(value.version().access_set().tool_codes())
    .bind(funding_tier_text(value.version().funding_tier()))
    .bind(value.version().tax_treatment().value_added_tax_profile())
    .bind(value.version().tax_treatment().income_tax_profile())
    .bind(value.version().assessment_mechanism())
    .bind(value.version().liability_profile())
    .bind(constraint.map(|value| value.0))
    .bind(constraint.map(|value| version_i64(value.1)).transpose()?)
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}

async fn read_subject(
    repository: &PostgresRepository,
    subject_id: &str,
    version: u64,
) -> Result<Option<SubjectRecord>, ApplicationError> {
    let mut transaction = repository.pool().begin().await.map_err(map_sqlx_error)?;
    let value = read_subject_in_transaction(&mut transaction, subject_id, version).await?;
    transaction.commit().await.map_err(map_sqlx_error)?;
    Ok(value)
}

async fn read_subject_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject_id: &str,
    version: u64,
) -> Result<Option<SubjectRecord>, ApplicationError> {
    let row: Option<SubjectRow> = sqlx::query_as(
        "SELECT display_name, market_codes, tool_codes, funding_tier,
                value_added_tax_profile, income_tax_profile,
                assessment_mechanism, liability_profile,
                constraint_set_id::text, constraint_set_version
         FROM core.subject_versions
         WHERE subject_id = $1 AND version = $2
         FOR SHARE",
    )
    .bind(subject_id)
    .bind(version_i64(version)?)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    row.map(|row| decode_subject(subject_id, version, row))
        .transpose()
}

type SubjectRow = (
    String,
    Vec<String>,
    Vec<String>,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<i64>,
);

fn decode_subject(
    subject_id: &str,
    version: u64,
    row: SubjectRow,
) -> Result<SubjectRecord, ApplicationError> {
    let (
        display_name,
        markets,
        tools,
        funding,
        vat,
        income,
        assessment,
        liability,
        constraint_id,
        constraint_version,
    ) = row;
    let id = Ulid::new(subject_id).map_err(map_domain_error)?;
    let version = Version::new(version).map_err(map_domain_error)?;
    let reference = VersionRef::new(id.clone(), version);
    let constraint = match (constraint_id, constraint_version) {
        (Some(id), Some(version)) => Some(ConstraintSetRef::new(VersionRef::new(
            Ulid::new(&id).map_err(map_domain_error)?,
            Version::new(u64::try_from(version).map_err(|_| invalid())?)
                .map_err(map_domain_error)?,
        ))),
        (None, None) => None,
        _ => return Err(invalid()),
    };
    let funding_tier = match funding.as_str() {
        "DR_AVAILABLE" => FundingTier::DrAvailable,
        "R_ONLY" => FundingTier::ROnly,
        _ => return Err(invalid()),
    };
    SubjectRecord::new(
        Subject::new(id, display_name).map_err(map_domain_error)?,
        SubjectVersion::new(
            reference,
            AccessSet::new(markets, tools).map_err(map_domain_error)?,
            funding_tier,
            TaxTreatment::new(vat, income).map_err(map_domain_error)?,
            assessment,
            liability,
            constraint,
        )
        .map_err(map_domain_error)?,
    )
    .map_err(map_domain_error)
}

type StateRow = (
    String,
    i64,
    String,
    i32,
    String,
    i64,
    DateTime<Utc>,
    DateTime<Utc>,
    String,
);

async fn read_state_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    snapshot_id: &str,
) -> Result<Option<SubjectStateSnapshot>, ApplicationError> {
    let row: Option<StateRow> = sqlx::query_as(
        "SELECT subject_id::text, subject_version,
                net_capital_coefficient, net_capital_scale,
                net_capital_unit_id::text, net_capital_unit_version,
                observed_at, visible_at, market_timezone
         FROM core.subject_state_snapshots
         WHERE snapshot_id = $1
         FOR SHARE",
    )
    .bind(snapshot_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let Some(row) = row else { return Ok(None) };
    decode_state_from_transaction(snapshot_id, row, transaction)
        .await
        .map(Some)
}

async fn decode_state(
    snapshot_id: &Ulid,
    row: StateRow,
    repository: &PostgresRepository,
) -> Result<SubjectStateSnapshot, ApplicationError> {
    let mut transaction = repository.pool().begin().await.map_err(map_sqlx_error)?;
    let value = decode_state_from_transaction(snapshot_id.as_str(), row, &mut transaction).await?;
    transaction.commit().await.map_err(map_sqlx_error)?;
    Ok(value)
}

async fn decode_state_from_transaction(
    snapshot_id: &str,
    row: StateRow,
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<SubjectStateSnapshot, ApplicationError> {
    let (
        subject_id,
        subject_version,
        coefficient,
        scale,
        unit_id,
        unit_version,
        observed_at,
        visible_at,
        market_timezone,
    ) = row;
    let snapshot_id = Ulid::new(snapshot_id).map_err(map_domain_error)?;
    let subject_ref = VersionRef::new(
        Ulid::new(&subject_id).map_err(map_domain_error)?,
        Version::new(u64::try_from(subject_version).map_err(|_| invalid())?)
            .map_err(map_domain_error)?,
    );
    let net_capital = decode_decimal(coefficient, scale, &unit_id, unit_version)?;
    let rows: Vec<LimitRow> = sqlx::query_as(
        "SELECT limit_code, coefficient, scale, unit_id::text, unit_version
         FROM core.subject_state_limit_ceilings
         WHERE snapshot_id = $1
         ORDER BY limit_code",
    )
    .bind(snapshot_id.as_str())
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let limit_ceilings = rows
        .into_iter()
        .map(|(code, coefficient, scale, unit_id, unit_version)| {
            LimitCeiling::new(
                code,
                decode_decimal(coefficient, scale, &unit_id, unit_version)?,
            )
            .map_err(map_domain_error)
        })
        .collect::<Result<Vec<_>, _>>()?;
    SubjectStateSnapshot::new(
        snapshot_id,
        subject_ref,
        net_capital,
        limit_ceilings,
        observed_at,
        visible_at,
        market_timezone,
    )
    .map_err(map_domain_error)
}

type LimitRow = (String, String, i32, String, i64);

fn decode_decimal(
    coefficient: String,
    scale: i32,
    unit_id: &str,
    unit_version: i64,
) -> Result<DecimalValue, ApplicationError> {
    DecimalValue::new(
        coefficient,
        u32::try_from(scale).map_err(|_| invalid())?,
        UnitRef::new(
            Ulid::new(unit_id).map_err(map_domain_error)?,
            Version::new(u64::try_from(unit_version).map_err(|_| invalid())?)
                .map_err(map_domain_error)?,
        ),
    )
    .map_err(map_domain_error)
}

const fn funding_tier_text(value: FundingTier) -> &'static str {
    match value {
        FundingTier::DrAvailable => "DR_AVAILABLE",
        FundingTier::ROnly => "R_ONLY",
    }
}

fn version_i64(value: u64) -> Result<i64, ApplicationError> {
    i64::try_from(value).map_err(|_| invalid())
}

fn invalid() -> ApplicationError {
    application_error(ApplicationErrorCategory::ValidationFailed, false)
}
