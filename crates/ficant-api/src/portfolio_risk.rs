use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, AuthorizedPrincipal, BondAnalyticsEngine, CanonicalSnapshotDecoder,
    CurvePointSetDecoder, CurveSnapshotMetadataRepository, DataSourceRepository,
    DefinitionRepository, DefinitionValue, FactorTopologyRepository, FuturesDeliveryEngine,
    FuturesDeliveryRuleParser, IntegrityEventSink, PositionSnapshotRepository,
    RequiredVerifiedBlobRead, SafeTraceContext, SnapshotValue,
    SnapshotVerifiedReadMetadataRepository, SubjectRepository, VerifiedBlobReader,
    VerifiedBlobRole, VerifiedReadResourceKind, YieldCurveEngine, stored_definition_content_hash,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, CalculateBondKeyRateDv01,
    CalculateBondKeyRateDv01Command, map_domain_error,
};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::market::v1 as market;
use ficant_contracts::ficant::research::v1 as pb;
use ficant_contracts::ficant::research::v1::portfolio_risk_service_server::PortfolioRiskService;
use ficant_domain::governance::PlatformRole;
use ficant_domain::market::{PriceSourceType, data_source_content_hash};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    CoverageDeclaration, FactorDv01, PortfolioKeyRateExposure, PositionKeyRateExposure,
    PriceSourceSummary,
};
use ficant_domain::{ContentAddressed, Lineaged};
use ficant_runtime::{
    FormalImplementationBinding, FormalInputBinding, FormalInputBindingInput, FormalInputKind,
    FormalInputReference, NamedContentRef,
};
use tonic::{Request, Response, Status};

use crate::core_error::CoreBusinessErrorMapper;
use crate::grpc_web::request_credential;
use crate::registry::PlatformPort;
use crate::{
    FormalOutputPublisher,
    formal_evidence::{
        FormalInputTimes, domain_separated_hash, exact_subject_binding, implementation_binding,
        message_parameters_hash,
    },
};

const ANALYZE_SCOPE: &str = "rates:analyze";

#[derive(Clone)]
pub struct PortfolioRiskGrpcService {
    identity: Arc<dyn PlatformPort>,
    positions: Arc<dyn PositionSnapshotRepository>,
    curves: Arc<dyn CurveSnapshotMetadataRepository>,
    definitions: Arc<dyn DefinitionRepository>,
    data_sources: Arc<dyn DataSourceRepository>,
    factors: Arc<dyn FactorTopologyRepository>,
    blobs: Arc<dyn VerifiedBlobReader>,
    integrity_events: Arc<dyn IntegrityEventSink>,
    decoder: Arc<dyn CurvePointSetDecoder>,
    curve_engine: Arc<dyn YieldCurveEngine>,
    bond_engine: Arc<dyn BondAnalyticsEngine>,
    futures_snapshot_metadata: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
    futures_snapshot_decoder: Arc<dyn CanonicalSnapshotDecoder>,
    futures_rule_parser: Arc<dyn FuturesDeliveryRuleParser>,
    futures_engine: Arc<dyn FuturesDeliveryEngine>,
    subjects: Option<Arc<dyn SubjectRepository>>,
    formal_outputs: Option<FormalOutputPublisher>,
    errors: CoreBusinessErrorMapper,
}

impl PortfolioRiskGrpcService {
    /// Composes the authenticated `PortfolioRisk` transport with exact application ports.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the trace-key contract is invalid.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        identity: Arc<dyn PlatformPort>,
        _access_scope: AccessScope,
        positions: Arc<dyn PositionSnapshotRepository>,
        curves: Arc<dyn CurveSnapshotMetadataRepository>,
        definitions: Arc<dyn DefinitionRepository>,
        data_sources: Arc<dyn DataSourceRepository>,
        factors: Arc<dyn FactorTopologyRepository>,
        blobs: Arc<dyn VerifiedBlobReader>,
        integrity_events: Arc<dyn IntegrityEventSink>,
        decoder: Arc<dyn CurvePointSetDecoder>,
        curve_engine: Arc<dyn YieldCurveEngine>,
        bond_engine: Arc<dyn BondAnalyticsEngine>,
        futures_snapshot_metadata: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
        futures_snapshot_decoder: Arc<dyn CanonicalSnapshotDecoder>,
        futures_rule_parser: Arc<dyn FuturesDeliveryRuleParser>,
        futures_engine: Arc<dyn FuturesDeliveryEngine>,
        trace_key: &[u8],
    ) -> Result<Self, &'static str> {
        Ok(Self {
            identity,
            positions,
            curves,
            definitions,
            data_sources,
            factors,
            blobs,
            integrity_events,
            decoder,
            curve_engine,
            bond_engine,
            futures_snapshot_metadata,
            futures_snapshot_decoder,
            futures_rule_parser,
            futures_engine,
            subjects: None,
            formal_outputs: None,
            errors: CoreBusinessErrorMapper::new(trace_key)?,
        })
    }

    /// Enables the mandatory R7B formal-output boundary for portfolio KRD results.
    #[must_use]
    pub fn with_formal_outputs(
        mut self,
        subjects: Arc<dyn SubjectRepository>,
        formal_outputs: FormalOutputPublisher,
    ) -> Self {
        self.subjects = Some(subjects);
        self.formal_outputs = Some(formal_outputs);
        self
    }

    fn authorize(
        &self,
        request: &Request<impl Sized>,
    ) -> Result<AuthorizedPrincipal, ApplicationError> {
        let credential = request_credential(request.metadata());
        let session = self
            .identity
            .current_session(&credential)
            .map_err(|_| forbidden())?;
        let principal = session.authorized_principal()?;
        require_portfolio_risk_access(principal)
    }

    // The complete KRD proof is assembled in one fail-closed boundary to keep evidence exact.
    #[allow(clippy::too_many_lines)]
    async fn formal_portfolio(
        &self,
        principal: &AuthorizedPrincipal,
        request: &pb::CalculateKeyRateDv01Request,
        value: &PortfolioKeyRateExposure,
    ) -> Result<pb::PortfolioKeyRateExposure, ApplicationError> {
        let subjects = self.subjects.as_deref().ok_or_else(configuration)?;
        let publisher = self.formal_outputs.as_ref().ok_or_else(configuration)?;
        let knowledge_at = parse_market_time(request.knowledge_at.as_ref())?;
        let position_snapshot = self
            .positions
            .get_position_snapshot(
                principal.access_scope(),
                value.position_snapshot_id().clone(),
                knowledge_at,
            )
            .await?
            .ok_or_else(not_found)?;
        principal
            .access_scope()
            .authorize(position_snapshot.owner())?;
        if position_snapshot.id() != value.position_snapshot_id() {
            return Err(lineage_incomplete());
        }
        let subject = exact_subject_binding(
            subjects,
            principal.access_scope(),
            position_snapshot.owner(),
            position_snapshot.subject_ref(),
        )
        .await?;

        let mut candidates = BTreeMap::new();
        insert_candidate(
            &mut candidates,
            PortfolioInputCandidate::object(
                "position-snapshot",
                FormalInputKind::PositionSnapshot,
                position_snapshot.owner().clone(),
                LineageRef::new(
                    position_snapshot.id().clone(),
                    None,
                    Some(position_snapshot.content_hash().clone()),
                )
                .map_err(map_domain_error)?,
                FormalInputTimes {
                    observed_at: Some(position_snapshot.observed_at().clone()),
                    visible_at: Some(position_snapshot.visible_at().clone()),
                    ..FormalInputTimes::default()
                },
            ),
        )?;

        let curve_metadata = self
            .curves
            .get_curve_snapshot_metadata(
                principal.access_scope(),
                value.curve_snapshot_id().clone(),
            )
            .await?
            .ok_or_else(not_found)?;
        let curve = curve_metadata.snapshot();
        if curve.id() != value.curve_snapshot_id() || curve.owner() != position_snapshot.owner() {
            return Err(lineage_incomplete());
        }
        insert_candidate(
            &mut candidates,
            PortfolioInputCandidate::object(
                "curve-snapshot",
                FormalInputKind::CurveSnapshot,
                curve.owner().clone(),
                LineageRef::new(curve.id().clone(), None, Some(curve.content_hash().clone()))
                    .map_err(map_domain_error)?,
                FormalInputTimes {
                    observed_at: Some(curve.as_of().clone()),
                    visible_at: curve.visible_at().cloned(),
                    ..FormalInputTimes::default()
                },
            ),
        )?;

        let mut futures_snapshot = None;
        if let Some(data_snapshot_id) = value.futures_data_snapshot_id() {
            let metadata = self
                .futures_snapshot_metadata
                .get_verified_read_metadata(principal.access_scope(), data_snapshot_id.clone())
                .await?
                .ok_or_else(not_found)?;
            let SnapshotValue::Data(snapshot) = metadata.snapshot() else {
                return Err(lineage_incomplete());
            };
            if snapshot.owner() != position_snapshot.owner() || snapshot.id() != data_snapshot_id {
                return Err(lineage_incomplete());
            }
            insert_candidate(
                &mut candidates,
                PortfolioInputCandidate::object(
                    "data-snapshot",
                    FormalInputKind::DataSnapshot,
                    snapshot.owner().clone(),
                    LineageRef::new(
                        snapshot.id().clone(),
                        None,
                        Some(snapshot.content_hash().clone()),
                    )
                    .map_err(map_domain_error)?,
                    FormalInputTimes {
                        observed_at: Some(snapshot.as_of().clone()),
                        visible_at: Some(snapshot.visible_at().clone()),
                        ..FormalInputTimes::default()
                    },
                ),
            )?;
            futures_snapshot = Some(snapshot);
        }

        let mut lineage = value.lineage().to_vec();
        for position in value.positions() {
            lineage.extend_from_slice(position.lineage());
        }
        lineage.sort_by(compare_lineage_refs);
        lineage.dedup();
        for reference in &lineage {
            self.resolve_lineage_candidate(
                principal.access_scope(),
                position_snapshot.owner(),
                &position_snapshot,
                curve,
                futures_snapshot.as_ref(),
                reference,
                &mut candidates,
            )
            .await?;
        }

        let curve_read = RequiredVerifiedBlobRead::new(
            principal.access_scope().clone(),
            curve.owner().clone(),
            VerifiedReadResourceKind::CurveSnapshot,
            curve.id().clone(),
            VerifiedBlobRole::CurvePoints,
            curve.content_hash().clone(),
            curve_metadata.blob_size(),
            formal_trace(request)?,
        )?;
        let curve_payload = self
            .blobs
            .read_required(&curve_read, self.integrity_events.as_ref())
            .await?;
        let points = self.decoder.decode_canonical(curve_payload.bytes())?;
        if Some(points.curve_family_id()) != curve.curve_family_id() {
            return Err(lineage_incomplete());
        }
        for point in points.points() {
            let curve_node = self
                .factors
                .get_curve_node_definition(point.curve_node_id())
                .await?
                .ok_or_else(not_found)?;
            if curve_node.content_hash() != point.curve_node_content_hash() {
                return Err(lineage_incomplete());
            }
            insert_candidate(
                &mut candidates,
                PortfolioInputCandidate::named(
                    "curve-node-definition",
                    FormalInputKind::CurveNodeDefinition,
                    position_snapshot.owner().clone(),
                    curve_node.curve_node_id(),
                    curve_node.content_hash().clone(),
                ),
            )?;
        }

        let mut topology_hashes = BTreeSet::new();
        for position in value.positions() {
            topology_hashes.extend(position.input_evidence_hashes().iter().cloned());
        }
        for factor in value.totals() {
            let definition = self
                .factors
                .get_factor_definition(factor.factor_id())
                .await?
                .ok_or_else(not_found)?;
            if definition.content_hash() != factor.factor_definition_hash() {
                return Err(lineage_incomplete());
            }
            insert_candidate(
                &mut candidates,
                PortfolioInputCandidate::named(
                    "factor-definition",
                    FormalInputKind::FactorDefinition,
                    position_snapshot.owner().clone(),
                    definition.factor_id(),
                    definition.content_hash().clone(),
                ),
            )?;
        }

        let consumed = candidates_into_bindings(candidates)?;
        let algorithm_version = value.algorithm().algorithm_version().to_be_bytes();
        let mut implementations = vec![implementation_binding(
            "portfolio-krd",
            "ficant/portfolio-krd/implementation/v1",
            &[
                value.algorithm().algorithm_id().as_bytes(),
                &algorithm_version,
                value.algorithm().convention_profile().as_bytes(),
            ],
        )?];
        let topology_parts = topology_hashes
            .iter()
            .map(|hash| hash.as_bytes().as_slice())
            .collect::<Vec<_>>();
        implementations.push(
            FormalImplementationBinding::new(
                "factor-topology",
                domain_separated_hash("ficant/portfolio-krd/factor-topology/v1", &topology_parts),
            )
            .map_err(map_domain_error)?,
        );

        let mut result = portfolio(value);
        let evidence = publisher
            .publish_message(
                principal.access_scope(),
                position_snapshot.owner(),
                "ficant.research.v1.PortfolioKeyRateExposure",
                subject,
                consumed,
                implementations,
                message_parameters_hash("ficant/portfolio-krd/parameters/v1", request),
                None,
                &result,
            )
            .await?;
        result.formal_evidence = Some(evidence);
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    async fn resolve_lineage_candidate(
        &self,
        scope: &AccessScope,
        owner: &OwnerRef,
        position_snapshot: &ficant_domain::research::PositionSnapshot,
        curve: &ficant_domain::market::CurveSnapshot,
        futures_snapshot: Option<&ficant_domain::research::DataSnapshot>,
        reference: &LineageRef,
        candidates: &mut BTreeMap<String, PortfolioInputCandidate>,
    ) -> Result<(), ApplicationError> {
        if reference.object_id() == position_snapshot.id() {
            return require_exact_lineage(reference, None, position_snapshot.content_hash());
        }
        if reference.object_id() == curve.id() {
            return require_exact_lineage(reference, None, curve.content_hash());
        }
        if let Some(snapshot) = futures_snapshot
            && reference.object_id() == snapshot.id()
        {
            return require_exact_lineage(reference, None, snapshot.content_hash());
        }
        if reference.object_id() == position_snapshot.subject_ref().id()
            && reference.version() == Some(position_snapshot.subject_ref().version())
        {
            return Ok(());
        }

        let version = reference.version().ok_or_else(lineage_incomplete)?;
        let claimed_hash = reference.content_hash();
        if let Some(definition) = self
            .definitions
            .get_version(scope, reference.object_id().clone(), version)
            .await?
        {
            let actual_hash = stored_definition_content_hash(&definition);
            let claimed_hash_matches = claimed_hash.is_none_or(|hash| {
                hash == &actual_hash
                    || matches!(
                        &definition,
                        DefinitionValue::MarketRulePack(value) if hash == value.content_hash()
                    )
            });
            if definition.identity() != reference.object_id().as_str()
                || definition.version() != version.get()
                || definition.owner() != owner
                || !claimed_hash_matches
            {
                return Err(lineage_incomplete());
            }
            insert_candidate(
                candidates,
                definition_candidate(
                    definition,
                    LineageRef::new(
                        reference.object_id().clone(),
                        Some(version),
                        Some(actual_hash),
                    )
                    .map_err(map_domain_error)?,
                ),
            )?;
            return Ok(());
        }

        let source_ref = VersionRef::new(reference.object_id().clone(), version);
        if let Some(source) = self.data_sources.get_exact(scope, source_ref).await? {
            let actual_hash = data_source_content_hash(&source);
            if source.owner() != owner || claimed_hash.is_some_and(|hash| hash != &actual_hash) {
                return Err(lineage_incomplete());
            }
            insert_candidate(
                candidates,
                PortfolioInputCandidate::object(
                    "data-source",
                    FormalInputKind::DataSource,
                    owner.clone(),
                    LineageRef::new(
                        reference.object_id().clone(),
                        Some(version),
                        Some(actual_hash),
                    )
                    .map_err(map_domain_error)?,
                    FormalInputTimes::default(),
                ),
            )?;
            return Ok(());
        }
        Err(lineage_incomplete())
    }
}

#[derive(Clone, Debug)]
struct PortfolioInputCandidate {
    base_role: &'static str,
    kind: FormalInputKind,
    owner: OwnerRef,
    reference: FormalInputReference,
    times: FormalInputTimes,
}

impl PortfolioInputCandidate {
    fn object(
        base_role: &'static str,
        kind: FormalInputKind,
        owner: OwnerRef,
        reference: LineageRef,
        times: FormalInputTimes,
    ) -> Self {
        Self {
            base_role,
            kind,
            owner,
            reference: FormalInputReference::Object(reference),
            times,
        }
    }

    fn named(
        base_role: &'static str,
        kind: FormalInputKind,
        owner: OwnerRef,
        identity: impl Into<String>,
        content_hash: ContentHash,
    ) -> Self {
        Self {
            base_role,
            kind,
            owner,
            reference: FormalInputReference::Named(
                NamedContentRef::new(identity, content_hash)
                    .expect("verified factor identities satisfy the formal contract"),
            ),
            times: FormalInputTimes::default(),
        }
    }

    fn key(&self) -> String {
        let mut result = format!("{}:{:?}:", self.base_role, self.kind);
        match &self.reference {
            FormalInputReference::Object(reference) => {
                result.push_str(reference.object_id().as_str());
                result.push(':');
                if let Some(version) = reference.version() {
                    result.push_str(&version.get().to_string());
                }
                result.push(':');
                if let Some(hash) = reference.content_hash() {
                    result.push_str(&hex_hash(hash));
                }
            }
            FormalInputReference::Named(reference) => {
                result.push_str(reference.identity());
                result.push(':');
                result.push_str(&hex_hash(reference.content_hash()));
            }
        }
        result
    }
}

fn insert_candidate(
    candidates: &mut BTreeMap<String, PortfolioInputCandidate>,
    candidate: PortfolioInputCandidate,
) -> Result<(), ApplicationError> {
    let key = candidate.key();
    if let Some(existing) = candidates.get(&key) {
        if existing.kind != candidate.kind
            || existing.owner != candidate.owner
            || existing.reference != candidate.reference
            || existing.times.observed_at != candidate.times.observed_at
            || existing.times.visible_at != candidate.times.visible_at
            || existing.times.effective_from != candidate.times.effective_from
            || existing.times.effective_to != candidate.times.effective_to
        {
            return Err(lineage_incomplete());
        }
        return Ok(());
    }
    candidates.insert(key, candidate);
    Ok(())
}

fn candidates_into_bindings(
    candidates: BTreeMap<String, PortfolioInputCandidate>,
) -> Result<Vec<FormalInputBinding>, ApplicationError> {
    let mut counts = BTreeMap::new();
    for candidate in candidates.values() {
        *counts.entry(candidate.base_role).or_insert(0_usize) += 1;
    }
    let mut ordinals = BTreeMap::new();
    candidates
        .into_values()
        .map(|candidate| {
            let ordinal = ordinals.entry(candidate.base_role).or_insert(0_usize);
            *ordinal += 1;
            let role = if counts[candidate.base_role] == 1 {
                candidate.base_role.to_owned()
            } else {
                format!("{}.{:03}", candidate.base_role, *ordinal)
            };
            FormalInputBinding::new(FormalInputBindingInput {
                role,
                kind: candidate.kind,
                owner: candidate.owner,
                reference: candidate.reference,
                observed_at: candidate.times.observed_at,
                visible_at: candidate.times.visible_at,
                effective_from: candidate.times.effective_from,
                effective_to: candidate.times.effective_to,
            })
            .map_err(map_domain_error)
        })
        .collect()
}

fn definition_candidate(
    definition: DefinitionValue,
    reference: LineageRef,
) -> PortfolioInputCandidate {
    let owner = definition.owner().clone();
    match definition {
        DefinitionValue::Instrument(_) => PortfolioInputCandidate::object(
            "instrument",
            FormalInputKind::Instrument,
            owner,
            reference,
            FormalInputTimes::default(),
        ),
        DefinitionValue::Calendar(value) => PortfolioInputCandidate::object(
            "calendar",
            FormalInputKind::Calendar,
            owner,
            reference,
            FormalInputTimes {
                effective_from: Some(value.effective().from().clone()),
                effective_to: Some(value.effective().to().clone()),
                ..FormalInputTimes::default()
            },
        ),
        DefinitionValue::Unit(_) => PortfolioInputCandidate::object(
            "unit",
            FormalInputKind::Unit,
            owner,
            reference,
            FormalInputTimes::default(),
        ),
        DefinitionValue::MarketRulePack(value) => PortfolioInputCandidate::object(
            "rule-pack",
            FormalInputKind::RulePack,
            owner,
            reference,
            FormalInputTimes {
                effective_from: Some(value.effective().from().clone()),
                effective_to: Some(value.effective().to().clone()),
                ..FormalInputTimes::default()
            },
        ),
    }
}

fn compare_lineage_refs(left: &LineageRef, right: &LineageRef) -> std::cmp::Ordering {
    left.object_id()
        .cmp(right.object_id())
        .then_with(|| left.version().cmp(&right.version()))
        .then_with(|| left.content_hash().cmp(&right.content_hash()))
}

fn require_exact_lineage(
    reference: &LineageRef,
    version: Option<Version>,
    content_hash: &ContentHash,
) -> Result<(), ApplicationError> {
    if reference.version() != version || reference.content_hash() != Some(content_hash) {
        return Err(lineage_incomplete());
    }
    Ok(())
}

fn hex_hash(value: &ContentHash) -> String {
    value
        .as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            use std::fmt::Write as _;
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        })
}

fn formal_trace(
    request: &pb::CalculateKeyRateDv01Request,
) -> Result<SafeTraceContext, ApplicationError> {
    let digest = message_parameters_hash("ficant/portfolio-krd/formal-trace/v1", request);
    SafeTraceContext::new(hex_hash(&digest)[..32].to_owned())
}

fn require_portfolio_risk_access(
    principal: AuthorizedPrincipal,
) -> Result<AuthorizedPrincipal, ApplicationError> {
    principal.require_role(PlatformRole::Researcher)?;
    principal
        .has_scope(ANALYZE_SCOPE)
        .then_some(principal)
        .ok_or_else(forbidden)
}

#[tonic::async_trait]
impl PortfolioRiskService for PortfolioRiskGrpcService {
    async fn calculate_key_rate_dv01(
        &self,
        request: Request<pb::CalculateKeyRateDv01Request>,
    ) -> Result<Response<pb::CalculateKeyRateDv01Response>, Status> {
        const OPERATION: &str = "portfolio-risk.calculate-key-rate-dv01";
        let result = match self.authorize(&request) {
            Err(error) => Err(error),
            Ok(principal) => match parse_command(request.get_ref()) {
                Err(error) => Err(error),
                Ok(command) => {
                    match CalculateBondKeyRateDv01::new_with_futures(
                        self.positions.as_ref(),
                        self.curves.as_ref(),
                        self.definitions.as_ref(),
                        self.factors.as_ref(),
                        self.blobs.as_ref(),
                        self.integrity_events.as_ref(),
                        self.decoder.as_ref(),
                        self.curve_engine.as_ref(),
                        self.bond_engine.as_ref(),
                        self.futures_snapshot_metadata.as_ref(),
                        self.futures_snapshot_decoder.as_ref(),
                        self.data_sources.as_ref(),
                        self.futures_rule_parser.as_ref(),
                        self.futures_engine.as_ref(),
                    )
                    .execute(principal.access_scope(), command)
                    .await
                    {
                        Ok(value) => {
                            self.formal_portfolio(&principal, request.get_ref(), &value)
                                .await
                        }
                        Err(error) => Err(error),
                    }
                }
            },
        };
        Ok(Response::new(pb::CalculateKeyRateDv01Response {
            result: Some(match result {
                Ok(value) => pb::calculate_key_rate_dv01_response::Result::Exposure(value),
                Err(error) => pb::calculate_key_rate_dv01_response::Result::Error(self.errors.map(
                    OPERATION,
                    "portfolio-risk-application",
                    &error,
                )),
            }),
        }))
    }
}

fn parse_command(
    value: &pb::CalculateKeyRateDv01Request,
) -> Result<CalculateBondKeyRateDv01Command, ApplicationError> {
    let position_snapshot_id = parse_ulid(value.position_snapshot_id.as_ref())?;
    let knowledge_at = parse_market_time(value.knowledge_at.as_ref())?;
    let valuation_at = parse_market_time(value.valuation_at.as_ref())?;
    let curve_snapshot_id = parse_ulid(value.curve_snapshot_id.as_ref())?;
    let dv01_unit = parse_unit(value.dv01_unit.as_ref())?;
    match value.futures_data_snapshot_id.as_ref() {
        Some(snapshot_id) => CalculateBondKeyRateDv01Command::new_with_futures_data_snapshot(
            position_snapshot_id,
            knowledge_at,
            valuation_at,
            curve_snapshot_id,
            dv01_unit,
            parse_ulid(Some(snapshot_id))?,
        ),
        None => CalculateBondKeyRateDv01Command::new(
            position_snapshot_id,
            knowledge_at,
            valuation_at,
            curve_snapshot_id,
            dv01_unit,
        ),
    }
}

fn portfolio(value: &PortfolioKeyRateExposure) -> pb::PortfolioKeyRateExposure {
    pb::PortfolioKeyRateExposure {
        position_snapshot_id: Some(ulid(value.position_snapshot_id())),
        curve_snapshot_id: Some(ulid(value.curve_snapshot_id())),
        positions: value.positions().iter().map(position).collect(),
        totals: value.totals().iter().map(factor).collect(),
        algorithm: Some(pb::RiskAlgorithmBinding {
            algorithm_id: value.algorithm().algorithm_id().to_owned(),
            algorithm_version: value.algorithm().algorithm_version(),
            convention_profile: value.algorithm().convention_profile().to_owned(),
        }),
        content_hash: Some(hash(value.content_hash())),
        lineage: value.lineage().iter().map(lineage).collect(),
        futures_data_snapshot_id: value.futures_data_snapshot_id().map(ulid),
        source_confidence: Some(source_confidence(value.source_confidence())),
        coverage: Some(coverage(value.coverage())),
        formal_evidence: None,
    }
}

fn coverage(value: &CoverageDeclaration) -> pb::CoverageDeclaration {
    pb::CoverageDeclaration {
        imported_position_count: value.imported_position_count(),
        participating_position_count: value.participating_position_count(),
        imported_gross_economic_value_by_unit: value
            .imported_gross_economic_value_by_unit()
            .iter()
            .map(decimal)
            .collect(),
        participating_gross_economic_value_by_unit: value
            .participating_gross_economic_value_by_unit()
            .iter()
            .map(decimal)
            .collect(),
        missing_critical_field_record_count: value.missing_critical_field_record_count(),
        source_confidence: value.source_confidence().map(source_confidence),
        distinct_external_data_source_version_count: value
            .distinct_external_data_source_version_count(),
    }
}

fn source_confidence(value: &PriceSourceSummary) -> pb::PriceSourceSummary {
    pb::PriceSourceSummary {
        counts: value
            .counts()
            .iter()
            .map(|count| pb::PriceSourceCount {
                source_type: price_source_type(count.source_type()) as i32,
                record_count: count.record_count(),
            })
            .collect(),
        mixed: value.mixed(),
    }
}

const fn price_source_type(value: PriceSourceType) -> market::PriceSourceType {
    match value {
        PriceSourceType::RealTrade => market::PriceSourceType::RealTrade,
        PriceSourceType::ActiveQuote => market::PriceSourceType::ActiveQuote,
        PriceSourceType::ModelValuation => market::PriceSourceType::ModelValuation,
        PriceSourceType::CurveInterpolation => market::PriceSourceType::CurveInterpolation,
    }
}

fn position(value: &PositionKeyRateExposure) -> pb::PositionKeyRateExposure {
    pb::PositionKeyRateExposure {
        position_id: Some(ulid(value.position_id())),
        instrument: Some(version_ref(value.instrument())),
        exposures: value.exposures().iter().map(factor).collect(),
        content_hash: Some(hash(value.content_hash())),
        lineage: value.lineage().iter().map(lineage).collect(),
    }
}

fn factor(value: &FactorDv01) -> pb::FactorDv01 {
    pb::FactorDv01 {
        factor_id: value.factor_id().to_owned(),
        factor_definition_hash: Some(hash(value.factor_definition_hash())),
        dv01: Some(core::DecimalValue {
            coefficient: value.value().scaled().to_string(),
            scale: 12,
            unit: Some(unit(value.unit())),
        }),
    }
}

fn lineage(value: &LineageRef) -> core::LineageRef {
    core::LineageRef {
        object_id: Some(ulid(value.object_id())),
        version: value.version().map_or(0, Version::get),
        content_hash: value.content_hash().map(hash),
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

fn parse_market_time(value: Option<&core::MarketTime>) -> Result<MarketTime, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let instant = value.instant.as_ref().ok_or_else(invalid)?;
    let nanos = u32::try_from(instant.nanos).map_err(|_| invalid())?;
    let instant = Utc
        .timestamp_opt(instant.seconds, nanos)
        .single()
        .ok_or_else(invalid)?;
    let local_date =
        NaiveDate::parse_from_str(&value.local_trading_date, "%Y-%m-%d").map_err(|_| invalid())?;
    MarketTime::new(instant, value.market_timezone.clone(), local_date).map_err(map_domain_error)
}

fn parse_unit(value: Option<&core::UnitRef>) -> Result<UnitRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(UnitRef::new(
        parse_ulid(value.unit_id.as_ref())?,
        Version::new(value.version).map_err(map_domain_error)?,
    ))
}

fn parse_ulid(value: Option<&core::Ulid>) -> Result<Ulid, ApplicationError> {
    Ulid::new(value.ok_or_else(invalid)?.value.clone()).map_err(map_domain_error)
}

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}

fn lineage_incomplete() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
}

fn configuration() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StateConflict, false)
}

fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_administrator_cannot_enter_portfolio_risk_even_with_analysis_scope() {
        let principal = AuthorizedPrincipal::new(
            "portfolio-risk-admin".to_owned(),
            Ulid::new("01J00000000000000000000001").unwrap(),
            Ulid::new("01J00000000000000000000002").unwrap(),
            vec![Ulid::new("01J00000000000000000000003").unwrap()],
            PlatformRole::PlatformAdmin,
            vec![ANALYZE_SCOPE.to_owned()],
            ContentHash::digest(b"credential-fingerprint"),
        )
        .unwrap();

        let error = require_portfolio_risk_access(principal)
            .expect_err("administrator role must fail before command parsing or engine calls");
        assert_eq!(error.category(), ApplicationErrorCategory::Forbidden);
    }
}
