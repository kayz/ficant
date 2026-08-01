use std::sync::Arc;

use ficant_application::ports::{AccessScope, FactorTopologyRepository, IdempotencyKey};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, FactorTopologyUseCase, map_domain_error,
};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::research::v1 as pb;
use ficant_contracts::ficant::research::v1::factor_registry_service_server::FactorRegistryService;
use ficant_domain::ContentAddressed;
use ficant_domain::primitives::{
    ContentHash, DecimalValue, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    CurveNodeDefinition, CurveNodeDefinitionInput, CurveNodeRef, CurveRebuildPolicy,
    FactorDefinition, FactorDefinitionInput, FactorTarget, FactorTargetBinding,
    InstrumentFactorTarget, SecondOrderPolicy, SensitivityConvention, SensitivityDirection,
};
use tonic::{Request, Response, Status};

use crate::core_error::CoreBusinessErrorMapper;
use crate::grpc_web::request_credential;
use crate::registry::PlatformPort;

const READ_SCOPE: &str = "factors:read";
const WRITE_SCOPE: &str = "factors:write";

#[derive(Clone)]
pub struct FactorRegistryGrpcService {
    identity: Arc<dyn PlatformPort>,
    access_scope: AccessScope,
    repository: Arc<dyn FactorTopologyRepository>,
    errors: CoreBusinessErrorMapper,
}

impl FactorRegistryGrpcService {
    /// Builds the authenticated Factor registry transport.
    ///
    /// # Errors
    ///
    /// Returns an error when the trace-signing key cannot initialize the
    /// shared business-error mapper.
    pub fn new(
        identity: Arc<dyn PlatformPort>,
        access_scope: AccessScope,
        repository: Arc<dyn FactorTopologyRepository>,
        trace_key: &[u8],
    ) -> Result<Self, &'static str> {
        Ok(Self {
            identity,
            access_scope,
            repository,
            errors: CoreBusinessErrorMapper::new(trace_key)?,
        })
    }

    fn authorize(
        &self,
        request: &Request<impl Sized>,
        required_scope: &str,
    ) -> Result<(), ApplicationError> {
        let credential = request_credential(request.metadata());
        let session = self
            .identity
            .current_session(&credential)
            .map_err(|_| forbidden())?;
        session
            .has_scope(required_scope)
            .then_some(())
            .ok_or_else(forbidden)
    }

    fn error(&self, operation: &str, error: &ApplicationError) -> core::ErrorDetail {
        self.errors
            .map(operation, "factor-registry-application", error)
    }
}

#[tonic::async_trait]
impl FactorRegistryService for FactorRegistryGrpcService {
    async fn register_factor_definition(
        &self,
        request: Request<pb::RegisterFactorDefinitionRequest>,
    ) -> Result<Response<pb::RegisterFactorDefinitionResponse>, Status> {
        const OPERATION: &str = "factors.register-definition";
        let result = match self.authorize(&request, WRITE_SCOPE) {
            Err(error) => Err(error),
            Ok(()) => match (
                parse_factor(request.get_ref().definition.as_ref()),
                IdempotencyKey::new(request.get_ref().idempotency_key.clone()),
            ) {
                (Ok(definition), Ok(key)) => {
                    FactorTopologyUseCase::new(self.repository.as_ref())
                        .register_factor_definition(&self.access_scope, definition, key)
                        .await
                }
                (Err(error), _) | (_, Err(error)) => Err(error),
            },
        };
        Ok(Response::new(pb::RegisterFactorDefinitionResponse {
            result: Some(match result {
                Ok(value) => {
                    pb::register_factor_definition_response::Result::Definition(factor(&value))
                }
                Err(error) => pb::register_factor_definition_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }

    async fn register_curve_node_definition(
        &self,
        request: Request<pb::RegisterCurveNodeDefinitionRequest>,
    ) -> Result<Response<pb::RegisterCurveNodeDefinitionResponse>, Status> {
        const OPERATION: &str = "factors.register-curve-node";
        let result = match self.authorize(&request, WRITE_SCOPE) {
            Err(error) => Err(error),
            Ok(()) => match (
                parse_curve_node(request.get_ref().definition.as_ref()),
                IdempotencyKey::new(request.get_ref().idempotency_key.clone()),
            ) {
                (Ok(definition), Ok(key)) => {
                    FactorTopologyUseCase::new(self.repository.as_ref())
                        .register_curve_node_definition(&self.access_scope, definition, key)
                        .await
                }
                (Err(error), _) | (_, Err(error)) => Err(error),
            },
        };
        Ok(Response::new(pb::RegisterCurveNodeDefinitionResponse {
            result: Some(match result {
                Ok(value) => pb::register_curve_node_definition_response::Result::Definition(
                    curve_node(&value),
                ),
                Err(error) => pb::register_curve_node_definition_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }

    async fn bind_factor_target(
        &self,
        request: Request<pb::BindFactorTargetRequest>,
    ) -> Result<Response<pb::BindFactorTargetResponse>, Status> {
        const OPERATION: &str = "factors.bind-target";
        let result = match self.authorize(&request, WRITE_SCOPE) {
            Err(error) => Err(error),
            Ok(()) => match (
                parse_binding(request.get_ref().binding.as_ref()),
                IdempotencyKey::new(request.get_ref().idempotency_key.clone()),
            ) {
                (Ok(binding), Ok(key)) => {
                    FactorTopologyUseCase::new(self.repository.as_ref())
                        .bind_factor_target(&self.access_scope, binding, key)
                        .await
                }
                (Err(error), _) | (_, Err(error)) => Err(error),
            },
        };
        Ok(Response::new(pb::BindFactorTargetResponse {
            result: Some(match result {
                Ok(value) => pb::bind_factor_target_response::Result::Binding(binding(&value)),
                Err(error) => {
                    pb::bind_factor_target_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }

    async fn get_factor_definition(
        &self,
        request: Request<pb::GetFactorDefinitionRequest>,
    ) -> Result<Response<pb::GetFactorDefinitionResponse>, Status> {
        const OPERATION: &str = "factors.get-definition";
        let result = match self.authorize(&request, READ_SCOPE) {
            Err(error) => Err(error),
            Ok(()) => {
                FactorTopologyUseCase::new(self.repository.as_ref())
                    .get_factor_definition(&request.get_ref().factor_id)
                    .await
            }
        };
        Ok(Response::new(pb::GetFactorDefinitionResponse {
            result: Some(match result {
                Ok(value) => pb::get_factor_definition_response::Result::Definition(factor(&value)),
                Err(error) => {
                    pb::get_factor_definition_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }

    async fn get_factor_targets(
        &self,
        request: Request<pb::GetFactorTargetsRequest>,
    ) -> Result<Response<pb::GetFactorTargetsResponse>, Status> {
        const OPERATION: &str = "factors.get-targets";
        let result = match self.authorize(&request, READ_SCOPE) {
            Err(error) => Err(error),
            Ok(()) => {
                FactorTopologyUseCase::new(self.repository.as_ref())
                    .get_factor_targets(&self.access_scope, &request.get_ref().factor_id)
                    .await
            }
        };
        Ok(Response::new(pb::GetFactorTargetsResponse {
            result: Some(match result {
                Ok(values) => {
                    pb::get_factor_targets_response::Result::Bindings(pb::FactorTargetBindings {
                        bindings: values.iter().map(binding).collect(),
                    })
                }
                Err(error) => {
                    pb::get_factor_targets_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }

    async fn get_target_factors(
        &self,
        request: Request<pb::GetTargetFactorsRequest>,
    ) -> Result<Response<pb::GetTargetFactorsResponse>, Status> {
        const OPERATION: &str = "factors.get-target-factors";
        let result = match self.authorize(&request, READ_SCOPE) {
            Err(error) => Err(error),
            Ok(()) => match parse_target(request.get_ref().target.as_ref()) {
                Ok(target) => {
                    FactorTopologyUseCase::new(self.repository.as_ref())
                        .get_target_factors(&self.access_scope, &target)
                        .await
                }
                Err(error) => Err(error),
            },
        };
        Ok(Response::new(pb::GetTargetFactorsResponse {
            result: Some(match result {
                Ok(values) => {
                    pb::get_target_factors_response::Result::Definitions(pb::FactorDefinitions {
                        definitions: values.iter().map(factor).collect(),
                    })
                }
                Err(error) => {
                    pb::get_target_factors_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }
}

fn parse_factor(
    value: Option<&pb::FactorDefinition>,
) -> Result<FactorDefinition, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let convention = value.sensitivity_convention.as_ref().ok_or_else(invalid)?;
    let direction =
        match pb::SensitivityDirection::try_from(convention.direction).map_err(|_| invalid())? {
            pb::SensitivityDirection::Central => SensitivityDirection::Central,
            pb::SensitivityDirection::Up => SensitivityDirection::Up,
            pb::SensitivityDirection::Down => SensitivityDirection::Down,
            pb::SensitivityDirection::Unspecified => return Err(invalid()),
        };
    let curve_rebuild =
        match pb::CurveRebuildPolicy::try_from(convention.curve_rebuild).map_err(|_| invalid())? {
            pb::CurveRebuildPolicy::Rebuild => CurveRebuildPolicy::Rebuild,
            pb::CurveRebuildPolicy::Hold => CurveRebuildPolicy::Hold,
            pb::CurveRebuildPolicy::Unspecified => return Err(invalid()),
        };
    let second_order =
        match pb::SecondOrderPolicy::try_from(convention.second_order).map_err(|_| invalid())? {
            pb::SecondOrderPolicy::Include => SecondOrderPolicy::Include,
            pb::SecondOrderPolicy::Exclude => SecondOrderPolicy::Exclude,
            pb::SecondOrderPolicy::Unspecified => return Err(invalid()),
        };
    let sensitivity = SensitivityConvention::new(
        parse_decimal(convention.bump.as_ref())?,
        direction,
        curve_rebuild,
        second_order,
    )
    .map_err(map_domain_error)?;
    FactorDefinition::new(FactorDefinitionInput {
        factor_id: value.factor_id.clone(),
        factor_unit: parse_unit(value.factor_unit.as_ref())?,
        convention: sensitivity,
        content_hash: parse_hash(value.content_hash.as_ref())?,
    })
    .map_err(map_domain_error)
}

fn parse_curve_node(
    value: Option<&pb::CurveNodeDefinition>,
) -> Result<CurveNodeDefinition, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    CurveNodeDefinition::new(CurveNodeDefinitionInput {
        curve_node_id: value.curve_node_id.clone(),
        curve_family_id: value.curve_family_id.clone(),
        tenor: value.tenor.clone(),
        factor_unit: parse_unit(value.factor_unit.as_ref())?,
        content_hash: parse_hash(value.content_hash.as_ref())?,
    })
    .map_err(map_domain_error)
}
fn parse_binding(
    value: Option<&pb::FactorTargetBinding>,
) -> Result<FactorTargetBinding, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let binding = FactorTargetBinding::new(
        value.factor_id.clone(),
        parse_target(value.target.as_ref())?,
    )
    .map_err(map_domain_error)?;
    if parse_hash(value.content_hash.as_ref())? != *binding.content_hash() {
        return Err(ApplicationError::new(
            ApplicationErrorCategory::HashMismatch,
            false,
        ));
    }
    Ok(binding)
}
fn parse_target(value: Option<&pb::FactorTargetRef>) -> Result<FactorTarget, ApplicationError> {
    match value
        .ok_or_else(invalid)?
        .target
        .as_ref()
        .ok_or_else(invalid)?
    {
        pb::factor_target_ref::Target::Instrument(value) => {
            Ok(FactorTarget::Instrument(InstrumentFactorTarget::new(
                parse_owner(value.owner.as_ref())?,
                parse_version_ref(value.instrument.as_ref())?,
            )))
        }
        pb::factor_target_ref::Target::CurveNode(value) => Ok(FactorTarget::CurveNode(
            CurveNodeRef::new(
                value.curve_node_id.clone(),
                parse_hash(value.content_hash.as_ref())?,
            )
            .map_err(map_domain_error)?,
        )),
    }
}
fn parse_owner(value: Option<&core::OwnerRef>) -> Result<OwnerRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(OwnerRef::new(
        parse_ulid(value.tenant_id.as_ref())?,
        parse_ulid(value.owner_id.as_ref())?,
    ))
}
fn parse_version_ref(value: Option<&core::VersionRef>) -> Result<VersionRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(VersionRef::new(
        parse_ulid(value.id.as_ref())?,
        Version::new(value.version).map_err(map_domain_error)?,
    ))
}
fn parse_unit(value: Option<&core::UnitRef>) -> Result<UnitRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(UnitRef::new(
        parse_ulid(value.unit_id.as_ref())?,
        Version::new(value.version).map_err(map_domain_error)?,
    ))
}
fn parse_decimal(value: Option<&core::DecimalValue>) -> Result<DecimalValue, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    DecimalValue::new(
        value.coefficient.clone(),
        value.scale,
        parse_unit(value.unit.as_ref())?,
    )
    .map_err(map_domain_error)
}
fn parse_hash(value: Option<&core::Sha256>) -> Result<ContentHash, ApplicationError> {
    ContentHash::from_bytes(&value.ok_or_else(invalid)?.value).map_err(map_domain_error)
}
fn parse_ulid(value: Option<&core::Ulid>) -> Result<Ulid, ApplicationError> {
    Ulid::new(value.ok_or_else(invalid)?.value.clone()).map_err(map_domain_error)
}
fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}

fn factor(value: &FactorDefinition) -> pb::FactorDefinition {
    pb::FactorDefinition {
        factor_id: value.factor_id().to_owned(),
        factor_unit: Some(unit(value.factor_unit())),
        sensitivity_convention: Some(pb::SensitivityConvention {
            bump: Some(decimal(value.convention().bump())),
            direction: direction(value.convention().direction()) as i32,
            curve_rebuild: curve_rebuild(value.convention().curve_rebuild()) as i32,
            second_order: second_order(value.convention().second_order()) as i32,
        }),
        content_hash: Some(hash(value.content_hash())),
    }
}
fn curve_node(value: &CurveNodeDefinition) -> pb::CurveNodeDefinition {
    pb::CurveNodeDefinition {
        curve_node_id: value.curve_node_id().to_owned(),
        curve_family_id: value.curve_family_id().to_owned(),
        tenor: value.tenor().to_owned(),
        factor_unit: Some(unit(value.factor_unit())),
        content_hash: Some(hash(value.content_hash())),
    }
}
fn binding(value: &FactorTargetBinding) -> pb::FactorTargetBinding {
    pb::FactorTargetBinding {
        factor_id: value.factor_id().to_owned(),
        target: Some(target(value.target())),
        content_hash: Some(hash(value.content_hash())),
    }
}
fn target(value: &FactorTarget) -> pb::FactorTargetRef {
    pb::FactorTargetRef {
        target: Some(match value {
            FactorTarget::Instrument(value) => {
                pb::factor_target_ref::Target::Instrument(pb::InstrumentFactorTarget {
                    owner: Some(owner(value.owner())),
                    instrument: Some(version_ref(value.instrument())),
                })
            }
            FactorTarget::CurveNode(value) => {
                pb::factor_target_ref::Target::CurveNode(pb::CurveNodeRef {
                    curve_node_id: value.curve_node_id().to_owned(),
                    content_hash: Some(hash(value.content_hash())),
                })
            }
        }),
    }
}
fn owner(value: &OwnerRef) -> core::OwnerRef {
    core::OwnerRef {
        tenant_id: Some(ulid(value.tenant_id())),
        owner_id: Some(ulid(value.owner_id())),
    }
}
fn version_ref(value: &VersionRef) -> core::VersionRef {
    core::VersionRef {
        id: Some(ulid(value.id())),
        version: value.version().get(),
    }
}
fn unit(value: &UnitRef) -> core::UnitRef {
    core::UnitRef {
        unit_id: Some(ulid(value.unit_id())),
        version: value.version().get(),
    }
}
fn decimal(value: &DecimalValue) -> core::DecimalValue {
    core::DecimalValue {
        coefficient: value.coefficient().to_owned(),
        scale: value.scale(),
        unit: Some(unit(value.unit())),
    }
}
fn hash(value: &ContentHash) -> core::Sha256 {
    core::Sha256 {
        value: value.as_bytes().to_vec(),
    }
}
fn ulid(value: &Ulid) -> core::Ulid {
    core::Ulid {
        value: value.as_str().to_owned(),
    }
}
const fn direction(value: SensitivityDirection) -> pb::SensitivityDirection {
    match value {
        SensitivityDirection::Central => pb::SensitivityDirection::Central,
        SensitivityDirection::Up => pb::SensitivityDirection::Up,
        SensitivityDirection::Down => pb::SensitivityDirection::Down,
    }
}
const fn curve_rebuild(value: CurveRebuildPolicy) -> pb::CurveRebuildPolicy {
    match value {
        CurveRebuildPolicy::Rebuild => pb::CurveRebuildPolicy::Rebuild,
        CurveRebuildPolicy::Hold => pb::CurveRebuildPolicy::Hold,
    }
}
const fn second_order(value: SecondOrderPolicy) -> pb::SecondOrderPolicy {
    match value {
        SecondOrderPolicy::Include => pb::SecondOrderPolicy::Include,
        SecondOrderPolicy::Exclude => pb::SecondOrderPolicy::Exclude,
    }
}
