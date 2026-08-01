use async_trait::async_trait;
use ficant_application::ports::{AccessScope, FactorTopologyRepository, IdempotencyKey};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::ContentAddressed;
use ficant_domain::primitives::{
    ContentHash, DecimalValue, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    CurveNodeDefinition, CurveNodeDefinitionInput, CurveNodeRef, CurveRebuildPolicy,
    FactorDefinition, FactorDefinitionInput, FactorTarget, FactorTargetBinding,
    InstrumentFactorTarget, SecondOrderPolicy, SensitivityConvention, SensitivityDirection,
};

use super::PostgresRepository;
use super::common::{IdempotencyOutcome, application_error, lock_idempotency, map_sqlx_error};

type FactorRow = (
    String,
    String,
    i64,
    String,
    i32,
    String,
    i64,
    String,
    String,
    String,
    String,
);
type BindingRow = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
);

#[async_trait]
impl FactorTopologyRepository for PostgresRepository {
    async fn register_factor_definition(
        &self,
        scope: &AccessScope,
        definition: FactorDefinition,
        key: IdempotencyKey,
    ) -> Result<FactorDefinition, ApplicationError> {
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let outcome = lock_idempotency(
            &mut transaction,
            scope.tenant_id().as_str(),
            "factor:definition:v1",
            key.as_str(),
            definition.content_hash().as_bytes(),
            scope.actor_id().as_str(),
        )
        .await?;
        if outcome == IdempotencyOutcome::Fresh {
            sqlx::query(
                "INSERT INTO research.factor_definitions
                 (factor_id, factor_unit_id, factor_unit_version, bump_coefficient, bump_scale,
                  bump_unit_id, bump_unit_version, direction, curve_rebuild, second_order, content_hash)
                 VALUES ($1, $2, $3, $4::numeric, $5, $6, $7, $8, $9, $10, $11)
                 ON CONFLICT DO NOTHING",
            )
            .bind(definition.factor_id())
            .bind(definition.factor_unit().unit_id().as_str())
            .bind(version_i64(definition.factor_unit().version())?)
            .bind(definition.convention().bump().coefficient())
            .bind(i32::try_from(definition.convention().bump().scale()).map_err(|_| invalid())?)
            .bind(definition.convention().bump().unit().unit_id().as_str())
            .bind(version_i64(definition.convention().bump().unit().version())?)
            .bind(direction_sql(definition.convention().direction()))
            .bind(curve_rebuild_sql(definition.convention().curve_rebuild()))
            .bind(second_order_sql(definition.convention().second_order()))
            .bind(hash_hex(definition.content_hash()))
            .execute(&mut *transaction)
            .await
            .map_err(map_sqlx_error)?;
        }
        let stored = read_factor(&mut *transaction, definition.factor_id())
            .await?
            .ok_or_else(already_exists)?;
        if stored != definition {
            return Err(already_exists());
        }
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(stored)
    }

    async fn register_curve_node_definition(
        &self,
        scope: &AccessScope,
        definition: CurveNodeDefinition,
        key: IdempotencyKey,
    ) -> Result<CurveNodeDefinition, ApplicationError> {
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let outcome = lock_idempotency(
            &mut transaction,
            scope.tenant_id().as_str(),
            "factor:curve-node:v1",
            key.as_str(),
            definition.content_hash().as_bytes(),
            scope.actor_id().as_str(),
        )
        .await?;
        if outcome == IdempotencyOutcome::Fresh {
            sqlx::query(
                "INSERT INTO research.curve_node_definitions
                 (curve_node_id, curve_family_id, tenor, factor_unit_id, factor_unit_version, content_hash)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT DO NOTHING",
            )
            .bind(definition.curve_node_id())
            .bind(definition.curve_family_id())
            .bind(definition.tenor())
            .bind(definition.factor_unit().unit_id().as_str())
            .bind(version_i64(definition.factor_unit().version())?)
            .bind(hash_hex(definition.content_hash()))
            .execute(&mut *transaction)
            .await
            .map_err(map_sqlx_error)?;
        }
        let stored = read_curve_node(&mut *transaction, definition.curve_node_id())
            .await?
            .ok_or_else(already_exists)?;
        if stored != definition {
            return Err(already_exists());
        }
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(stored)
    }

    async fn bind_factor_target(
        &self,
        scope: &AccessScope,
        binding: FactorTargetBinding,
        key: IdempotencyKey,
    ) -> Result<FactorTargetBinding, ApplicationError> {
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let outcome = lock_idempotency(
            &mut transaction,
            scope.tenant_id().as_str(),
            "factor:binding:v1",
            key.as_str(),
            binding.content_hash().as_bytes(),
            scope.actor_id().as_str(),
        )
        .await?;
        if outcome == IdempotencyOutcome::Fresh {
            match binding.target() {
                FactorTarget::Instrument(target) => {
                    sqlx::query(
                        "INSERT INTO research.factor_target_bindings
                         (factor_id, target_kind, target_tenant_id, target_owner_id,
                          target_instrument_id, target_instrument_version, content_hash)
                         VALUES ($1, 'INSTRUMENT', $2, $3, $4, $5, $6)
                         ON CONFLICT DO NOTHING",
                    )
                    .bind(binding.factor_id())
                    .bind(target.owner().tenant_id().as_str())
                    .bind(target.owner().owner_id().as_str())
                    .bind(target.instrument().id().as_str())
                    .bind(version_i64(target.instrument().version())?)
                    .bind(hash_hex(binding.content_hash()))
                    .execute(&mut *transaction)
                    .await
                    .map_err(map_sqlx_error)?;
                }
                FactorTarget::CurveNode(target) => {
                    sqlx::query(
                        "INSERT INTO research.factor_target_bindings
                         (factor_id, target_kind, target_curve_node_id, target_curve_node_hash, content_hash)
                         VALUES ($1, 'CURVE_NODE', $2, $3, $4)
                         ON CONFLICT DO NOTHING",
                    )
                    .bind(binding.factor_id())
                    .bind(target.curve_node_id())
                    .bind(hash_hex(target.content_hash()))
                    .bind(hash_hex(binding.content_hash()))
                    .execute(&mut *transaction)
                    .await
                    .map_err(map_sqlx_error)?;
                }
            }
        }
        let stored = read_binding(&mut *transaction, binding.factor_id(), binding.target()).await?;
        if stored.as_ref() != Some(&binding) {
            return Err(already_exists());
        }
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(binding)
    }

    async fn get_factor_definition(
        &self,
        factor_id: &str,
    ) -> Result<Option<FactorDefinition>, ApplicationError> {
        read_factor(self.pool(), factor_id).await
    }

    async fn get_factor_targets(
        &self,
        scope: &AccessScope,
        factor_id: &str,
    ) -> Result<Vec<FactorTargetBinding>, ApplicationError> {
        let rows: Vec<BindingRow> = sqlx::query_as(
            "SELECT target_kind, target_tenant_id::text, target_owner_id::text, target_instrument_id::text,
                    target_instrument_version, target_curve_node_id, target_curve_node_hash
             FROM research.factor_target_bindings
             WHERE factor_id = $1
             ORDER BY target_kind, target_tenant_id, target_owner_id, target_instrument_id,
                      target_instrument_version, target_curve_node_id, target_curve_node_hash",
        )
        .bind(factor_id)
        .fetch_all(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        let bindings = rows
            .into_iter()
            .map(|row| binding_from_row(factor_id, row))
            .collect::<Result<Vec<_>, _>>()?;
        for binding in &bindings {
            if let FactorTarget::Instrument(target) = binding.target() {
                scope.authorize(target.owner())?;
            }
        }
        Ok(bindings)
    }

    async fn get_target_factors(
        &self,
        scope: &AccessScope,
        target: &FactorTarget,
    ) -> Result<Vec<FactorDefinition>, ApplicationError> {
        let factor_ids: Vec<(String,)> = match target {
            FactorTarget::Instrument(value) => sqlx::query_as(
                "SELECT factor_id FROM research.factor_target_bindings
                 WHERE target_kind = 'INSTRUMENT' AND target_tenant_id = $1 AND target_owner_id = $2
                   AND target_instrument_id = $3 AND target_instrument_version = $4
                 ORDER BY factor_id",
            )
            .bind(scope.tenant_id().as_str())
            .bind(value.owner().owner_id().as_str())
            .bind(value.instrument().id().as_str())
            .bind(version_i64(value.instrument().version())?)
            .fetch_all(self.pool())
            .await
            .map_err(map_sqlx_error)?,
            FactorTarget::CurveNode(value) => sqlx::query_as(
                "SELECT factor_id FROM research.factor_target_bindings
                 WHERE target_kind = 'CURVE_NODE' AND target_curve_node_id = $1
                   AND target_curve_node_hash = $2 ORDER BY factor_id",
            )
            .bind(value.curve_node_id())
            .bind(hash_hex(value.content_hash()))
            .fetch_all(self.pool())
            .await
            .map_err(map_sqlx_error)?,
        };
        let mut values = Vec::with_capacity(factor_ids.len());
        for (factor_id,) in factor_ids {
            values.push(
                read_factor(self.pool(), &factor_id)
                    .await?
                    .ok_or_else(invalid)?,
            );
        }
        Ok(values)
    }

    async fn exact_target_exists(&self, target: &FactorTarget) -> Result<bool, ApplicationError> {
        match target {
            FactorTarget::Instrument(target) => sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM market.instruments i
                 WHERE i.tenant_id = $1 AND i.owner_id = $2 AND i.instrument_id = $3 AND i.version = $4
                 AND (EXISTS (SELECT 1 FROM market.bonds b WHERE b.tenant_id = i.tenant_id
                              AND b.instrument_id = i.instrument_id AND b.version = i.version)
                      OR EXISTS (SELECT 1 FROM market.futures_contracts f WHERE f.tenant_id = i.tenant_id
                                 AND f.instrument_id = i.instrument_id AND f.version = i.version))
             )",
        )
        .bind(target.owner().tenant_id().as_str())
        .bind(target.owner().owner_id().as_str())
        .bind(target.instrument().id().as_str())
        .bind(version_i64(target.instrument().version())?)
        .fetch_one(self.pool())
        .await
        .map_err(map_sqlx_error),
            FactorTarget::CurveNode(target) => sqlx::query_scalar(
                "SELECT EXISTS(
                     SELECT 1 FROM research.curve_node_definitions
                     WHERE curve_node_id = $1 AND content_hash = $2
                 )",
            )
            .bind(target.curve_node_id())
            .bind(hash_hex(target.content_hash()))
            .fetch_one(self.pool())
            .await
            .map_err(map_sqlx_error),
        }
    }
}

async fn read_factor<'e, E>(
    executor: E,
    factor_id: &str,
) -> Result<Option<FactorDefinition>, ApplicationError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row: Option<FactorRow> = sqlx::query_as(
        "SELECT factor_id, factor_unit_id::text, factor_unit_version, bump_coefficient::text, bump_scale,
                bump_unit_id::text, bump_unit_version, direction, curve_rebuild, second_order, content_hash
         FROM research.factor_definitions WHERE factor_id = $1",
    ).bind(factor_id).fetch_optional(executor).await.map_err(map_sqlx_error)?;
    row.map(factor_from_row).transpose()
}

async fn read_curve_node<'e, E>(
    executor: E,
    curve_node_id: &str,
) -> Result<Option<CurveNodeDefinition>, ApplicationError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row: Option<(String, String, String, String, i64, String)> = sqlx::query_as(
        "SELECT curve_node_id, curve_family_id, tenor, factor_unit_id::text, factor_unit_version, content_hash
         FROM research.curve_node_definitions WHERE curve_node_id = $1",
    ).bind(curve_node_id).fetch_optional(executor).await.map_err(map_sqlx_error)?;
    row.map(curve_node_from_row).transpose()
}

async fn read_binding<'e, E>(
    executor: E,
    factor_id: &str,
    target: &FactorTarget,
) -> Result<Option<FactorTargetBinding>, ApplicationError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row: Option<BindingRow> = match target {
        FactorTarget::Instrument(value) => sqlx::query_as("SELECT target_kind, target_tenant_id::text, target_owner_id::text, target_instrument_id::text, target_instrument_version, target_curve_node_id, target_curve_node_hash FROM research.factor_target_bindings WHERE factor_id=$1 AND target_kind='INSTRUMENT' AND target_tenant_id=$2 AND target_owner_id=$3 AND target_instrument_id=$4 AND target_instrument_version=$5")
            .bind(factor_id).bind(value.owner().tenant_id().as_str()).bind(value.owner().owner_id().as_str()).bind(value.instrument().id().as_str()).bind(version_i64(value.instrument().version())?).fetch_optional(executor).await.map_err(map_sqlx_error)?,
        FactorTarget::CurveNode(value) => sqlx::query_as("SELECT target_kind, target_tenant_id::text, target_owner_id::text, target_instrument_id::text, target_instrument_version, target_curve_node_id, target_curve_node_hash FROM research.factor_target_bindings WHERE factor_id=$1 AND target_kind='CURVE_NODE' AND target_curve_node_id=$2 AND target_curve_node_hash=$3")
            .bind(factor_id).bind(value.curve_node_id()).bind(hash_hex(value.content_hash())).fetch_optional(executor).await.map_err(map_sqlx_error)?,
    };
    row.map(|value| binding_from_row(factor_id, value))
        .transpose()
}

fn factor_from_row(row: FactorRow) -> Result<FactorDefinition, ApplicationError> {
    let (
        factor_id,
        unit_id,
        unit_version,
        coefficient,
        scale,
        bump_unit_id,
        bump_unit_version,
        direction,
        curve_rebuild,
        second_order,
        hash,
    ) = row;
    let factor_unit = unit_ref(&unit_id, unit_version)?;
    let bump = DecimalValue::new(
        coefficient,
        u32::try_from(scale).map_err(|_| invalid())?,
        unit_ref(&bump_unit_id, bump_unit_version)?,
    )
    .map_err(ficant_application::map_domain_error)?;
    let convention = SensitivityConvention::new(
        bump,
        direction_from_sql(&direction)?,
        curve_rebuild_from_sql(&curve_rebuild)?,
        second_order_from_sql(&second_order)?,
    )
    .map_err(ficant_application::map_domain_error)?;
    FactorDefinition::new(FactorDefinitionInput {
        factor_id,
        factor_unit,
        convention,
        content_hash: parse_hash(&hash)?,
    })
    .map_err(ficant_application::map_domain_error)
}

fn curve_node_from_row(
    row: (String, String, String, String, i64, String),
) -> Result<CurveNodeDefinition, ApplicationError> {
    let (curve_node_id, curve_family_id, tenor, unit_id, unit_version, hash) = row;
    CurveNodeDefinition::new(CurveNodeDefinitionInput {
        curve_node_id,
        curve_family_id,
        tenor,
        factor_unit: unit_ref(&unit_id, unit_version)?,
        content_hash: parse_hash(&hash)?,
    })
    .map_err(ficant_application::map_domain_error)
}

fn binding_from_row(
    factor_id: &str,
    row: BindingRow,
) -> Result<FactorTargetBinding, ApplicationError> {
    let (kind, tenant, owner, instrument, version, curve_node_id, curve_node_hash) = row;
    let target = match kind.as_str() {
        "INSTRUMENT" => FactorTarget::Instrument(InstrumentFactorTarget::new(
            OwnerRef::new(
                parse_id(&tenant.ok_or_else(invalid)?)?,
                parse_id(&owner.ok_or_else(invalid)?)?,
            ),
            VersionRef::new(
                parse_id(&instrument.ok_or_else(invalid)?)?,
                version_from_i64(version.ok_or_else(invalid)?)?,
            ),
        )),
        "CURVE_NODE" => FactorTarget::CurveNode(
            CurveNodeRef::new(
                curve_node_id.ok_or_else(invalid)?,
                parse_hash(&curve_node_hash.ok_or_else(invalid)?)?,
            )
            .map_err(ficant_application::map_domain_error)?,
        ),
        _ => return Err(invalid()),
    };
    FactorTargetBinding::new(factor_id, target).map_err(ficant_application::map_domain_error)
}

fn unit_ref(id: &str, version: i64) -> Result<UnitRef, ApplicationError> {
    Ok(UnitRef::new(parse_id(id)?, version_from_i64(version)?))
}
fn parse_id(value: &str) -> Result<Ulid, ApplicationError> {
    Ulid::new(value).map_err(ficant_application::map_domain_error)
}
fn version_from_i64(value: i64) -> Result<Version, ApplicationError> {
    Version::new(u64::try_from(value).map_err(|_| invalid())?)
        .map_err(ficant_application::map_domain_error)
}
fn version_i64(value: Version) -> Result<i64, ApplicationError> {
    i64::try_from(value.get()).map_err(|_| invalid())
}
fn invalid() -> ApplicationError {
    application_error(ApplicationErrorCategory::ValidationFailed, false)
}
fn already_exists() -> ApplicationError {
    application_error(ApplicationErrorCategory::AlreadyExists, false)
}

fn parse_hash(value: &str) -> Result<ContentHash, ApplicationError> {
    if value.len() != 64 {
        return Err(invalid());
    }
    let mut bytes = [0_u8; 32];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk).map_err(|_| invalid())?;
        bytes[index] = u8::from_str_radix(text, 16).map_err(|_| invalid())?;
    }
    ContentHash::from_bytes(&bytes).map_err(ficant_application::map_domain_error)
}

fn hash_hex(value: &ContentHash) -> String {
    value
        .as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            use std::fmt::Write as _;
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        })
}
const fn direction_sql(value: SensitivityDirection) -> &'static str {
    match value {
        SensitivityDirection::Central => "CENTRAL",
        SensitivityDirection::Up => "UP",
        SensitivityDirection::Down => "DOWN",
    }
}
fn direction_from_sql(value: &str) -> Result<SensitivityDirection, ApplicationError> {
    match value {
        "CENTRAL" => Ok(SensitivityDirection::Central),
        "UP" => Ok(SensitivityDirection::Up),
        "DOWN" => Ok(SensitivityDirection::Down),
        _ => Err(invalid()),
    }
}
const fn curve_rebuild_sql(value: CurveRebuildPolicy) -> &'static str {
    match value {
        CurveRebuildPolicy::Rebuild => "REBUILD",
        CurveRebuildPolicy::Hold => "HOLD",
    }
}
fn curve_rebuild_from_sql(value: &str) -> Result<CurveRebuildPolicy, ApplicationError> {
    match value {
        "REBUILD" => Ok(CurveRebuildPolicy::Rebuild),
        "HOLD" => Ok(CurveRebuildPolicy::Hold),
        _ => Err(invalid()),
    }
}
const fn second_order_sql(value: SecondOrderPolicy) -> &'static str {
    match value {
        SecondOrderPolicy::Include => "INCLUDE",
        SecondOrderPolicy::Exclude => "EXCLUDE",
    }
}
fn second_order_from_sql(value: &str) -> Result<SecondOrderPolicy, ApplicationError> {
    match value {
        "INCLUDE" => Ok(SecondOrderPolicy::Include),
        "EXCLUDE" => Ok(SecondOrderPolicy::Exclude),
        _ => Err(invalid()),
    }
}
