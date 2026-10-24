//! Point-in-time bar replay building blocks (ADR-0024).
//!
//! This module sits beside `research` rather than inside it: the R7A gate freezes the existing
//! L0 primitives and L1 research sources, and phase-two objects are added as new modules.
//!
//! Everything here is pure and deterministic: no clock, no randomness, no I/O, ordered
//! collections only. The replay loop that schedules decisions lives in the application layer; it
//! can only hand a policy a [`PointInTimeView`], whose constructor refuses any bar that was not yet
//! visible at the decision time (I1, AC38).
//!
//! Execution with a participation cap lives in [`execution`]; the book, equity and the per-bar
//! P&L breakdown in [`ledger`] (ADR-0025; AC40, AC41).

mod calibration;
mod constraints;
mod execution;
mod ledger;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

pub use calibration::{
    CalibrationObjective, CalibrationRecord, CandidateResult, MAX_CALIBRATION_CANDIDATES,
    ParameterGrid, WindowResult, best_training_candidate, check_windows,
};
pub use constraints::{
    ConstraintAction, ConstraintHandling, ConstraintOutcome, ExecutionContext, ReplayConstraint,
    bind_constraints, check_execution,
};
pub use execution::{
    ExecutionRules, FillPriceBasis, NoFillReason, Remainder, RemainderHandling, RemainderOutcome,
    SimulatedFill, attempt_in_bar, no_later_bar,
};
pub use ledger::{
    BarPnl, BondCarry, COST_NONE, COST_RATE_PLUS_FIXED, CostModel, InstrumentPnl, ReplayFunding,
    ReplayLedger,
};

use crate::market::{Bar, BarFrequency, BarSettlement, PriceSourceType};
use crate::primitives::{
    ContentHash, EffectivePeriod, FixedDecimal, LineageRef, OwnerRef, Ulid, VersionRef,
};
use crate::subject::FundingTier;
use crate::{DomainErrorCode, DomainResult};

/// Built-in policy family: hold `target_face` of each universe instrument while its latest close
/// yield is at or above `entry_yield`, go flat at or below `exit_yield`, otherwise keep the
/// current position.
pub const YIELD_THRESHOLD_FAMILY: &str = "ficant.policy.yield-threshold";

/// An exact decimal amount (face value, price, yield) used by replay arithmetic.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReplayAmount(Decimal);

impl ReplayAmount {
    pub const ZERO: Self = Self(Decimal::ZERO);

    /// Parses plain decimal text such as `"100000000"` or `"1.85"`.
    pub fn parse(text: &str) -> DomainResult<Self> {
        text.parse::<Decimal>()
            .map(|value| Self(value.normalize()))
            .map_err(|_| DomainErrorCode::InvalidValue)
    }

    pub fn is_zero(self) -> bool {
        self.0.is_zero()
    }

    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        Self::from_decimal(self.0 + other.0)
    }

    #[must_use]
    pub fn minus(self, other: Self) -> Self {
        Self::from_decimal(self.0 - other.0)
    }

    #[must_use]
    pub fn abs(self) -> Self {
        Self(self.0.abs())
    }

    /// Canonical text: no trailing zeros, no exponent.
    pub fn canonical(self) -> String {
        self.0.normalize().to_string()
    }

    /// Exact conversion from the platform's fixed-scale decimal (e.g. a resolved funding rate).
    pub fn from_fixed(value: FixedDecimal) -> Self {
        Self::from_decimal(Decimal::from_i128_with_scale(
            value.scaled(),
            crate::primitives::DECIMAL_SCALE,
        ))
    }

    pub(crate) fn of(value: &crate::primitives::DecimalValue) -> Self {
        let coefficient = value
            .coefficient()
            .parse::<i128>()
            .expect("validated decimal coefficient fits in i128");
        Self(Decimal::from_i128_with_scale(coefficient, value.scale()).normalize())
    }

    pub(crate) fn from_decimal(value: Decimal) -> Self {
        Self(value.normalize())
    }

    pub(crate) const fn value(self) -> Decimal {
        self.0
    }
}

/// A calibrated (or to-be-calibrated) strategy as delivered by ficant (ADR-0014).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyArtifact {
    input: PolicyArtifactInput,
    content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyArtifactInput {
    pub policy_artifact_id: Ulid,
    pub owner: OwnerRef,
    pub family: String,
    pub family_version: u32,
    pub parameters: BTreeMap<String, ReplayAmount>,
    pub universe: Vec<VersionRef>,
    pub frequency: BarFrequency,
    pub settlement: BarSettlement,
    pub valid: EffectivePeriod,
    /// Participation cap and remainder handling for simulated execution (AC41).
    pub execution: ExecutionRules,
    /// How each constraint of the bound Subject's constraint set is enforced (AC42).
    pub constraints: BTreeMap<String, ConstraintHandling>,
    /// How these parameters were calibrated, for a calibrated artifact (AC43).
    pub calibration: Option<CalibrationRecord>,
}

impl PolicyArtifact {
    pub fn new(mut input: PolicyArtifactInput) -> DomainResult<Self> {
        input.universe.sort();
        input.universe.dedup();
        if input.family.trim().is_empty()
            || input.family != input.family.trim()
            || input.family_version == 0
            || input.universe.is_empty()
            || input
                .parameters
                .keys()
                .chain(input.constraints.keys())
                .any(|key| key.trim().is_empty() || key != key.trim())
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        // A calibrated artifact carries exactly the parameters of the selected candidate.
        if input
            .calibration
            .as_ref()
            .is_some_and(|record| record.selected_candidate().parameters != input.parameters)
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        let content_hash = ContentHash::digest(&policy_bytes(&input));
        Ok(Self {
            input,
            content_hash,
        })
    }

    pub fn id(&self) -> &Ulid {
        &self.input.policy_artifact_id
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.input.owner
    }

    pub fn family(&self) -> &str {
        &self.input.family
    }

    pub const fn family_version(&self) -> u32 {
        self.input.family_version
    }

    pub fn parameter(&self, name: &str) -> Option<ReplayAmount> {
        self.input.parameters.get(name).copied()
    }

    pub fn universe(&self) -> &[VersionRef] {
        &self.input.universe
    }

    pub const fn frequency(&self) -> BarFrequency {
        self.input.frequency
    }

    pub const fn settlement(&self) -> BarSettlement {
        self.input.settlement
    }

    pub fn valid(&self) -> &EffectivePeriod {
        &self.input.valid
    }

    pub const fn execution(&self) -> ExecutionRules {
        self.input.execution
    }

    pub fn constraint_handling(&self) -> &BTreeMap<String, ConstraintHandling> {
        &self.input.constraints
    }

    pub fn calibration(&self) -> Option<&CalibrationRecord> {
        self.input.calibration.as_ref()
    }

    /// The input this artifact was built from, for deriving a related artifact.
    pub fn input(&self) -> &PolicyArtifactInput {
        &self.input
    }

    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }

    /// Whether a bar belongs to this policy's universe, frequency and settlement type.
    pub fn covers(&self, bar: &Bar) -> bool {
        bar.frequency() == self.input.frequency
            && bar.settlement() == self.input.settlement
            && self.input.universe.binary_search(bar.instrument()).is_ok()
    }
}

/// Everything a policy may see at one decision time.
#[derive(Clone, Debug)]
pub struct PointInTimeView<'a> {
    decision_time: DateTime<Utc>,
    bars: Vec<&'a Bar>,
}

impl<'a> PointInTimeView<'a> {
    /// Builds a view; fails if any bar was not yet visible at `decision_time` (AC38).
    pub fn new(decision_time: DateTime<Utc>, bars: Vec<&'a Bar>) -> DomainResult<Self> {
        if bars.iter().any(|bar| bar.visible_at() > decision_time) {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        Ok(Self {
            decision_time,
            bars,
        })
    }

    pub const fn decision_time(&self) -> DateTime<Utc> {
        self.decision_time
    }

    pub fn bars(&self) -> &[&'a Bar] {
        &self.bars
    }

    /// The latest visible bar with a close yield for an instrument, frequency and settlement.
    pub fn latest_with_yield(
        &self,
        instrument: &VersionRef,
        frequency: BarFrequency,
        settlement: BarSettlement,
    ) -> Option<&'a Bar> {
        self.bars
            .iter()
            .copied()
            .filter(|bar| {
                bar.instrument() == instrument
                    && bar.frequency() == frequency
                    && bar.settlement() == settlement
                    && bar.close_yield().is_some()
            })
            .max_by_key(|bar| (bar.bar_end(), bar.source_record_id().to_owned()))
    }
}

/// Target face amount per instrument. A target is not an order (AC44).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TargetExposure {
    targets: BTreeMap<VersionRef, ReplayAmount>,
}

impl TargetExposure {
    pub fn set(&mut self, instrument: VersionRef, face: ReplayAmount) {
        self.targets.insert(instrument, face);
    }

    pub fn get(&self, instrument: &VersionRef) -> Option<ReplayAmount> {
        self.targets.get(instrument).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&VersionRef, &ReplayAmount)> {
        self.targets.iter()
    }
}

/// Simulated positions (face amount) and cash. Trades settle at dirty price; costs, funding and
/// coupons move cash too (ADR-0025).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SimulatedBook {
    positions: BTreeMap<VersionRef, ReplayAmount>,
    cash: ReplayAmount,
}

impl SimulatedBook {
    pub fn position(&self, instrument: &VersionRef) -> ReplayAmount {
        self.positions
            .get(instrument)
            .copied()
            .unwrap_or(ReplayAmount::ZERO)
    }

    pub const fn cash(&self) -> ReplayAmount {
        self.cash
    }

    pub fn positions(&self) -> impl Iterator<Item = (&VersionRef, &ReplayAmount)> {
        self.positions.iter()
    }

    /// The book as it will be once every pending trade executes, judged only by intent: whether a
    /// pending trade will actually fill is decided by a bar that is not visible yet, so it must
    /// not influence what the policy sees.
    #[must_use]
    pub fn with_pending<'a>(
        &self,
        intents: impl IntoIterator<Item = (&'a VersionRef, ReplayAmount)>,
    ) -> Self {
        let mut projected = self.clone();
        for (instrument, face) in intents {
            projected.add_position(instrument, face.0);
        }
        projected
    }

    /// Face amount to trade per instrument to move from the current book to `target`.
    pub fn deltas_toward(&self, target: &TargetExposure) -> Vec<(VersionRef, ReplayAmount)> {
        target
            .iter()
            .filter_map(|(instrument, face)| {
                let delta = face.0 - self.position(instrument).0;
                (!delta.is_zero()).then(|| (instrument.clone(), ReplayAmount(delta.normalize())))
            })
            .collect()
    }

    pub(crate) fn add_position(&mut self, instrument: &VersionRef, face: Decimal) {
        let position = self.position(instrument).0 + face;
        if position.is_zero() {
            self.positions.remove(instrument);
        } else {
            self.positions
                .insert(instrument.clone(), ReplayAmount(position.normalize()));
        }
    }

    pub(crate) fn add_cash(&mut self, amount: Decimal) {
        self.cash = ReplayAmount((self.cash.0 + amount).normalize());
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyError {
    /// The policy needs a parameter the artifact does not carry.
    MissingParameter(String),
    /// The evaluator does not implement this artifact's family or version.
    UnsupportedFamily,
}

/// The shared evaluator (ADR-0014): the same code runs in replay and in production.
pub trait PolicyEvaluator {
    fn family(&self) -> &str;
    fn family_version(&self) -> u32;
    fn decide(
        &self,
        policy: &PolicyArtifact,
        view: &PointInTimeView<'_>,
        book: &SimulatedBook,
    ) -> Result<TargetExposure, PolicyError>;
}

/// Implementation of [`YIELD_THRESHOLD_FAMILY`], version 1.
#[derive(Clone, Copy, Debug, Default)]
pub struct YieldThresholdPolicy;

impl PolicyEvaluator for YieldThresholdPolicy {
    fn family(&self) -> &str {
        YIELD_THRESHOLD_FAMILY
    }

    fn family_version(&self) -> u32 {
        1
    }

    fn decide(
        &self,
        policy: &PolicyArtifact,
        view: &PointInTimeView<'_>,
        book: &SimulatedBook,
    ) -> Result<TargetExposure, PolicyError> {
        if policy.family() != self.family() || policy.family_version() != self.family_version() {
            return Err(PolicyError::UnsupportedFamily);
        }
        let parameter = |name: &str| {
            policy
                .parameter(name)
                .ok_or_else(|| PolicyError::MissingParameter(name.to_owned()))
        };
        let entry = parameter("entry_yield")?;
        let exit = parameter("exit_yield")?;
        let target_face = parameter("target_face")?;
        let mut target = TargetExposure::default();
        for instrument in policy.universe() {
            let current = book.position(instrument);
            let Some(bar) =
                view.latest_with_yield(instrument, policy.frequency(), policy.settlement())
            else {
                target.set(instrument.clone(), current);
                continue;
            };
            let yield_now = bar
                .close_yield()
                .map_or(ReplayAmount::ZERO, ReplayAmount::of);
            let face = if yield_now >= entry {
                target_face
            } else if yield_now <= exit {
                ReplayAmount::ZERO
            } else {
                current
            };
            target.set(instrument.clone(), face);
        }
        Ok(target)
    }
}

/// One decision: when it was made, the newest visibility time among its inputs, and the target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayDecision {
    pub decision_time: DateTime<Utc>,
    pub inputs_visible_through: Option<DateTime<Utc>>,
    pub input_bar_count: usize,
    pub target: TargetExposure,
}

/// What an input object was to a replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LineageRole {
    Policy,
    DataSnapshot,
    Subject,
    SubjectState,
    ConstraintSet,
    FundingRulePack,
    CouponData,
}

/// One input of a replay result: its role and exact reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayLineage {
    pub role: LineageRole,
    pub reference: LineageRef,
}

/// Content hash of a Subject state snapshot: identity, Subject, net capital, ceilings and times.
pub fn subject_state_hash(state: &crate::subject::SubjectStateSnapshot) -> ContentHash {
    let mut bytes = Canonical::new("ficant-subject-state-snapshot/v1");
    bytes.text(state.id().as_str());
    bytes.version_ref(state.subject_ref());
    bytes.text(state.net_capital().coefficient());
    bytes.number(u64::from(state.net_capital().scale()));
    let mut ceilings: Vec<_> = state.limit_ceilings().iter().collect();
    ceilings.sort_by(|left, right| left.limit_code().cmp(right.limit_code()));
    bytes.number(ceilings.len() as u64);
    for ceiling in ceilings {
        bytes.text(ceiling.limit_code());
        bytes.text(ceiling.ceiling().coefficient());
        bytes.number(u64::from(ceiling.ceiling().scale()));
    }
    bytes.time(state.observed_at());
    bytes.time(state.visible_at());
    bytes.text(state.market_timezone());
    ContentHash::digest(&bytes.0)
}

/// Everything that identifies a replay's inputs (AC39). Replacing any field changes the result ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayIdentity {
    pub policy_id: Ulid,
    pub policy: ContentHash,
    pub data_snapshot: Ulid,
    pub data_snapshot_content: ContentHash,
    pub subject: VersionRef,
    /// The Subject state snapshot the constraint ceilings come from.
    pub subject_state: Ulid,
    /// Content hash of that snapshot ([`subject_state_hash`]).
    pub subject_state_content: ContentHash,
    /// The regulatory `RulePack` version of the Subject's constraint set, if it binds one.
    pub constraint_set: Option<VersionRef>,
    /// Every enforced constraint with its measure, ceiling and handling.
    pub constraints: Vec<ReplayConstraint>,
    pub funding: ReplayFunding,
    /// Version of the coupon reference data behind the carry source.
    pub carry_source: VersionRef,
    /// Content hash of the coupon data source.
    pub carry: ContentHash,
    pub cost_model: String,
    pub cost_parameters: BTreeMap<String, ReplayAmount>,
    pub seed: u64,
    pub code: ContentHash,
    pub runtime: ContentHash,
}

/// Outcome of one replay. Decisions, fills and P&L rows are in time order; the ID binds inputs and
/// outputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimulationResult {
    result_id: ContentHash,
    identity: ReplayIdentity,
    decisions: Vec<ReplayDecision>,
    fills: Vec<SimulatedFill>,
    pnl: Vec<BarPnl>,
    book: SimulatedBook,
}

impl SimulationResult {
    pub fn new(
        identity: ReplayIdentity,
        decisions: Vec<ReplayDecision>,
        fills: Vec<SimulatedFill>,
        pnl: Vec<BarPnl>,
        book: SimulatedBook,
    ) -> DomainResult<Self> {
        let ordered = decisions
            .windows(2)
            .all(|pair| pair[0].decision_time < pair[1].decision_time)
            && fills.windows(2).all(|pair| {
                (pair[0].effective_at, &pair[0].instrument)
                    <= (pair[1].effective_at, &pair[1].instrument)
            })
            && pnl.windows(2).all(|pair| pair[0].bar_end < pair[1].bar_end);
        let visible = decisions.iter().all(|decision| {
            decision
                .inputs_visible_through
                .is_none_or(|time| time <= decision.decision_time)
        });
        if !ordered || !visible {
            return Err(DomainErrorCode::InvalidValue);
        }
        let result_id =
            ContentHash::digest(&result_bytes(&identity, &decisions, &fills, &pnl, &book));
        Ok(Self {
            result_id,
            identity,
            decisions,
            fills,
            pnl,
            book,
        })
    }

    pub fn result_id(&self) -> &ContentHash {
        &self.result_id
    }

    pub fn identity(&self) -> &ReplayIdentity {
        &self.identity
    }

    pub fn decisions(&self) -> &[ReplayDecision] {
        &self.decisions
    }

    pub fn fills(&self) -> &[SimulatedFill] {
        &self.fills
    }

    /// One P&L row per replay step (AC40).
    pub fn pnl(&self) -> &[BarPnl] {
        &self.pnl
    }

    /// Every input object the result was computed from, each as an exact reference (AC44).
    pub fn lineage(&self) -> Vec<ReplayLineage> {
        let identity = &self.identity;
        let hashed = |object: &Ulid, hash: &ContentHash| {
            LineageRef::content_addressed(object.clone(), hash.clone())
        };
        let versioned = |reference: &VersionRef| {
            LineageRef::versioned(reference.id().clone(), reference.version())
        };
        let mut lineage = vec![
            ReplayLineage {
                role: LineageRole::Policy,
                reference: hashed(&identity.policy_id, &identity.policy),
            },
            ReplayLineage {
                role: LineageRole::DataSnapshot,
                reference: hashed(&identity.data_snapshot, &identity.data_snapshot_content),
            },
            ReplayLineage {
                role: LineageRole::Subject,
                reference: versioned(&identity.subject),
            },
            ReplayLineage {
                role: LineageRole::SubjectState,
                reference: hashed(&identity.subject_state, &identity.subject_state_content),
            },
        ];
        if let Some(constraint_set) = &identity.constraint_set {
            lineage.push(ReplayLineage {
                role: LineageRole::ConstraintSet,
                reference: versioned(constraint_set),
            });
        }
        lineage.push(ReplayLineage {
            role: LineageRole::FundingRulePack,
            reference: versioned(identity.funding.rule_pack()),
        });
        lineage.push(ReplayLineage {
            role: LineageRole::CouponData,
            // With a version present the reference is always valid; the fallback is unreachable.
            reference: LineageRef::new(
                identity.carry_source.id().clone(),
                Some(identity.carry_source.version()),
                Some(identity.carry.clone()),
            )
            .unwrap_or_else(|_| versioned(&identity.carry_source)),
        });
        lineage
    }

    pub fn book(&self) -> &SimulatedBook {
        &self.book
    }

    /// The part of the result fixed by time `t`: decisions made, fills effective and P&L rows known
    /// at or before it (AC38 compares these between snapshots).
    pub fn through(
        &self,
        t: DateTime<Utc>,
    ) -> (Vec<&ReplayDecision>, Vec<&SimulatedFill>, Vec<&BarPnl>) {
        (
            self.decisions
                .iter()
                .filter(|decision| decision.decision_time <= t)
                .collect(),
            self.fills
                .iter()
                .filter(|fill| fill.effective_at <= t)
                .collect(),
            self.pnl.iter().filter(|row| row.known_at <= t).collect(),
        )
    }
}

struct Canonical(Vec<u8>);

impl Canonical {
    fn new(tag: &str) -> Self {
        let mut value = Self(Vec::new());
        value.text(tag);
        value
    }

    fn text(&mut self, value: &str) {
        self.0
            .extend_from_slice(&(value.len() as u64).to_be_bytes());
        self.0.extend_from_slice(value.as_bytes());
    }

    fn number(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn time(&mut self, value: DateTime<Utc>) {
        self.0
            .extend_from_slice(&value.timestamp_micros().to_be_bytes());
    }

    fn version_ref(&mut self, value: &VersionRef) {
        self.text(value.id().as_str());
        self.number(value.version().get());
    }

    fn hash(&mut self, value: &ContentHash) {
        self.0.extend_from_slice(value.as_bytes());
    }

    fn amount(&mut self, value: ReplayAmount) {
        self.text(&value.canonical());
    }

    fn optional<T>(&mut self, value: Option<T>, write: impl FnOnce(&mut Self, T)) {
        match value {
            Some(inner) => {
                self.0.push(1);
                write(self, inner);
            }
            None => self.0.push(0),
        }
    }
}

fn policy_bytes(input: &PolicyArtifactInput) -> Vec<u8> {
    let mut bytes = Canonical::new("ficant-policy-artifact/v4");
    bytes.text(input.policy_artifact_id.as_str());
    bytes.text(input.owner.tenant_id().as_str());
    bytes.text(input.owner.owner_id().as_str());
    bytes.text(&input.family);
    bytes.number(u64::from(input.family_version));
    bytes.number(input.parameters.len() as u64);
    for (name, value) in &input.parameters {
        bytes.text(name);
        bytes.amount(*value);
    }
    bytes.number(input.universe.len() as u64);
    for instrument in &input.universe {
        bytes.version_ref(instrument);
    }
    bytes.text(input.frequency.code());
    bytes.text(input.settlement.code());
    bytes.time(input.valid.from().instant());
    bytes.time(input.valid.to().instant());
    bytes.amount(input.execution.participation_cap());
    bytes.text(input.execution.remainder().code());
    bytes.number(input.constraints.len() as u64);
    for (code, handling) in &input.constraints {
        bytes.text(code);
        bytes.text(handling.code());
    }
    bytes.optional(input.calibration.as_ref(), calibration_bytes);
    bytes.0
}

fn calibration_bytes(bytes: &mut Canonical, record: &CalibrationRecord) {
    bytes.hash(record.base_policy());
    for window in [record.training(), record.validation()] {
        bytes.time(window.from().instant());
        bytes.time(window.to().instant());
    }
    bytes.text(record.objective().code());
    bytes.number(record.grid().axes().len() as u64);
    for (name, values) in record.grid().axes() {
        bytes.text(name);
        bytes.number(values.len() as u64);
        for value in values {
            bytes.amount(*value);
        }
    }
    bytes.number(record.candidates().len() as u64);
    for candidate in record.candidates() {
        bytes.number(candidate.parameters.len() as u64);
        for (name, value) in &candidate.parameters {
            bytes.text(name);
            bytes.amount(*value);
        }
        for window in [&candidate.training, &candidate.validation] {
            bytes.hash(&window.result_id);
            bytes.amount(window.objective);
        }
    }
    bytes.number(record.selected() as u64);
}

fn result_bytes(
    identity: &ReplayIdentity,
    decisions: &[ReplayDecision],
    fills: &[SimulatedFill],
    pnl: &[BarPnl],
    book: &SimulatedBook,
) -> Vec<u8> {
    let mut bytes = Canonical::new("ficant-simulation-result/v3");
    identity_bytes(&mut bytes, identity);
    bytes.number(decisions.len() as u64);
    for decision in decisions {
        bytes.time(decision.decision_time);
        bytes.optional(decision.inputs_visible_through, Canonical::time);
        bytes.number(decision.input_bar_count as u64);
        bytes.number(decision.target.targets.len() as u64);
        for (instrument, face) in decision.target.iter() {
            bytes.version_ref(instrument);
            bytes.amount(*face);
        }
    }
    bytes.number(fills.len() as u64);
    for fill in fills {
        fill_bytes(&mut bytes, fill);
    }
    bytes.number(pnl.len() as u64);
    for row in pnl {
        pnl_bytes(&mut bytes, row);
    }
    bytes.number(book.positions.len() as u64);
    for (instrument, face) in book.positions() {
        bytes.version_ref(instrument);
        bytes.amount(*face);
    }
    bytes.amount(book.cash);
    bytes.0
}

fn identity_bytes(bytes: &mut Canonical, identity: &ReplayIdentity) {
    bytes.text(identity.policy_id.as_str());
    bytes.hash(&identity.policy);
    bytes.text(identity.data_snapshot.as_str());
    bytes.hash(&identity.data_snapshot_content);
    bytes.version_ref(&identity.subject);
    bytes.text(identity.subject_state.as_str());
    bytes.hash(&identity.subject_state_content);
    bytes.optional(identity.constraint_set.as_ref(), Canonical::version_ref);
    bytes.number(identity.constraints.len() as u64);
    for constraint in &identity.constraints {
        bytes.text(&constraint.limit_code);
        bytes.text(constraint.measure.code());
        bytes.amount(constraint.ceiling);
        bytes.text(constraint.handling.code());
    }
    bytes.version_ref(identity.funding.rule_pack());
    bytes.text(match identity.funding.tier() {
        FundingTier::DrAvailable => "DR_AVAILABLE",
        FundingTier::ROnly => "R_ONLY",
    });
    bytes.amount(identity.funding.annual_rate());
    bytes.number(u64::from(identity.funding.day_basis()));
    bytes.version_ref(&identity.carry_source);
    bytes.hash(&identity.carry);
    bytes.text(&identity.cost_model);
    bytes.number(identity.cost_parameters.len() as u64);
    for (name, value) in &identity.cost_parameters {
        bytes.text(name);
        bytes.amount(*value);
    }
    bytes.number(identity.seed);
    bytes.hash(&identity.code);
    bytes.hash(&identity.runtime);
}

fn fill_bytes(bytes: &mut Canonical, fill: &SimulatedFill) {
    bytes.time(fill.decision_time);
    bytes.version_ref(&fill.instrument);
    bytes.amount(fill.requested);
    bytes.amount(fill.quantity);
    bytes.optional(fill.limit, Canonical::amount);
    bytes.time(fill.effective_at);
    bytes.optional(fill.execution_bar.as_deref(), Canonical::text);
    bytes.optional(fill.price, Canonical::amount);
    bytes.optional(fill.basis, |bytes, basis| bytes.text(basis.code()));
    bytes.text(price_source_code(fill.price_source));
    bytes.optional(fill.no_fill, |bytes, reason| bytes.text(reason.code()));
    bytes.optional(fill.remainder, |bytes, remainder| {
        bytes.amount(remainder.quantity);
        bytes.text(remainder.reason.code());
        bytes.text(remainder.outcome.code());
    });
    bytes.optional(fill.constraint.as_ref(), |bytes, outcome| {
        bytes.amount(outcome.unconstrained);
        bytes.number(outcome.triggered.len() as u64);
        for code in &outcome.triggered {
            bytes.text(code);
        }
        bytes.text(outcome.action.code());
    });
}

fn pnl_bytes(bytes: &mut Canonical, row: &BarPnl) {
    bytes.time(row.bar_start);
    bytes.time(row.bar_end);
    bytes.time(row.known_at);
    bytes.text(&row.trading_date.to_string());
    bytes.number(row.instruments.len() as u64);
    for (instrument, item) in &row.instruments {
        bytes.version_ref(instrument);
        for value in [
            item.position_start,
            item.position_end,
            item.mark,
            item.accrued_interest,
            item.price,
            item.coupon,
            item.transaction_cost,
        ] {
            bytes.amount(value);
        }
    }
    for value in [row.funding, row.total, row.cash, row.equity] {
        bytes.amount(value);
    }
}

const fn price_source_code(value: PriceSourceType) -> &'static str {
    match value {
        PriceSourceType::RealTrade => "REAL_TRADE",
        PriceSourceType::ActiveQuote => "ACTIVE_QUOTE",
        PriceSourceType::ModelValuation => "MODEL_VALUATION",
        PriceSourceType::CurveInterpolation => "CURVE_INTERPOLATION",
    }
}

/// Convenience used by policies and tests.
pub fn policy_period_contains(policy: &PolicyArtifact, instant: DateTime<Utc>) -> bool {
    policy.valid().from().instant() <= instant && instant < policy.valid().to().instant()
}
