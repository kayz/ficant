use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ficant_api::{
    FactorRegistryGrpcService, PlatformApplication, PlatformPort, SessionPolicy, SystemClock,
    TrustedIdentity,
};
use ficant_application::ports::{
    AccessScope, ApplicationResult, FactorTopologyRepository, IdempotencyKey,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::research::v1 as pb;
use ficant_contracts::ficant::research::v1::factor_registry_service_server::FactorRegistryService;
use ficant_domain::ContentAddressed;
use ficant_domain::governance::PlatformRole;
use ficant_domain::primitives::{ContentHash, DecimalValue, Ulid, UnitRef, Version};
use ficant_domain::research::{
    CurveNodeDefinition, CurveRebuildPolicy, FactorDefinition, FactorDefinitionInput, FactorTarget,
    FactorTargetBinding, SecondOrderPolicy, SensitivityConvention, SensitivityDirection,
};
use tonic::Request;

const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

#[derive(Default)]
struct Repository {
    factor: Mutex<Option<FactorDefinition>>,
}

#[async_trait]
impl FactorTopologyRepository for Repository {
    async fn register_factor_definition(
        &self,
        _: &AccessScope,
        value: FactorDefinition,
        _: IdempotencyKey,
    ) -> ApplicationResult<FactorDefinition> {
        let mut stored = self.factor.lock().unwrap();
        match stored.as_ref() {
            None => {
                *stored = Some(value.clone());
                Ok(value)
            }
            Some(existing) if existing == &value => Ok(value),
            Some(_) => Err(ApplicationError::new(
                ApplicationErrorCategory::AlreadyExists,
                false,
            )),
        }
    }
    async fn register_curve_node_definition(
        &self,
        _: &AccessScope,
        value: CurveNodeDefinition,
        _: IdempotencyKey,
    ) -> ApplicationResult<CurveNodeDefinition> {
        Ok(value)
    }
    async fn bind_factor_target(
        &self,
        _: &AccessScope,
        value: FactorTargetBinding,
        _: IdempotencyKey,
    ) -> ApplicationResult<FactorTargetBinding> {
        Ok(value)
    }
    async fn get_factor_definition(
        &self,
        factor_id: &str,
    ) -> ApplicationResult<Option<FactorDefinition>> {
        Ok(self
            .factor
            .lock()
            .unwrap()
            .clone()
            .filter(|value| value.factor_id() == factor_id))
    }
    async fn get_factor_targets(
        &self,
        _: &AccessScope,
        _: &str,
    ) -> ApplicationResult<Vec<FactorTargetBinding>> {
        Ok(vec![])
    }
    async fn get_target_factors(
        &self,
        _: &AccessScope,
        _: &FactorTarget,
    ) -> ApplicationResult<Vec<FactorDefinition>> {
        Ok(vec![])
    }
    async fn exact_target_exists(&self, _: &FactorTarget) -> ApplicationResult<bool> {
        Ok(true)
    }
}

#[tokio::test]
async fn factor_registry_enforces_scopes_hashes_and_global_immutability() {
    fn assert_service<T: FactorRegistryService>() {}
    assert_service::<FactorRegistryGrpcService>();

    let repository = Arc::new(Repository::default());
    let grpc = service(["factors:read", "factors:write"], repository.clone());
    let definition = factor("1");
    let response = grpc
        .register_factor_definition(Request::new(pb::RegisterFactorDefinitionRequest {
            idempotency_key: "factor:register:v1".to_owned(),
            definition: Some(proto_factor(&definition)),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        response.result,
        Some(pb::register_factor_definition_response::Result::Definition(
            _
        ))
    ));

    let read = grpc
        .get_factor_definition(Request::new(pb::GetFactorDefinitionRequest {
            factor_id: definition.factor_id().to_owned(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        read.result,
        Some(pb::get_factor_definition_response::Result::Definition(_))
    ));

    let conflict = grpc
        .register_factor_definition(Request::new(pb::RegisterFactorDefinitionRequest {
            idempotency_key: "factor:register:conflict:v1".to_owned(),
            definition: Some(proto_factor(&factor("2"))),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        conflict.result,
        Some(pb::register_factor_definition_response::Result::Error(_))
    ));
    assert_eq!(
        repository.factor.lock().unwrap().as_ref(),
        Some(&definition)
    );

    let denied_repository = Arc::new(Repository::default());
    let denied = service(["factors:read"], denied_repository.clone())
        .register_factor_definition(Request::new(pb::RegisterFactorDefinitionRequest {
            idempotency_key: "factor:denied:v1".to_owned(),
            definition: Some(proto_factor(&definition)),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        denied.result,
        Some(pb::register_factor_definition_response::Result::Error(_))
    ));
    assert!(denied_repository.factor.lock().unwrap().is_none());

    let denied_role_repository = Arc::new(Repository::default());
    let denied_role = service_with_role(
        ["factors:read", "factors:write"],
        PlatformRole::PlatformAdmin,
        denied_role_repository.clone(),
    )
    .register_factor_definition(Request::new(pb::RegisterFactorDefinitionRequest {
        idempotency_key: "factor:denied-role:v1".to_owned(),
        definition: Some(proto_factor(&definition)),
    }))
    .await
    .unwrap()
    .into_inner();
    let Some(pb::register_factor_definition_response::Result::Error(error)) = denied_role.result
    else {
        panic!("platform administrator must not reach researcher-only factor operations");
    };
    assert_eq!(error.code, core::ErrorCode::Forbidden as i32);
    assert!(denied_role_repository.factor.lock().unwrap().is_none());
}

fn service<const N: usize>(
    scopes: [&str; N],
    repository: Arc<Repository>,
) -> FactorRegistryGrpcService {
    service_with_role(scopes, PlatformRole::Researcher, repository)
}

fn service_with_role<const N: usize>(
    scopes: [&str; N],
    role: PlatformRole,
    repository: Arc<Repository>,
) -> FactorRegistryGrpcService {
    let identity =
        TrustedIdentity::implicit("factor-test", id('A'), id('T'), vec![id('P')], role, scopes)
            .unwrap();
    let application: Arc<dyn PlatformPort> = Arc::new(
        PlatformApplication::try_new(
            Arc::new(SystemClock),
            SessionPolicy::new(900, 60).unwrap(),
            KEY,
            vec![],
            Some(identity),
            vec![],
        )
        .unwrap(),
    );
    FactorRegistryGrpcService::new(
        application,
        AccessScope::new(id('T'), id('A'), vec![id('P')]).unwrap(),
        repository,
        KEY,
    )
    .unwrap()
}

fn factor(coefficient: &str) -> FactorDefinition {
    let unit = UnitRef::new(id('V'), Version::new(1).unwrap());
    let convention = SensitivityConvention::new(
        DecimalValue::new(coefficient, 0, unit.clone()).unwrap(),
        SensitivityDirection::Central,
        CurveRebuildPolicy::Rebuild,
        SecondOrderPolicy::Exclude,
    )
    .unwrap();
    let mut input = FactorDefinitionInput {
        factor_id: "cn.gov.yield.10y".to_owned(),
        factor_unit: unit,
        convention,
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = FactorDefinition::content_hash_for(&input);
    FactorDefinition::new(input).unwrap()
}

fn proto_factor(value: &FactorDefinition) -> pb::FactorDefinition {
    pb::FactorDefinition {
        factor_id: value.factor_id().to_owned(),
        factor_unit: Some(proto_unit(value.factor_unit())),
        sensitivity_convention: Some(pb::SensitivityConvention {
            bump: Some(core::DecimalValue {
                coefficient: value.convention().bump().coefficient().to_owned(),
                scale: value.convention().bump().scale(),
                unit: Some(proto_unit(value.convention().bump().unit())),
            }),
            direction: pb::SensitivityDirection::Central as i32,
            curve_rebuild: pb::CurveRebuildPolicy::Rebuild as i32,
            second_order: pb::SecondOrderPolicy::Exclude as i32,
        }),
        content_hash: Some(core::Sha256 {
            value: value.content_hash().as_bytes().to_vec(),
        }),
    }
}

fn proto_unit(value: &UnitRef) -> core::UnitRef {
    core::UnitRef {
        unit_id: Some(core::Ulid {
            value: value.unit_id().as_str().to_owned(),
        }),
        version: value.version().get(),
    }
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5F0{suffix}")).unwrap()
}
