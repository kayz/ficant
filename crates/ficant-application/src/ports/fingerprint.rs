use ficant_domain::market::{
    ArtifactInputKind, Bond, BondBusinessDayConvention, BondCouponFrequency,
    BondDayCountConvention, BondTaxAttributes, Calendar, Cashflow, CashflowType, CurveSnapshot,
    FactSource, FuturesContract, IncomeTaxStatus, Instrument, InstrumentKind, MarketRulePack,
    Quote, Trade, Unit, Valuation, ValueAddedTaxStatus, VerificationStatus,
};
use ficant_domain::primitives::ContentHash;
use ficant_domain::primitives::{
    DecimalValue, EffectivePeriod, LineageRef, MarketTime, OwnerRef, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    Artifact, ArtifactKind, DataSnapshot, ExperimentRun, RunJournal, RunState, SignalSet,
    UniverseSnapshot,
};
use ficant_domain::{ContentAddressed, Lineaged, VersionedDefinition};

use super::definitions::{DefinitionValue, InstrumentDefinition, InstrumentSubtype};
use super::facts::MarketFact;
use super::snapshots::SnapshotValue;

const MAGIC: &[u8; 4] = b"FCMD";
const VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationFingerprint(ContentHash);

impl OperationFingerprint {
    #[must_use]
    pub fn content_hash(&self) -> &ContentHash {
        &self.0
    }
}

pub(crate) struct FingerprintBuilder {
    bytes: Vec<u8>,
}

impl FingerprintBuilder {
    pub(crate) fn new(schema: &str) -> Self {
        let mut result = Self {
            bytes: Vec::with_capacity(256),
        };
        result.bytes.extend_from_slice(MAGIC);
        result.bytes.extend_from_slice(&VERSION.to_be_bytes());
        result.field(1, schema.as_bytes());
        result
    }

    pub(crate) fn field(&mut self, tag: u8, value: &[u8]) -> &mut Self {
        self.bytes.push(tag);
        let length = u64::try_from(value.len()).expect("canonical field length fits in u64");
        self.bytes.extend_from_slice(&length.to_be_bytes());
        self.bytes.extend_from_slice(value);
        self
    }

    pub(crate) fn u64(&mut self, tag: u8, value: u64) -> &mut Self {
        self.field(tag, &value.to_be_bytes())
    }

    pub(crate) fn optional_u64(&mut self, tag: u8, value: Option<u64>) -> &mut Self {
        let mut encoded = Vec::with_capacity(9);
        match value {
            Some(value) => {
                encoded.push(1);
                encoded.extend_from_slice(&value.to_be_bytes());
            }
            None => encoded.push(0),
        }
        self.field(tag, &encoded)
    }

    pub(crate) fn finish(self) -> OperationFingerprint {
        OperationFingerprint(ContentHash::digest(&self.bytes))
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

pub(crate) fn owner_bytes(owner: &OwnerRef) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("owner-ref/v1");
    value.field(2, owner.tenant_id().as_str().as_bytes());
    value.field(3, owner.owner_id().as_str().as_bytes());
    value.into_bytes()
}

pub(crate) fn version_ref_bytes(reference: &VersionRef) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("version-ref/v1");
    value.field(2, reference.id().as_str().as_bytes());
    value.u64(3, reference.version().get());
    value.into_bytes()
}

fn unit_ref_bytes(reference: &UnitRef) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("unit-ref/v1");
    value.field(2, reference.unit_id().as_str().as_bytes());
    value.u64(3, reference.version().get());
    value.into_bytes()
}

fn decimal_bytes(decimal: &DecimalValue) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("decimal/v1");
    value.field(2, decimal.coefficient().as_bytes());
    value.u64(3, u64::from(decimal.scale()));
    value.field(4, &unit_ref_bytes(decimal.unit()));
    value.into_bytes()
}

pub(crate) fn market_time_bytes(time: &MarketTime) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("market-time/v1");
    value.field(2, &time.instant().timestamp().to_be_bytes());
    value.field(3, &time.instant().timestamp_subsec_nanos().to_be_bytes());
    value.field(4, time.market_timezone().as_bytes());
    value.field(5, time.local_trading_date().to_string().as_bytes());
    value.into_bytes()
}

fn period_bytes(period: &EffectivePeriod) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("effective-period/v1");
    value.field(2, &market_time_bytes(period.from()));
    value.field(3, &market_time_bytes(period.to()));
    value.into_bytes()
}

fn lineage_ref_bytes(reference: &LineageRef) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("lineage-ref/v1");
    value.field(2, reference.object_id().as_str().as_bytes());
    value.optional_u64(3, reference.version().map(Version::get));
    match reference.content_hash() {
        Some(hash) => {
            value.field(4, &[1]);
            value.field(5, hash.as_bytes());
        }
        None => {
            value.field(4, &[0]);
        }
    }
    value.into_bytes()
}

pub(crate) fn lineage_bytes(lineage: &[LineageRef]) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("lineage-list/v1");
    value.u64(
        2,
        u64::try_from(lineage.len()).expect("lineage count fits in u64"),
    );
    let mut references = lineage.iter().collect::<Vec<_>>();
    references.sort_by(|left, right| compare_lineage(left, right));
    for reference in references {
        value.field(3, &lineage_ref_bytes(reference));
    }
    value.into_bytes()
}

fn source_bytes(source: &FactSource) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("fact-source/v1");
    value.field(2, source.source_id().as_bytes());
    value.field(3, source.external_id().as_bytes());
    value.u64(4, source.source_revision());
    value.into_bytes()
}

fn optional_id_bytes(value: Option<&ficant_domain::primitives::Ulid>) -> Vec<u8> {
    match value {
        Some(value) => {
            let mut result = Vec::with_capacity(27);
            result.push(1);
            result.extend_from_slice(value.as_str().as_bytes());
            result
        }
        None => vec![0],
    }
}

pub(crate) fn definition_bytes(definition: &DefinitionValue) -> Vec<u8> {
    match definition {
        DefinitionValue::Instrument(value) => instrument_definition_bytes(value),
        DefinitionValue::Calendar(value) => calendar_bytes(value),
        DefinitionValue::Unit(value) => unit_bytes(value),
        DefinitionValue::MarketRulePack(value) => rule_pack_bytes(value),
    }
}

pub(crate) fn definition_content_hash(definition: &DefinitionValue) -> ContentHash {
    ContentHash::digest(&definition_bytes(definition))
}

fn instrument_definition_bytes(definition: &InstrumentDefinition) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("definition/instrument-aggregate/v1");
    value.field(2, &instrument_bytes(definition.instrument()));
    match definition.subtype() {
        None => {
            value.field(3, &[0]);
        }
        Some(InstrumentSubtype::Bond(bond)) => {
            value.field(3, &[1]);
            value.field(4, &bond_bytes(bond));
        }
        Some(InstrumentSubtype::FuturesContract(contract)) => {
            value.field(3, &[2]);
            value.field(4, &futures_bytes(contract));
        }
    }
    value.into_bytes()
}

fn instrument_bytes(instrument: &Instrument) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("definition/instrument/v1");
    value.field(2, instrument.id().as_str().as_bytes());
    value.u64(3, instrument.version());
    value.field(4, &owner_bytes(instrument.owner()));
    value.field(5, &[instrument_kind_code(instrument.kind())]);
    value.field(6, instrument.market().as_bytes());
    value.field(7, instrument.symbol().as_bytes());
    value.field(8, &unit_ref_bytes(instrument.currency()));
    value.field(9, &version_ref_bytes(instrument.calendar()));
    value.into_bytes()
}

fn bond_bytes(bond: &Bond) -> Vec<u8> {
    if let Some(pricing) = bond.pricing_terms() {
        let mut value = FingerprintBuilder::new("definition/bond/v3");
        value.field(2, &version_ref_bytes(bond.instrument()));
        value.field(3, bond.first_issue_date().to_string().as_bytes());
        value.field(4, bond.current_issue_date().to_string().as_bytes());
        value.field(5, bond.maturity_date().to_string().as_bytes());
        value.field(6, &decimal_bytes(bond.cumulative_issued_amount()));
        value.field(
            7,
            &bond_tax_attributes_bytes(
                bond.tax_attributes()
                    .expect("priced Bond construction requires tax attributes"),
            ),
        );
        value.field(8, &decimal_bytes(bond.face_value()));
        value.field(9, &decimal_bytes(pricing.coupon_rate()));
        value.field(
            10,
            &[match pricing.frequency() {
                BondCouponFrequency::Annual => 1,
                BondCouponFrequency::Semiannual => 2,
            }],
        );
        value.field(
            11,
            &[match pricing.day_count() {
                BondDayCountConvention::ActActBondIsma => 1,
            }],
        );
        value.field(
            12,
            &[match pricing.business_day() {
                BondBusinessDayConvention::Following => 1,
            }],
        );
        return value.into_bytes();
    }
    let Some(tax_attributes) = bond.tax_attributes() else {
        let mut legacy = FingerprintBuilder::new("definition/bond/v1");
        legacy.field(2, &version_ref_bytes(bond.instrument()));
        legacy.field(3, bond.first_issue_date().to_string().as_bytes());
        legacy.field(4, bond.maturity_date().to_string().as_bytes());
        legacy.field(5, &decimal_bytes(bond.face_value()));
        return legacy.into_bytes();
    };

    let mut value = FingerprintBuilder::new("definition/bond/v2");
    value.field(2, &version_ref_bytes(bond.instrument()));
    value.field(3, bond.first_issue_date().to_string().as_bytes());
    value.field(4, bond.current_issue_date().to_string().as_bytes());
    value.field(5, bond.maturity_date().to_string().as_bytes());
    value.field(6, &decimal_bytes(bond.cumulative_issued_amount()));
    value.field(7, &bond_tax_attributes_bytes(tax_attributes));
    value.field(8, &decimal_bytes(bond.face_value()));
    value.into_bytes()
}

fn bond_tax_attributes_bytes(attributes: BondTaxAttributes) -> [u8; 2] {
    [
        match attributes.value_added_tax_status() {
            ValueAddedTaxStatus::Exempt => 1,
            ValueAddedTaxStatus::Taxable => 2,
        },
        match attributes.income_tax_status() {
            IncomeTaxStatus::Exempt => 1,
            IncomeTaxStatus::Taxable => 2,
        },
    ]
}

fn futures_bytes(contract: &FuturesContract) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("definition/futures/v1");
    value.field(2, &version_ref_bytes(contract.instrument()));
    value.field(3, &market_time_bytes(contract.last_trade_time()));
    value.field(4, &market_time_bytes(contract.expiry_time()));
    value.field(5, &market_time_bytes(contract.settlement_time()));
    value.field(6, &decimal_bytes(contract.multiplier()));
    value.field(7, &version_ref_bytes(contract.rule_pack()));
    if let (Some(product_code), Some(price_unit)) = (contract.product_code(), contract.price_unit())
    {
        value.field(8, product_code.as_bytes());
        value.field(9, &unit_ref_bytes(price_unit));
    }
    value.into_bytes()
}

fn calendar_bytes(calendar: &Calendar) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("definition/calendar/v1");
    value.field(2, calendar.identity().as_bytes());
    value.u64(3, calendar.version());
    value.field(4, &owner_bytes(calendar.owner()));
    value.field(5, calendar.market().as_bytes());
    value.field(6, calendar.market_timezone().as_bytes());
    value.field(7, &period_bytes(calendar.effective()));
    value.u64(
        8,
        u64::try_from(calendar.sessions().len()).expect("session count fits"),
    );
    for session in calendar.sessions() {
        let mut encoded = FingerprintBuilder::new("calendar-session/v1");
        encoded.field(2, session.local_date().to_string().as_bytes());
        encoded.field(
            3,
            session
                .open_local_time()
                .map_or_else(|| "-".to_owned(), |time| time.to_string())
                .as_bytes(),
        );
        encoded.field(
            4,
            session
                .close_local_time()
                .map_or_else(|| "-".to_owned(), |time| time.to_string())
                .as_bytes(),
        );
        value.field(9, &encoded.into_bytes());
    }
    value.into_bytes()
}

fn unit_bytes(unit: &Unit) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("definition/unit/v1");
    value.field(2, unit.identity().as_bytes());
    value.u64(3, unit.version());
    value.field(4, &owner_bytes(unit.owner()));
    value.field(5, unit.code().as_bytes());
    value.field(6, unit.dimension().as_bytes());
    value.u64(7, u64::from(unit.scale()));
    value.u64(8, u64::from(unit.precision()));
    value.into_bytes()
}

fn rule_pack_bytes(rule: &MarketRulePack) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("definition/rule-pack/v1");
    value.field(2, rule.identity().as_bytes());
    value.u64(3, rule.version());
    value.field(4, &owner_bytes(rule.owner()));
    value.field(5, rule.market().as_bytes());
    value.field(6, rule.rule_type().as_bytes());
    value.field(7, rule.source().as_bytes());
    value.field(8, &period_bytes(rule.effective()));
    value.field(9, &[verification_status_code(rule.verification_status())]);
    value.field(10, rule.content_hash().as_bytes());
    value.into_bytes()
}

pub(crate) fn curve_snapshot_bytes(curve: &CurveSnapshot) -> Vec<u8> {
    let mut value = FingerprintBuilder::new(if curve.visible_at().is_some() {
        "curve-snapshot/v2"
    } else {
        "curve-snapshot/v1"
    });
    value.field(2, curve.id().as_str().as_bytes());
    value.field(3, &owner_bytes(curve.owner()));
    value.field(4, &market_time_bytes(curve.as_of()));
    value.field(5, &unit_ref_bytes(curve.currency()));
    value.field(6, curve.curve_kind().as_bytes());
    value.field(7, &version_ref_bytes(curve.calendar()));
    value.field(8, &version_ref_bytes(curve.rule_pack()));
    value.field(9, curve.point_schema().as_bytes());
    value.field(10, curve.content_hash().as_bytes());
    value.field(11, &lineage_bytes(curve.lineage()));
    value.field(12, &[artifact_input_kind_code(curve.input_kind())]);
    if let (Some(visible_at), Some(curve_family_id)) = (curve.visible_at(), curve.curve_family_id())
    {
        value.field(13, &market_time_bytes(visible_at));
        value.field(14, curve_family_id.as_bytes());
    }
    value.into_bytes()
}

pub(crate) fn fact_bytes(fact: &MarketFact) -> Vec<u8> {
    match fact {
        MarketFact::Cashflow(value) => cashflow_bytes(value),
        MarketFact::Quote(value) => quote_bytes(value),
        MarketFact::Trade(value) => trade_bytes(value),
        MarketFact::Valuation(value) => valuation_bytes(value),
    }
}

fn cashflow_bytes(fact: &Cashflow) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("fact/cashflow/v1");
    value.field(2, fact.id().as_str().as_bytes());
    value.field(3, &version_ref_bytes(fact.bond()));
    value.field(4, &market_time_bytes(fact.payment_time()));
    value.field(5, &decimal_bytes(fact.amount()));
    value.field(6, &owner_bytes(fact.owner()));
    value.field(7, &source_bytes(fact.source()));
    value.field(8, &optional_id_bytes(fact.supersedes_id()));
    value.field(9, &[cashflow_type_code(fact.cashflow_type())]);
    value.field(10, fact.schedule_id().as_bytes());
    value.u64(11, fact.sequence());
    value.into_bytes()
}

fn quote_bytes(fact: &Quote) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("fact/quote/v1");
    value.field(2, fact.id().as_str().as_bytes());
    value.field(3, &version_ref_bytes(fact.instrument()));
    value.field(4, &owner_bytes(fact.owner()));
    value.field(5, &source_bytes(fact.source()));
    value.field(6, &market_time_bytes(fact.observed_at()));
    value.field(7, &market_time_bytes(fact.received_at()));
    value.field(
        8,
        &fact.bid().map_or_else(
            || vec![0],
            |decimal| {
                let mut result = vec![1];
                result.extend_from_slice(&decimal_bytes(decimal));
                result
            },
        ),
    );
    value.field(
        9,
        &fact.ask().map_or_else(
            || vec![0],
            |decimal| {
                let mut result = vec![1];
                result.extend_from_slice(&decimal_bytes(decimal));
                result
            },
        ),
    );
    value.field(10, &optional_id_bytes(fact.supersedes_id()));
    value.into_bytes()
}

fn trade_bytes(fact: &Trade) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("fact/trade/v1");
    value.field(2, fact.id().as_str().as_bytes());
    value.field(3, &version_ref_bytes(fact.instrument()));
    value.field(4, &owner_bytes(fact.owner()));
    value.field(5, &source_bytes(fact.source()));
    value.field(6, &market_time_bytes(fact.executed_at()));
    value.field(7, &decimal_bytes(fact.price()));
    value.field(8, &decimal_bytes(fact.quantity()));
    value.field(9, &optional_id_bytes(fact.supersedes_id()));
    value.into_bytes()
}

fn valuation_bytes(fact: &Valuation) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("fact/valuation/v1");
    value.field(2, fact.id().as_str().as_bytes());
    value.field(3, &version_ref_bytes(fact.instrument()));
    value.field(4, &owner_bytes(fact.owner()));
    value.field(5, &source_bytes(fact.source()));
    value.field(6, &market_time_bytes(fact.valuation_at()));
    value.field(7, fact.method().as_bytes());
    value.field(8, &version_ref_bytes(fact.rule_pack()));
    value.u64(
        9,
        u64::try_from(fact.values().len()).expect("value count fits"),
    );
    for decimal in fact.values() {
        value.field(10, &decimal_bytes(decimal));
    }
    value.field(11, &optional_id_bytes(fact.supersedes_id()));
    value.into_bytes()
}

pub(crate) fn snapshot_bytes(snapshot: &SnapshotValue) -> Vec<u8> {
    match snapshot {
        SnapshotValue::Data(value) => data_snapshot_bytes(value),
        SnapshotValue::Position(value) => {
            let mut fingerprint = FingerprintBuilder::new("snapshot/position/v1");
            fingerprint.field(2, value.id().as_str().as_bytes());
            fingerprint.field(3, &owner_bytes(value.owner()));
            fingerprint.field(4, value.content_hash().as_bytes());
            fingerprint.field(5, &lineage_bytes(value.lineage()));
            fingerprint.into_bytes()
        }
        SnapshotValue::Universe(value) => universe_snapshot_bytes(value),
    }
}

fn data_snapshot_bytes(snapshot: &DataSnapshot) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("snapshot/data/v1");
    value.field(2, snapshot.id().as_str().as_bytes());
    value.field(3, &owner_bytes(snapshot.owner()));
    value.field(4, &market_time_bytes(snapshot.visible_at()));
    value.field(5, &market_time_bytes(snapshot.as_of()));
    value.field(6, snapshot.schema_hash().as_bytes());
    value.field(7, snapshot.manifest_hash().as_bytes());
    value.field(8, snapshot.content_hash().as_bytes());
    value.field(9, &lineage_bytes(snapshot.lineage()));
    value.into_bytes()
}

fn universe_snapshot_bytes(snapshot: &UniverseSnapshot) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("snapshot/universe/v1");
    value.field(2, snapshot.id().as_str().as_bytes());
    value.field(3, &owner_bytes(snapshot.owner()));
    value.u64(
        4,
        u64::try_from(snapshot.instrument_versions().len()).expect("instrument count fits"),
    );
    for reference in snapshot.instrument_versions() {
        value.field(5, &version_ref_bytes(reference));
    }
    value.field(6, snapshot.filter_digest().as_bytes());
    value.field(7, snapshot.content_hash().as_bytes());
    value.field(8, &lineage_bytes(snapshot.lineage()));
    value.into_bytes()
}

pub(crate) fn run_bytes(run: &ExperimentRun) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("experiment-run/v1");
    value.field(2, run.id().as_str().as_bytes());
    value.field(3, &owner_bytes(run.owner()));
    value.field(4, &lineage_ref_bytes(run.data_snapshot()));
    value.field(5, &lineage_ref_bytes(run.universe_snapshot()));
    value.u64(
        6,
        u64::try_from(run.rule_packs().len()).expect("rule count fits"),
    );
    let mut rule_packs = run.rule_packs().iter().collect::<Vec<_>>();
    rule_packs.sort_by(|left, right| compare_version_ref(left, right));
    for reference in rule_packs {
        value.field(7, &version_ref_bytes(reference));
    }
    value.field(8, run.runtime_image_digest().as_bytes());
    value.field(9, run.parameters_hash().as_bytes());
    value.u64(10, run.seed());
    value.field(11, &[run_state_code(run.state())]);
    value.u64(12, run.revision());
    value.into_bytes()
}

pub(crate) fn journal_bytes(event: &RunJournal) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("run-journal/v1");
    value.field(2, event.content_hash().as_bytes());
    value.into_bytes()
}

pub(crate) fn artifact_bytes(artifact: &Artifact) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("artifact/v1");
    value.field(2, artifact.id().as_str().as_bytes());
    value.field(3, &owner_bytes(artifact.owner()));
    value.field(4, &[artifact_kind_code(artifact.kind())]);
    value.field(5, artifact.media_type().as_bytes());
    value.field(6, artifact.content_hash().as_bytes());
    value.u64(7, artifact.blob_size());
    value.field(8, &lineage_bytes(artifact.lineage()));
    value.into_bytes()
}

pub(crate) fn signal_bytes(signal: &SignalSet) -> Vec<u8> {
    let mut value = FingerprintBuilder::new("signal-set/v1");
    value.field(2, signal.id().as_str().as_bytes());
    value.field(3, &owner_bytes(signal.owner()));
    value.field(4, &lineage_ref_bytes(signal.artifact()));
    value.field(5, signal.experiment_run_id().as_str().as_bytes());
    value.field(6, &lineage_ref_bytes(signal.data_snapshot()));
    value.field(7, &lineage_ref_bytes(signal.universe_snapshot()));
    let mut rule_packs = signal.rule_packs().iter().collect::<Vec<_>>();
    rule_packs.sort_by(|left, right| compare_version_ref(left, right));
    for reference in rule_packs {
        value.field(8, &version_ref_bytes(reference));
    }
    let mut input_artifacts = signal.input_artifacts().iter().collect::<Vec<_>>();
    input_artifacts.sort_by(|left, right| compare_lineage(left, right));
    for reference in input_artifacts {
        value.field(9, &lineage_ref_bytes(reference));
    }
    value.field(10, &period_bytes(signal.valid()));
    value.field(11, signal.content_hash().as_bytes());
    value.into_bytes()
}

fn compare_version_ref(left: &VersionRef, right: &VersionRef) -> Ordering {
    left.id()
        .as_str()
        .cmp(right.id().as_str())
        .then_with(|| left.version().get().cmp(&right.version().get()))
}

fn compare_lineage(left: &LineageRef, right: &LineageRef) -> Ordering {
    left.object_id()
        .as_str()
        .cmp(right.object_id().as_str())
        .then_with(|| {
            left.version()
                .map(Version::get)
                .cmp(&right.version().map(Version::get))
        })
        .then_with(|| {
            left.content_hash()
                .map(ContentHash::as_bytes)
                .cmp(&right.content_hash().map(ContentHash::as_bytes))
        })
}

pub(crate) const fn run_state_code(state: RunState) -> u8 {
    match state {
        RunState::Created => 1,
        RunState::Running => 2,
        RunState::Succeeded => 3,
        RunState::Failed => 4,
        RunState::Cancelled => 5,
    }
}

const fn instrument_kind_code(kind: InstrumentKind) -> u8 {
    match kind {
        InstrumentKind::Bond => 1,
        InstrumentKind::Futures => 2,
        InstrumentKind::Other => 3,
    }
}

const fn cashflow_type_code(value: CashflowType) -> u8 {
    match value {
        CashflowType::Coupon => 1,
        CashflowType::Principal => 2,
        CashflowType::Fee => 3,
        CashflowType::Other => 4,
    }
}

const fn verification_status_code(value: VerificationStatus) -> u8 {
    match value {
        VerificationStatus::Unverified => 1,
        VerificationStatus::Verified => 2,
        VerificationStatus::Rejected => 3,
    }
}

const fn artifact_kind_code(value: ArtifactKind) -> u8 {
    match value {
        ArtifactKind::Generic => 1,
        ArtifactKind::CurveSnapshot => 2,
        ArtifactKind::DataSnapshot => 3,
        ArtifactKind::UniverseSnapshot => 4,
        ArtifactKind::SignalSet => 5,
    }
}

const fn artifact_input_kind_code(value: ArtifactInputKind) -> u8 {
    match value {
        ArtifactInputKind::ExternalFixture => 1,
    }
}
use std::cmp::Ordering;
