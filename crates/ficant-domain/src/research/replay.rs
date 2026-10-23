//! Point-in-time bar replay building blocks (ADR-0024).
//!
//! Everything here is pure and deterministic: no clock, no randomness, no I/O, ordered
//! collections only. The replay loop that schedules decisions lives in the application layer; it
//! can only hand a policy a [`PointInTimeView`], whose constructor refuses any bar that was not yet
//! visible at the decision time (I1, AC38).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::market::{Bar, BarFrequency, BarSettlement};
use crate::primitives::{ContentHash, EffectivePeriod, OwnerRef, Ulid, VersionRef};
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

    /// Canonical text: no trailing zeros, no exponent.
    pub fn canonical(self) -> String {
        self.0.normalize().to_string()
    }

    fn of(value: &crate::primitives::DecimalValue) -> Self {
        Self(value.as_decimal().normalize())
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
                .any(|key| key.trim().is_empty() || key != key.trim())
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

/// Simulated positions (face amount) and cash. Cash moves by `face × price / 100`.
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
    pub fn with_pending<'a>(&self, pending: impl IntoIterator<Item = &'a SimulatedFill>) -> Self {
        let mut projected = self.clone();
        for fill in pending {
            let position = projected.position(&fill.instrument).0 + fill.quantity.0;
            if position.is_zero() {
                projected.positions.remove(&fill.instrument);
            } else {
                projected
                    .positions
                    .insert(fill.instrument.clone(), ReplayAmount(position.normalize()));
            }
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

    fn apply(&mut self, fill: &SimulatedFill) {
        let (Some(price), true) = (fill.price, fill.filled()) else {
            return;
        };
        let position = self.position(&fill.instrument).0 + fill.quantity.0;
        if position.is_zero() {
            self.positions.remove(&fill.instrument);
        } else {
            self.positions
                .insert(fill.instrument.clone(), ReplayAmount(position.normalize()));
        }
        let notional = fill.quantity.0 * price.0 / Decimal::ONE_HUNDRED;
        self.cash = ReplayAmount((self.cash.0 - notional).normalize());
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

/// Which price a simulated fill used (I9 travels with the snapshot's price source type).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FillPriceBasis {
    Vwap,
    Close,
}

/// Why a requested trade did not fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NoFillReason {
    /// The execution bar had no trades (AC41).
    NoPriceInBar,
    /// The snapshot has no bar after the decision for this instrument.
    NoLaterBar,
}

/// One simulated execution of a decision's delta. Never an order (AC44).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimulatedFill {
    pub decision_time: DateTime<Utc>,
    pub instrument: VersionRef,
    pub quantity: ReplayAmount,
    /// When the fill became part of the book: end of the execution bar, or the decision time when
    /// nothing could be executed.
    pub effective_at: DateTime<Utc>,
    pub execution_bar: Option<String>,
    pub price: Option<ReplayAmount>,
    pub basis: Option<FillPriceBasis>,
    pub no_fill: Option<NoFillReason>,
}

impl SimulatedFill {
    pub fn filled(&self) -> bool {
        self.no_fill.is_none()
    }

    /// Simulates executing `quantity` in `execution_bar`, the first bar starting at or after the
    /// decision time. Price is the bar's VWAP, else its close; no trades means no fill.
    pub fn simulate(
        decision_time: DateTime<Utc>,
        instrument: VersionRef,
        quantity: ReplayAmount,
        execution_bar: Option<&Bar>,
    ) -> Self {
        let Some(bar) = execution_bar else {
            return Self {
                decision_time,
                instrument,
                quantity,
                effective_at: decision_time,
                execution_bar: None,
                price: None,
                basis: None,
                no_fill: Some(NoFillReason::NoLaterBar),
            };
        };
        let (price, basis, no_fill) = match (bar.vwap(), bar.prices()) {
            (Some(vwap), _) => (
                Some(ReplayAmount::of(vwap)),
                Some(FillPriceBasis::Vwap),
                None,
            ),
            (None, Some(prices)) => (
                Some(ReplayAmount::of(&prices.close)),
                Some(FillPriceBasis::Close),
                None,
            ),
            (None, None) => (None, None, Some(NoFillReason::NoPriceInBar)),
        };
        Self {
            decision_time,
            instrument,
            quantity,
            effective_at: bar.bar_end(),
            execution_bar: Some(bar.source_record_id().to_owned()),
            price,
            basis,
            no_fill,
        }
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

/// Everything that identifies a replay's inputs (AC39). Replacing any field changes the result ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayIdentity {
    pub policy: ContentHash,
    pub data_snapshot: Ulid,
    pub data_snapshot_content: ContentHash,
    pub subject: VersionRef,
    pub subject_state: VersionRef,
    pub cost_model: String,
    pub cost_parameters: BTreeMap<String, ReplayAmount>,
    pub seed: u64,
    pub code: ContentHash,
    pub runtime: ContentHash,
}

/// Outcome of one replay. Decisions and fills are in time order; the ID binds inputs and outputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimulationResult {
    result_id: ContentHash,
    identity: ReplayIdentity,
    decisions: Vec<ReplayDecision>,
    fills: Vec<SimulatedFill>,
    book: SimulatedBook,
}

impl SimulationResult {
    pub fn new(
        identity: ReplayIdentity,
        decisions: Vec<ReplayDecision>,
        fills: Vec<SimulatedFill>,
        book: SimulatedBook,
    ) -> DomainResult<Self> {
        let ordered = decisions
            .windows(2)
            .all(|pair| pair[0].decision_time < pair[1].decision_time)
            && fills.windows(2).all(|pair| {
                (pair[0].effective_at, &pair[0].instrument)
                    <= (pair[1].effective_at, &pair[1].instrument)
            });
        let visible = decisions.iter().all(|decision| {
            decision
                .inputs_visible_through
                .is_none_or(|time| time <= decision.decision_time)
        });
        if !ordered || !visible {
            return Err(DomainErrorCode::InvalidValue);
        }
        let result_id = ContentHash::digest(&result_bytes(&identity, &decisions, &fills, &book));
        Ok(Self {
            result_id,
            identity,
            decisions,
            fills,
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

    pub fn book(&self) -> &SimulatedBook {
        &self.book
    }

    /// The part of the result fixed by time `t`: decisions made and fills effective at or before
    /// it (AC38 compares these between snapshots).
    pub fn through(&self, t: DateTime<Utc>) -> (Vec<&ReplayDecision>, Vec<&SimulatedFill>) {
        (
            self.decisions
                .iter()
                .filter(|decision| decision.decision_time <= t)
                .collect(),
            self.fills
                .iter()
                .filter(|fill| fill.effective_at <= t)
                .collect(),
        )
    }
}

/// Applies fills to a book in order; used by the replay loop.
pub fn apply_fills<'a>(
    book: &mut SimulatedBook,
    fills: impl IntoIterator<Item = &'a SimulatedFill>,
) {
    for fill in fills {
        book.apply(fill);
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
    let mut bytes = Canonical::new("ficant-policy-artifact/v1");
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
    bytes.0
}

fn result_bytes(
    identity: &ReplayIdentity,
    decisions: &[ReplayDecision],
    fills: &[SimulatedFill],
    book: &SimulatedBook,
) -> Vec<u8> {
    let mut bytes = Canonical::new("ficant-simulation-result/v1");
    bytes.hash(&identity.policy);
    bytes.text(identity.data_snapshot.as_str());
    bytes.hash(&identity.data_snapshot_content);
    bytes.version_ref(&identity.subject);
    bytes.version_ref(&identity.subject_state);
    bytes.text(&identity.cost_model);
    bytes.number(identity.cost_parameters.len() as u64);
    for (name, value) in &identity.cost_parameters {
        bytes.text(name);
        bytes.amount(*value);
    }
    bytes.number(identity.seed);
    bytes.hash(&identity.code);
    bytes.hash(&identity.runtime);
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
        bytes.time(fill.decision_time);
        bytes.version_ref(&fill.instrument);
        bytes.amount(fill.quantity);
        bytes.time(fill.effective_at);
        bytes.optional(fill.execution_bar.as_deref(), Canonical::text);
        bytes.optional(fill.price, Canonical::amount);
        bytes.optional(fill.basis, |bytes, basis| {
            bytes.text(match basis {
                FillPriceBasis::Vwap => "VWAP",
                FillPriceBasis::Close => "CLOSE",
            });
        });
        bytes.optional(fill.no_fill, |bytes, reason| {
            bytes.text(match reason {
                NoFillReason::NoPriceInBar => "NO_PRICE_IN_BAR",
                NoFillReason::NoLaterBar => "NO_LATER_BAR",
            });
        });
    }
    bytes.number(book.positions.len() as u64);
    for (instrument, face) in book.positions() {
        bytes.version_ref(instrument);
        bytes.amount(*face);
    }
    bytes.amount(book.cash);
    bytes.0
}

/// Convenience used by policies and tests.
pub fn policy_period_contains(policy: &PolicyArtifact, instant: DateTime<Utc>) -> bool {
    policy.valid().from().instant() <= instant && instant < policy.valid().to().instant()
}
