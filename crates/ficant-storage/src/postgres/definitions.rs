use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, AppendDefinitionVersion, DefinitionIdentity, DefinitionKind, DefinitionRepository,
    DefinitionValue, InstrumentSubtype,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::primitives::{MarketTime, Ulid, Version};
use ficant_domain::{ContentAddressed, VersionedDefinition};
use sqlx::{Postgres, Transaction};

use super::PostgresRepository;
use super::codec::{decode_definition, encode_definition};
use super::common::{IdempotencyOutcome, application_error, lock_idempotency, map_sqlx_error};

#[async_trait]
impl DefinitionRepository for PostgresRepository {
    async fn create_identity(&self, identity: DefinitionIdentity) -> Result<(), ApplicationError> {
        let tenant_id = identity.owner().tenant_id().as_str();
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        sqlx::query(
            "INSERT INTO core.definition_identities
             (tenant_id, definition_id, owner_id, kind, idempotency_key, fingerprint)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT DO NOTHING",
        )
        .bind(tenant_id)
        .bind(identity.definition_id().as_str())
        .bind(identity.owner().owner_id().as_str())
        .bind(definition_kind(identity.kind()))
        .bind(identity.idempotency_key().as_str())
        .bind(identity.fingerprint().content_hash().as_bytes().as_slice())
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        let persisted: Option<(String, String, String, Vec<u8>)> = sqlx::query_as(
            "SELECT owner_id::text, kind, idempotency_key, fingerprint
             FROM core.definition_identities
             WHERE tenant_id = $1 AND definition_id = $2
             FOR UPDATE",
        )
        .bind(tenant_id)
        .bind(identity.definition_id().as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        let expected = (
            identity.owner().owner_id().as_str().to_owned(),
            definition_kind(identity.kind()).to_owned(),
            identity.idempotency_key().as_str().to_owned(),
            identity.fingerprint().content_hash().as_bytes().to_vec(),
        );
        if persisted != Some(expected) {
            return Err(application_error(
                ApplicationErrorCategory::AlreadyExists,
                false,
            ));
        }
        transaction.commit().await.map_err(map_sqlx_error)
    }

    async fn append_version(
        &self,
        command: AppendDefinitionVersion,
    ) -> Result<DefinitionValue, ApplicationError> {
        let value = command.value();
        let tenant_id = value.owner().tenant_id().as_str();
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let outcome = lock_idempotency(
            &mut transaction,
            tenant_id,
            "definition:append:v1",
            command.idempotency_key().as_str(),
            command.fingerprint().content_hash().as_bytes(),
            value.identity(),
        )
        .await?;
        if outcome == IdempotencyOutcome::Replay {
            transaction.commit().await.map_err(map_sqlx_error)?;
            return Ok(value.clone());
        }
        let identity: Option<(String, String, i64)> = sqlx::query_as(
            "SELECT owner_id::text, kind, latest_version
             FROM core.definition_identities
             WHERE tenant_id = $1 AND definition_id = $2
             FOR UPDATE",
        )
        .bind(tenant_id)
        .bind(value.identity())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        let expected_latest = command
            .expected_latest_version()
            .map(|version| version_i64(version.get()))
            .transpose()?
            .unwrap_or(0);
        if identity
            != Some((
                value.owner().owner_id().as_str().to_owned(),
                definition_kind(value.kind()).to_owned(),
                expected_latest,
            ))
        {
            return Err(application_error(
                ApplicationErrorCategory::VersionConflict,
                true,
            ));
        }
        insert_definition(&mut transaction, value).await?;
        sqlx::query(
            "UPDATE core.definition_identities
             SET latest_version = $3
             WHERE tenant_id = $1 AND definition_id = $2 AND latest_version = $4",
        )
        .bind(tenant_id)
        .bind(value.identity())
        .bind(version_i64(value.version())?)
        .bind(expected_latest)
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value.clone())
    }

    async fn get_version(
        &self,
        scope: &AccessScope,
        definition_id: Ulid,
        version: Version,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        read_definition(self, scope, definition_id.as_str(), Some(version), None).await
    }

    async fn resolve_as_of(
        &self,
        scope: &AccessScope,
        definition_id: Ulid,
        instant: MarketTime,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        read_definition(self, scope, definition_id.as_str(), None, Some(&instant)).await
    }
}

// Keeping all definition variants together makes the table mapping auditable against the enum.
#[allow(clippy::too_many_lines)]
async fn insert_definition(
    transaction: &mut Transaction<'_, Postgres>,
    value: &DefinitionValue,
) -> Result<(), ApplicationError> {
    let tenant = value.owner().tenant_id().as_str();
    let owner = value.owner().owner_id().as_str();
    let payload = encode_definition(value);
    match value {
        DefinitionValue::Unit(unit) => {
            sqlx::query(
                "INSERT INTO market.units
                 (tenant_id, unit_id, version, owner_id, code, dimension, scale, precision, payload)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(tenant)
            .bind(unit.identity())
            .bind(version_i64(unit.version())?)
            .bind(owner)
            .bind(unit.code())
            .bind(unit.dimension())
            .bind(i32::try_from(unit.scale()).map_err(|_| invalid())?)
            .bind(i32::try_from(unit.precision()).map_err(|_| invalid())?)
            .bind(payload)
            .execute(&mut **transaction)
            .await
            .map_err(map_sqlx_error)?;
        }
        DefinitionValue::Calendar(calendar) => {
            sqlx::query(
                "INSERT INTO market.calendars
                 (tenant_id, calendar_id, version, owner_id, market, market_timezone,
                  effective_from, effective_to, payload)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(tenant)
            .bind(calendar.identity())
            .bind(version_i64(calendar.version())?)
            .bind(owner)
            .bind(calendar.market())
            .bind(calendar.market_timezone())
            .bind(calendar.effective().from().instant())
            .bind(calendar.effective().to().instant())
            .bind(payload)
            .execute(&mut **transaction)
            .await
            .map_err(map_sqlx_error)?;
        }
        DefinitionValue::MarketRulePack(rule_pack) => {
            sqlx::query(
                "INSERT INTO market.market_rule_packs
                 (tenant_id, rule_pack_id, version, owner_id, market, rule_type, source,
                  effective_from, effective_to, verification_status, content_hash, payload)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
            )
            .bind(tenant)
            .bind(rule_pack.identity())
            .bind(version_i64(rule_pack.version())?)
            .bind(owner)
            .bind(rule_pack.market())
            .bind(rule_pack.rule_type())
            .bind(rule_pack.source())
            .bind(rule_pack.effective().from().instant())
            .bind(rule_pack.effective().to().instant())
            .bind(verification_status(rule_pack.verification_status()))
            .bind(crate::s3::content_addressed::hash_hex(
                rule_pack.content_hash(),
            ))
            .bind(payload)
            .execute(&mut **transaction)
            .await
            .map_err(map_sqlx_error)?;
        }
        DefinitionValue::Instrument(definition) => {
            let instrument = definition.instrument();
            sqlx::query(
                "INSERT INTO market.instruments
                 (tenant_id, instrument_id, version, owner_id, kind, market, symbol,
                  currency_unit_id, currency_unit_version, calendar_id, calendar_version, payload)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
            )
            .bind(tenant)
            .bind(instrument.id().as_str())
            .bind(version_i64(definition.version())?)
            .bind(owner)
            .bind(instrument_kind(instrument.kind()))
            .bind(instrument.market())
            .bind(instrument.symbol())
            .bind(instrument.currency().unit_id().as_str())
            .bind(version_i64(instrument.currency().version().get())?)
            .bind(instrument.calendar().id().as_str())
            .bind(version_i64(instrument.calendar().version().get())?)
            .bind(&payload)
            .execute(&mut **transaction)
            .await
            .map_err(map_sqlx_error)?;
            match definition.subtype() {
                None => {}
                Some(InstrumentSubtype::Bond(bond)) => {
                    let tax_attributes = bond.tax_attributes();
                    let cumulative_issued_amount =
                        tax_attributes.map(|_| bond.cumulative_issued_amount());
                    let value_added_tax_status =
                        tax_attributes.map(|value| match value.value_added_tax_status() {
                            ficant_domain::market::ValueAddedTaxStatus::Exempt => "exempt",
                            ficant_domain::market::ValueAddedTaxStatus::Taxable => "taxable",
                        });
                    let income_tax_status =
                        tax_attributes.map(|value| match value.income_tax_status() {
                            ficant_domain::market::IncomeTaxStatus::Exempt => "exempt",
                            ficant_domain::market::IncomeTaxStatus::Taxable => "taxable",
                        });
                    let pricing = bond.pricing_terms();
                    let coupon_frequency = pricing.map(|value| match value.frequency() {
                        ficant_domain::market::BondCouponFrequency::Annual => "annual",
                        ficant_domain::market::BondCouponFrequency::Semiannual => "semiannual",
                    });
                    let day_count = pricing.map(|value| match value.day_count() {
                        ficant_domain::market::BondDayCountConvention::ActActBondIsma => {
                            "act_act_bond_isma"
                        }
                    });
                    let business_day = pricing.map(|value| match value.business_day() {
                        ficant_domain::market::BondBusinessDayConvention::Following => "following",
                    });
                    sqlx::query(
                        "INSERT INTO market.bonds
                         (tenant_id, instrument_id, version, issue_date, first_issue_date,
                          current_issue_date, maturity_date, cumulative_issued_coefficient,
                          cumulative_issued_scale, cumulative_issued_unit_id,
                          cumulative_issued_unit_version, value_added_tax_status,
                          income_tax_status, face_coefficient, face_scale, face_unit_id,
                          face_unit_version, coupon_coefficient, coupon_scale, coupon_unit_id,
                          coupon_unit_version, coupon_frequency, day_count_convention,
                          business_day_convention, payload)
                         VALUES ($1, $2, $3, $4, $5, $6, $7, $8::numeric, $9, $10, $11,
                                 $12, $13, $14::numeric, $15, $16, $17, $18::numeric, $19,
                                 $20, $21, $22, $23, $24, $25)",
                    )
                    .bind(tenant)
                    .bind(instrument.id().as_str())
                    .bind(version_i64(definition.version())?)
                    .bind(bond.first_issue_date())
                    .bind(tax_attributes.map(|_| bond.first_issue_date()))
                    .bind(tax_attributes.map(|_| bond.current_issue_date()))
                    .bind(bond.maturity_date())
                    .bind(
                        cumulative_issued_amount
                            .map(ficant_domain::primitives::DecimalValue::coefficient),
                    )
                    .bind(
                        cumulative_issued_amount
                            .map(|value| i32::try_from(value.scale()))
                            .transpose()
                            .map_err(|_| invalid())?,
                    )
                    .bind(cumulative_issued_amount.map(|value| value.unit().unit_id().as_str()))
                    .bind(
                        cumulative_issued_amount
                            .map(|value| version_i64(value.unit().version().get()))
                            .transpose()?,
                    )
                    .bind(value_added_tax_status)
                    .bind(income_tax_status)
                    .bind(bond.face_value().coefficient())
                    .bind(i32::try_from(bond.face_value().scale()).map_err(|_| invalid())?)
                    .bind(bond.face_value().unit().unit_id().as_str())
                    .bind(version_i64(bond.face_value().unit().version().get())?)
                    .bind(pricing.map(|value| value.coupon_rate().coefficient()))
                    .bind(
                        pricing
                            .map(|value| i32::try_from(value.coupon_rate().scale()))
                            .transpose()
                            .map_err(|_| invalid())?,
                    )
                    .bind(pricing.map(|value| value.coupon_rate().unit().unit_id().as_str()))
                    .bind(
                        pricing
                            .map(|value| version_i64(value.coupon_rate().unit().version().get()))
                            .transpose()?,
                    )
                    .bind(coupon_frequency)
                    .bind(day_count)
                    .bind(business_day)
                    .bind(&payload)
                    .execute(&mut **transaction)
                    .await
                    .map_err(map_sqlx_error)?;
                }
                Some(InstrumentSubtype::FuturesContract(future)) => {
                    sqlx::query(
                        "INSERT INTO market.futures_contracts
                         (tenant_id, instrument_id, version, last_trade_time, expiry_time,
                          settlement_time, multiplier_coefficient, multiplier_scale,
                          multiplier_unit_id, multiplier_unit_version, rule_pack_id,
                          rule_pack_version, payload)
                         VALUES ($1, $2, $3, $4, $5, $6, $7::numeric, $8, $9, $10, $11, $12, $13)",
                    )
                    .bind(tenant)
                    .bind(instrument.id().as_str())
                    .bind(version_i64(definition.version())?)
                    .bind(future.last_trade_time().instant())
                    .bind(future.expiry_time().instant())
                    .bind(future.settlement_time().instant())
                    .bind(future.multiplier().coefficient())
                    .bind(i32::try_from(future.multiplier().scale()).map_err(|_| invalid())?)
                    .bind(future.multiplier().unit().unit_id().as_str())
                    .bind(version_i64(future.multiplier().unit().version().get())?)
                    .bind(future.rule_pack().id().as_str())
                    .bind(version_i64(future.rule_pack().version().get())?)
                    .bind(&payload)
                    .execute(&mut **transaction)
                    .await
                    .map_err(map_sqlx_error)?;
                }
            }
        }
    }
    Ok(())
}

// The scoped query variants deliberately share one authorization and decoding boundary.
#[allow(clippy::too_many_lines)]
async fn read_definition(
    repository: &PostgresRepository,
    scope: &AccessScope,
    definition_id: &str,
    exact_version: Option<Version>,
    as_of: Option<&MarketTime>,
) -> Result<Option<DefinitionValue>, ApplicationError> {
    let owners = scope
        .allowed_owner_ids()
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect::<Vec<_>>();
    let identity: Option<(String, i64)> = sqlx::query_as(
        "SELECT kind, latest_version
         FROM core.definition_identities
         WHERE tenant_id = $1 AND definition_id = $2
           AND owner_id::text = ANY($3::text[])",
    )
    .bind(scope.tenant_id().as_str())
    .bind(definition_id)
    .bind(&owners)
    .fetch_optional(repository.pool())
    .await
    .map_err(map_sqlx_error)?;
    let Some((kind, latest)) = identity else {
        return Ok(None);
    };
    let exact = exact_version
        .map(|value| version_i64(value.get()))
        .transpose()?;
    let payload: Option<Vec<u8>> = match kind.as_str() {
        "UNIT" => sqlx::query_scalar(
            "SELECT payload FROM market.units
             WHERE tenant_id = $1 AND unit_id = $2 AND version = $3
               AND owner_id::text = ANY($4::text[])",
        )
        .bind(scope.tenant_id().as_str())
        .bind(definition_id)
        .bind(exact.unwrap_or(latest))
        .bind(&owners)
        .fetch_optional(repository.pool())
        .await
        .map_err(map_sqlx_error)?,
        "INSTRUMENT" => sqlx::query_scalar(
            "SELECT payload FROM market.instruments
             WHERE tenant_id = $1 AND instrument_id = $2 AND version = $3
               AND owner_id::text = ANY($4::text[])",
        )
        .bind(scope.tenant_id().as_str())
        .bind(definition_id)
        .bind(exact.unwrap_or(latest))
        .bind(&owners)
        .fetch_optional(repository.pool())
        .await
        .map_err(map_sqlx_error)?,
        "CALENDAR" if exact.is_none() && as_of.is_some() => sqlx::query_scalar(
            "SELECT payload FROM market.calendars
             WHERE tenant_id = $1 AND calendar_id = $2
               AND owner_id::text = ANY($3::text[])
               AND effective_from <= $4 AND $4 < effective_to
             ORDER BY version DESC LIMIT 1",
        )
        .bind(scope.tenant_id().as_str())
        .bind(definition_id)
        .bind(&owners)
        .bind(as_of.expect("guarded").instant())
        .fetch_optional(repository.pool())
        .await
        .map_err(map_sqlx_error)?,
        "CALENDAR" => sqlx::query_scalar(
            "SELECT payload FROM market.calendars
             WHERE tenant_id = $1 AND calendar_id = $2 AND version = $3
               AND owner_id::text = ANY($4::text[])",
        )
        .bind(scope.tenant_id().as_str())
        .bind(definition_id)
        .bind(exact.unwrap_or(latest))
        .bind(&owners)
        .fetch_optional(repository.pool())
        .await
        .map_err(map_sqlx_error)?,
        "MARKET_RULE_PACK" if exact.is_none() && as_of.is_some() => sqlx::query_scalar(
            "SELECT payload FROM market.market_rule_packs
             WHERE tenant_id = $1 AND rule_pack_id = $2
               AND owner_id::text = ANY($3::text[])
               AND effective_from <= $4 AND $4 < effective_to
             ORDER BY version DESC LIMIT 1",
        )
        .bind(scope.tenant_id().as_str())
        .bind(definition_id)
        .bind(&owners)
        .bind(as_of.expect("guarded").instant())
        .fetch_optional(repository.pool())
        .await
        .map_err(map_sqlx_error)?,
        "MARKET_RULE_PACK" => sqlx::query_scalar(
            "SELECT payload FROM market.market_rule_packs
             WHERE tenant_id = $1 AND rule_pack_id = $2 AND version = $3
               AND owner_id::text = ANY($4::text[])",
        )
        .bind(scope.tenant_id().as_str())
        .bind(definition_id)
        .bind(exact.unwrap_or(latest))
        .bind(&owners)
        .fetch_optional(repository.pool())
        .await
        .map_err(map_sqlx_error)?,
        _ => return Err(storage_corruption()),
    };
    payload.map(|bytes| decode_definition(&bytes)).transpose()
}

const fn definition_kind(value: DefinitionKind) -> &'static str {
    match value {
        DefinitionKind::Instrument => "INSTRUMENT",
        DefinitionKind::Calendar => "CALENDAR",
        DefinitionKind::Unit => "UNIT",
        DefinitionKind::MarketRulePack => "MARKET_RULE_PACK",
    }
}

const fn instrument_kind(value: ficant_domain::market::InstrumentKind) -> &'static str {
    match value {
        ficant_domain::market::InstrumentKind::Bond => "BOND",
        ficant_domain::market::InstrumentKind::Futures => "FUTURES",
        ficant_domain::market::InstrumentKind::Other => "OTHER",
    }
}

const fn verification_status(value: ficant_domain::market::VerificationStatus) -> &'static str {
    match value {
        ficant_domain::market::VerificationStatus::Unverified => "UNVERIFIED",
        ficant_domain::market::VerificationStatus::Verified => "VERIFIED",
        ficant_domain::market::VerificationStatus::Rejected => "REJECTED",
    }
}

fn version_i64(value: u64) -> Result<i64, ApplicationError> {
    i64::try_from(value).map_err(|_| invalid())
}

fn invalid() -> ApplicationError {
    application_error(ApplicationErrorCategory::ValidationFailed, false)
}

fn storage_corruption() -> ApplicationError {
    application_error(ApplicationErrorCategory::StorageUnavailable, false)
}
