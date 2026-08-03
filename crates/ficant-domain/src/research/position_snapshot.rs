use crate::primitives::{
    ContentHash, DecimalValue, LineageRef, MarketTime, OwnerRef, Ulid, VersionRef,
};
use crate::{ContentAddressed, DomainErrorCode, DomainResult, Lineaged};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountingClassificationState {
    Classified,
    NotApplicable,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountingBook {
    Ac,
    Fvoci,
    Fvtpl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PositionHoldingForm {
    Owned,
    RepoSold,
    ReverseRepoCollateral,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountingClassification {
    state: AccountingClassificationState,
    book: Option<AccountingBook>,
}

impl AccountingClassification {
    pub fn new(
        state: AccountingClassificationState,
        book: Option<AccountingBook>,
    ) -> DomainResult<Self> {
        if matches!(state, AccountingClassificationState::Classified) != book.is_some() {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self { state, book })
    }

    pub fn state(&self) -> AccountingClassificationState {
        self.state
    }

    pub fn book(&self) -> Option<AccountingBook> {
        self.book
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Position {
    position_id: Ulid,
    instrument_ref: VersionRef,
    quantity: DecimalValue,
    economic_value: DecimalValue,
    economic_pnl: DecimalValue,
    accounting_pnl: DecimalValue,
    capital_requirement: DecimalValue,
    accounting_classification: AccountingClassification,
    holding_form: PositionHoldingForm,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionInput {
    pub position_id: Ulid,
    pub instrument_ref: VersionRef,
    pub quantity: DecimalValue,
    pub economic_value: DecimalValue,
    pub economic_pnl: DecimalValue,
    pub accounting_pnl: DecimalValue,
    pub capital_requirement: DecimalValue,
    pub accounting_classification: AccountingClassification,
    pub holding_form: PositionHoldingForm,
}

impl Position {
    pub fn new(input: PositionInput) -> DomainResult<Self> {
        if input.economic_value.unit() != input.economic_pnl.unit()
            || input.economic_pnl.unit() != input.accounting_pnl.unit()
            || input.accounting_pnl.unit() != input.capital_requirement.unit()
        {
            return Err(DomainErrorCode::InvalidUnit);
        }
        Ok(Self {
            position_id: input.position_id,
            instrument_ref: input.instrument_ref,
            quantity: input.quantity,
            economic_value: input.economic_value,
            economic_pnl: input.economic_pnl,
            accounting_pnl: input.accounting_pnl,
            capital_requirement: input.capital_requirement,
            accounting_classification: input.accounting_classification,
            holding_form: input.holding_form,
        })
    }

    pub fn id(&self) -> &Ulid {
        &self.position_id
    }
    pub fn instrument_ref(&self) -> &VersionRef {
        &self.instrument_ref
    }
    pub fn quantity(&self) -> &DecimalValue {
        &self.quantity
    }
    pub fn economic_value(&self) -> &DecimalValue {
        &self.economic_value
    }
    pub fn economic_pnl(&self) -> &DecimalValue {
        &self.economic_pnl
    }
    pub fn accounting_pnl(&self) -> &DecimalValue {
        &self.accounting_pnl
    }
    pub fn capital_requirement(&self) -> &DecimalValue {
        &self.capital_requirement
    }
    pub fn accounting_classification(&self) -> &AccountingClassification {
        &self.accounting_classification
    }
    pub fn holding_form(&self) -> PositionHoldingForm {
        self.holding_form
    }

    pub fn includes_position_exposure(&self) -> bool {
        !matches!(
            self.holding_form,
            PositionHoldingForm::ReverseRepoCollateral
        )
    }

    pub fn includes_available_liquidity(&self) -> bool {
        matches!(self.holding_form, PositionHoldingForm::Owned)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionSnapshot {
    snapshot_id: Ulid,
    owner: OwnerRef,
    subject_ref: VersionRef,
    observed_at: MarketTime,
    visible_at: MarketTime,
    content_hash: ContentHash,
    lineage: Vec<LineageRef>,
    positions: Vec<Position>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionSnapshotInput {
    pub snapshot_id: Ulid,
    pub owner: OwnerRef,
    pub subject_ref: VersionRef,
    pub observed_at: MarketTime,
    pub visible_at: MarketTime,
    pub content_hash: ContentHash,
    pub lineage: Vec<LineageRef>,
    pub positions: Vec<Position>,
}

impl PositionSnapshot {
    pub fn new(input: PositionSnapshotInput) -> DomainResult<Self> {
        if input.observed_at.instant() > input.visible_at.instant() {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        if input.lineage.is_empty() {
            return Err(DomainErrorCode::BrokenLineage);
        }
        if input
            .positions
            .windows(2)
            .any(|pair| pair[0].id() >= pair[1].id())
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        let actual = ContentHash::digest(&canonical_bytes(&input));
        if input.content_hash != actual {
            return Err(DomainErrorCode::ContentHashMismatch);
        }
        Ok(Self {
            snapshot_id: input.snapshot_id,
            owner: input.owner,
            subject_ref: input.subject_ref,
            observed_at: input.observed_at,
            visible_at: input.visible_at,
            content_hash: input.content_hash,
            lineage: input.lineage,
            positions: input.positions,
        })
    }

    pub fn content_hash_for(input: &PositionSnapshotInput) -> ContentHash {
        ContentHash::digest(&canonical_bytes(input))
    }

    /// Returns the deterministic payload whose digest is this snapshot's content hash.
    #[must_use]
    pub fn canonical_payload(&self) -> Vec<u8> {
        canonical_bytes(&PositionSnapshotInput {
            snapshot_id: self.snapshot_id.clone(),
            owner: self.owner.clone(),
            subject_ref: self.subject_ref.clone(),
            observed_at: self.observed_at.clone(),
            visible_at: self.visible_at.clone(),
            content_hash: self.content_hash.clone(),
            lineage: self.lineage.clone(),
            positions: self.positions.clone(),
        })
    }

    pub fn id(&self) -> &Ulid {
        &self.snapshot_id
    }
    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }
    pub fn subject_ref(&self) -> &VersionRef {
        &self.subject_ref
    }
    pub fn observed_at(&self) -> &MarketTime {
        &self.observed_at
    }
    pub fn visible_at(&self) -> &MarketTime {
        &self.visible_at
    }
    pub fn positions(&self) -> &[Position] {
        &self.positions
    }
}

impl ContentAddressed for PositionSnapshot {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

impl Lineaged for PositionSnapshot {
    fn lineage(&self) -> &[LineageRef] {
        &self.lineage
    }
}

fn canonical_bytes(input: &PositionSnapshotInput) -> Vec<u8> {
    let mut bytes = Vec::new();
    append(&mut bytes, input.snapshot_id.as_str());
    append(&mut bytes, input.owner.tenant_id().as_str());
    append(&mut bytes, input.owner.owner_id().as_str());
    append(&mut bytes, input.subject_ref.id().as_str());
    append(&mut bytes, &input.subject_ref.version().get().to_string());
    append(
        &mut bytes,
        &input
            .observed_at
            .instant()
            .timestamp_nanos_opt()
            .unwrap_or_default()
            .to_string(),
    );
    append(&mut bytes, input.observed_at.market_timezone());
    append(
        &mut bytes,
        &input.observed_at.local_trading_date().to_string(),
    );
    append(
        &mut bytes,
        &input
            .visible_at
            .instant()
            .timestamp_nanos_opt()
            .unwrap_or_default()
            .to_string(),
    );
    append(&mut bytes, input.visible_at.market_timezone());
    append(
        &mut bytes,
        &input.visible_at.local_trading_date().to_string(),
    );
    for lineage in &input.lineage {
        append(&mut bytes, lineage.object_id().as_str());
        append(
            &mut bytes,
            &lineage
                .version()
                .map(|version| version.get().to_string())
                .unwrap_or_default(),
        );
        match lineage.content_hash() {
            Some(hash) => append_bytes(&mut bytes, hash.as_bytes()),
            None => append_bytes(&mut bytes, &[]),
        }
    }
    for position in &input.positions {
        append(&mut bytes, position.id().as_str());
        append(&mut bytes, position.instrument_ref().id().as_str());
        append(
            &mut bytes,
            &position.instrument_ref().version().get().to_string(),
        );
        for value in [
            position.quantity(),
            position.economic_value(),
            position.economic_pnl(),
            position.accounting_pnl(),
            position.capital_requirement(),
        ] {
            append(&mut bytes, value.coefficient());
            append(&mut bytes, &value.scale().to_string());
            append(&mut bytes, value.unit().unit_id().as_str());
            append(&mut bytes, &value.unit().version().get().to_string());
        }
        append(
            &mut bytes,
            &format!(
                "{:?}{:?}{:?}",
                position.accounting_classification().state(),
                position.accounting_classification().book(),
                position.holding_form()
            ),
        );
    }
    bytes
}

fn append(bytes: &mut Vec<u8>, value: &str) {
    append_bytes(bytes, value.as_bytes());
}

fn append_bytes(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}
