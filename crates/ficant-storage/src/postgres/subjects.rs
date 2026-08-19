use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ficant_application::ports::{
    AccessScope, GovernedPublishSubjectState, GovernedRegisterSubject, SubjectRepository,
    subject_record_content_hash,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_domain::governance::FoundationChangeOperation;
use ficant_domain::primitives::{DecimalValue, OwnerRef, Ulid, UnitRef, Version, VersionRef};
use ficant_domain::subject::{
    AccessSet, ConstraintSetRef, FundingTier, LimitCeiling, Subject, SubjectRecord,
    SubjectStateSnapshot, SubjectVersion, TaxTreatment,
};
use sqlx::{Postgres, Transaction};

use super::PostgresRepository;
use super::common::{IdempotencyOutcome, application_error, lock_idempotency, map_sqlx_error};

#[async_trait]
impl SubjectRepository for PostgresRepository {
    async fn register_governed_subject(
        &self,
        command: GovernedRegisterSubject,
    ) -> Result<SubjectRecord, ApplicationError> {
        let value = command.value();
        let owner = value.subject().owner().ok_or_else(invalid)?;
        command.scope().authorize(owner)?;
        let tenant = owner.tenant_id().as_str();
        let subject_id = value.subject().id().as_str();
        let version = value.version().reference().version().get();
        let expected_latest = version
            .checked_sub(1)
            .ok_or_else(|| application_error(ApplicationErrorCategory::VersionConflict, false))?;
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let outcome = lock_idempotency(
            &mut transaction,
            tenant,
            "subject:register:v1",
            command.idempotency_key().as_str(),
            command.fingerprint().content_hash().as_bytes(),
            subject_id,
        )
        .await?;
        if outcome == IdempotencyOutcome::Replay {
            let persisted =
                read_subject_in_transaction(&mut transaction, tenant, subject_id, version)
                    .await?
                    .ok_or_else(immutable)?;
            if &persisted != value {
                return Err(immutable());
            }
            super::governance::verify_change_replay(
                &mut transaction,
                tenant,
                FoundationChangeOperation::RegisterSubject,
                &format!("subject:{subject_id}@{version}"),
                command.fingerprint().content_hash(),
            )
            .await?;
            transaction.commit().await.map_err(map_sqlx_error)?;
            return Ok(persisted);
        }

        lock_subject_identity(&mut transaction, owner, subject_id, expected_latest).await?;
        let before_hash = if expected_latest == 0 {
            None
        } else {
            Some(subject_record_content_hash(
                &read_subject_in_transaction(&mut transaction, tenant, subject_id, expected_latest)
                    .await?
                    .ok_or_else(|| {
                        application_error(ApplicationErrorCategory::VersionConflict, true)
                    })?,
            )?)
        };
        insert_subject(&mut transaction, value).await?;
        let updated = sqlx::query(
            "UPDATE core.subject_identities
             SET latest_version = $3
             WHERE tenant_id = $1 AND subject_id = $2 AND latest_version = $4",
        )
        .bind(tenant)
        .bind(subject_id)
        .bind(version_i64(version)?)
        .bind(version_i64(expected_latest)?)
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        if updated.rows_affected() != 1 {
            return Err(application_error(
                ApplicationErrorCategory::ConcurrencyConflict,
                true,
            ));
        }
        let change = command.change_record(before_hash)?;
        super::governance::insert_change(&mut transaction, tenant, &change).await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value.clone())
    }

    async fn publish_governed_subject_state(
        &self,
        command: GovernedPublishSubjectState,
    ) -> Result<SubjectStateSnapshot, ApplicationError> {
        let value = command.value();
        let owner = value.owner().ok_or_else(invalid)?;
        command.scope().authorize(owner)?;
        let tenant = owner.tenant_id().as_str();
        let snapshot_id = value.id().as_str();
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let outcome = lock_idempotency(
            &mut transaction,
            tenant,
            "subject-state:publish:v1",
            command.idempotency_key().as_str(),
            command.fingerprint().content_hash().as_bytes(),
            snapshot_id,
        )
        .await?;
        if outcome == IdempotencyOutcome::Replay {
            let persisted = read_state_in_transaction(&mut transaction, tenant, snapshot_id)
                .await?
                .ok_or_else(immutable)?;
            if &persisted != value {
                return Err(immutable());
            }
            super::governance::verify_change_replay(
                &mut transaction,
                tenant,
                FoundationChangeOperation::PublishSubjectState,
                &format!("subject-state:{snapshot_id}"),
                command.fingerprint().content_hash(),
            )
            .await?;
            transaction.commit().await.map_err(map_sqlx_error)?;
            return Ok(persisted);
        }

        let subject = read_subject_in_transaction(
            &mut transaction,
            tenant,
            value.subject_ref().id().as_str(),
            value.subject_ref().version().get(),
        )
        .await?
        .ok_or_else(|| application_error(ApplicationErrorCategory::LineageIncomplete, false))?;
        if subject.subject().owner() != Some(owner) {
            return Err(application_error(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            ));
        }
        insert_state(&mut transaction, value).await?;
        let change = command.change_record()?;
        super::governance::insert_change(&mut transaction, tenant, &change).await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value.clone())
    }

    async fn register_subject(
        &self,
        _value: SubjectRecord,
    ) -> Result<SubjectRecord, ApplicationError> {
        Err(fail_closed())
    }

    async fn get_subject(
        &self,
        reference: VersionRef,
    ) -> Result<Option<SubjectRecord>, ApplicationError> {
        read_subject_global(self, reference.id().as_str(), reference.version().get()).await
    }

    async fn register_subject_state(
        &self,
        _value: SubjectStateSnapshot,
    ) -> Result<SubjectStateSnapshot, ApplicationError> {
        Err(fail_closed())
    }

    async fn get_subject_state(
        &self,
        snapshot_id: Ulid,
        knowledge_at: DateTime<Utc>,
    ) -> Result<Option<SubjectStateSnapshot>, ApplicationError> {
        read_state_global(self, snapshot_id.as_str(), knowledge_at).await
    }

    async fn get_subject_scoped(
        &self,
        scope: &AccessScope,
        reference: VersionRef,
    ) -> Result<Option<SubjectRecord>, ApplicationError> {
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let value = read_subject_in_transaction(
            &mut transaction,
            scope.tenant_id().as_str(),
            reference.id().as_str(),
            reference.version().get(),
        )
        .await?;
        if let Some(value) = value.as_ref() {
            scope.authorize(value.subject().owner().ok_or_else(invalid)?)?;
        }
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value)
    }

    async fn get_subject_state_scoped(
        &self,
        scope: &AccessScope,
        snapshot_id: Ulid,
        knowledge_at: DateTime<Utc>,
    ) -> Result<Option<SubjectStateSnapshot>, ApplicationError> {
        let row: Option<StateRow> = sqlx::query_as(
            "SELECT tenant_id::text, owner_id::text, subject_id::text, subject_version,
                    net_capital_coefficient, net_capital_scale,
                    net_capital_unit_id::text, net_capital_unit_version,
                    observed_at, visible_at, market_timezone
             FROM core.subject_state_snapshots
             WHERE tenant_id = $1 AND snapshot_id = $2 AND visible_at <= $3",
        )
        .bind(scope.tenant_id().as_str())
        .bind(snapshot_id.as_str())
        .bind(knowledge_at)
        .fetch_optional(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        match row {
            Some(row) => {
                let value = decode_state(&snapshot_id, row, self).await?;
                scope.authorize(value.owner().ok_or_else(invalid)?)?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }
}

async fn lock_subject_identity(
    transaction: &mut Transaction<'_, Postgres>,
    owner: &OwnerRef,
    subject_id: &str,
    expected_latest: u64,
) -> Result<(), ApplicationError> {
    if expected_latest == 0 {
        sqlx::query(
            "INSERT INTO core.subject_identities
             (tenant_id, subject_id, owner_id, latest_version)
             VALUES ($1, $2, $3, 0)
             ON CONFLICT DO NOTHING",
        )
        .bind(owner.tenant_id().as_str())
        .bind(subject_id)
        .bind(owner.owner_id().as_str())
        .execute(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
    }
    let identity: Option<(String, i64)> = sqlx::query_as(
        "SELECT owner_id::text, latest_version
         FROM core.subject_identities
         WHERE tenant_id = $1 AND subject_id = $2
         FOR UPDATE",
    )
    .bind(owner.tenant_id().as_str())
    .bind(subject_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    match identity {
        Some((owner_id, latest))
            if owner_id == owner.owner_id().as_str() && latest == version_i64(expected_latest)? =>
        {
            Ok(())
        }
        _ => Err(application_error(
            ApplicationErrorCategory::VersionConflict,
            true,
        )),
    }
}

async fn insert_subject(
    transaction: &mut Transaction<'_, Postgres>,
    value: &SubjectRecord,
) -> Result<(), ApplicationError> {
    let owner = value.subject().owner().ok_or_else(invalid)?;
    let constraint = value.version().constraint_set_ref().map(|reference| {
        (
            reference.reference().id().as_str(),
            reference.reference().version().get(),
        )
    });
    sqlx::query(
        "INSERT INTO core.subject_versions
         (tenant_id, owner_id, subject_id, version, display_name, market_codes, tool_codes,
          funding_tier, value_added_tax_profile, income_tax_profile,
          assessment_mechanism, liability_profile, constraint_set_id, constraint_set_version)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(owner.tenant_id().as_str())
    .bind(owner.owner_id().as_str())
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

async fn insert_state(
    transaction: &mut Transaction<'_, Postgres>,
    value: &SubjectStateSnapshot,
) -> Result<(), ApplicationError> {
    let owner = value.owner().ok_or_else(invalid)?;
    sqlx::query(
        "INSERT INTO core.subject_state_snapshots
         (tenant_id, owner_id, snapshot_id, subject_id, subject_version,
          net_capital_coefficient, net_capital_scale,
          net_capital_unit_id, net_capital_unit_version,
          observed_at, visible_at, market_timezone)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(owner.tenant_id().as_str())
    .bind(owner.owner_id().as_str())
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
    .execute(&mut **transaction)
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
        .execute(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
    }
    Ok(())
}

async fn read_subject_global(
    repository: &PostgresRepository,
    subject_id: &str,
    version: u64,
) -> Result<Option<SubjectRecord>, ApplicationError> {
    let row: Option<SubjectRow> = sqlx::query_as(
        "SELECT tenant_id::text, owner_id::text, display_name, market_codes, tool_codes,
                funding_tier, value_added_tax_profile, income_tax_profile,
                assessment_mechanism, liability_profile,
                constraint_set_id::text, constraint_set_version
         FROM core.subject_versions
         WHERE subject_id = $1 AND version = $2",
    )
    .bind(subject_id)
    .bind(version_i64(version)?)
    .fetch_optional(repository.pool())
    .await
    .map_err(map_sqlx_error)?;
    row.map(|row| decode_subject(subject_id, version, row))
        .transpose()
}

async fn read_subject_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    tenant: &str,
    subject_id: &str,
    version: u64,
) -> Result<Option<SubjectRecord>, ApplicationError> {
    let row: Option<SubjectRow> = sqlx::query_as(
        "SELECT tenant_id::text, owner_id::text, display_name, market_codes, tool_codes,
                funding_tier, value_added_tax_profile, income_tax_profile,
                assessment_mechanism, liability_profile,
                constraint_set_id::text, constraint_set_version
         FROM core.subject_versions
         WHERE tenant_id = $1 AND subject_id = $2 AND version = $3
         FOR SHARE",
    )
    .bind(tenant)
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
    String,
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
        tenant_id,
        owner_id,
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
    let owner = OwnerRef::new(
        Ulid::new(tenant_id).map_err(map_domain_error)?,
        Ulid::new(owner_id).map_err(map_domain_error)?,
    );
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
        Subject::new_owned(id, owner, display_name).map_err(map_domain_error)?,
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
    String,
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

async fn read_state_global(
    repository: &PostgresRepository,
    snapshot_id: &str,
    knowledge_at: DateTime<Utc>,
) -> Result<Option<SubjectStateSnapshot>, ApplicationError> {
    let row: Option<StateRow> = sqlx::query_as(
        "SELECT tenant_id::text, owner_id::text, subject_id::text, subject_version,
                net_capital_coefficient, net_capital_scale,
                net_capital_unit_id::text, net_capital_unit_version,
                observed_at, visible_at, market_timezone
         FROM core.subject_state_snapshots
         WHERE snapshot_id = $1 AND visible_at <= $2",
    )
    .bind(snapshot_id)
    .bind(knowledge_at)
    .fetch_optional(repository.pool())
    .await
    .map_err(map_sqlx_error)?;
    match row {
        Some(row) => Ok(Some(
            decode_state(
                &Ulid::new(snapshot_id).map_err(map_domain_error)?,
                row,
                repository,
            )
            .await?,
        )),
        None => Ok(None),
    }
}

async fn read_state_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    tenant: &str,
    snapshot_id: &str,
) -> Result<Option<SubjectStateSnapshot>, ApplicationError> {
    let row: Option<StateRow> = sqlx::query_as(
        "SELECT tenant_id::text, owner_id::text, subject_id::text, subject_version,
                net_capital_coefficient, net_capital_scale,
                net_capital_unit_id::text, net_capital_unit_version,
                observed_at, visible_at, market_timezone
         FROM core.subject_state_snapshots
         WHERE tenant_id = $1 AND snapshot_id = $2
         FOR SHARE",
    )
    .bind(tenant)
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
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<SubjectStateSnapshot, ApplicationError> {
    let (
        tenant_id,
        owner_id,
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
    let owner = OwnerRef::new(
        Ulid::new(tenant_id).map_err(map_domain_error)?,
        Ulid::new(owner_id).map_err(map_domain_error)?,
    );
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
    SubjectStateSnapshot::new_owned(
        snapshot_id,
        subject_ref,
        net_capital,
        limit_ceilings,
        observed_at,
        visible_at,
        market_timezone,
        owner,
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

fn immutable() -> ApplicationError {
    application_error(ApplicationErrorCategory::ImmutableViolation, false)
}

fn fail_closed() -> ApplicationError {
    application_error(ApplicationErrorCategory::StateConflict, false)
}

fn invalid() -> ApplicationError {
    application_error(ApplicationErrorCategory::ValidationFailed, false)
}
