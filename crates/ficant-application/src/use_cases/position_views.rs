use ficant_domain::ContentAddressed;
use ficant_domain::primitives::{ContentHash, DecimalValue, Ulid};
use ficant_domain::research::{
    AccountingClassificationState, CoverageDeclaration, PositionHoldingForm, PositionSnapshot,
};

use crate::ports::ApplicationResult;
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionView {
    pub position_id: Ulid,
    pub economic_value: DecimalValue,
    pub economic_pnl: DecimalValue,
    pub accounting_pnl: DecimalValue,
    pub included_in_position_exposure: bool,
    pub included_in_available_liquidity: bool,
    pub collateral_fact: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionViews {
    pub snapshot: PositionSnapshot,
    pub positions: Vec<PositionView>,
    pub coverage: CoverageDeclaration,
    pub content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapitalUse {
    pub snapshot: PositionSnapshot,
    pub total_capital_requirement: DecimalValue,
    pub coverage: CoverageDeclaration,
    pub content_hash: ContentHash,
}

pub fn project_position_views(snapshot: PositionSnapshot) -> ApplicationResult<PositionViews> {
    let positions = snapshot
        .positions()
        .iter()
        .map(|position| PositionView {
            position_id: position.id().clone(),
            economic_value: position.economic_value().clone(),
            economic_pnl: position.economic_pnl().clone(),
            accounting_pnl: position.accounting_pnl().clone(),
            included_in_position_exposure: position.includes_position_exposure(),
            included_in_available_liquidity: position.includes_available_liquidity(),
            collateral_fact: matches!(
                position.holding_form(),
                PositionHoldingForm::ReverseRepoCollateral
            ),
        })
        .collect::<Vec<_>>();
    let coverage = complete_coverage(&snapshot)?;
    let content_hash = position_views_hash(&snapshot, &positions, &coverage);
    Ok(PositionViews {
        snapshot,
        positions,
        coverage,
        content_hash,
    })
}

pub fn calculate_capital_use(snapshot: PositionSnapshot) -> ApplicationResult<CapitalUse> {
    let unknown = snapshot
        .positions()
        .iter()
        .filter(|position| {
            matches!(
                position.accounting_classification().state(),
                AccountingClassificationState::Unknown
            )
        })
        .map(|position| position.id().as_str().to_owned())
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(ApplicationError::unknown_accounting_positions(unknown));
    }
    let mut values = snapshot.positions().iter();
    let first = values
        .next()
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?
        .capital_requirement()
        .clone();
    let total = values.try_fold(first, |total, position| {
        total
            .checked_add(position.capital_requirement())
            .map_err(map_domain_error)
    })?;
    let coverage = complete_coverage(&snapshot)?;
    let content_hash = capital_use_hash(&snapshot, &total, &coverage);
    Ok(CapitalUse {
        snapshot,
        total_capital_requirement: total,
        coverage,
        content_hash,
    })
}

fn complete_coverage(snapshot: &PositionSnapshot) -> ApplicationResult<CoverageDeclaration> {
    let position_ids = snapshot
        .positions()
        .iter()
        .map(|position| position.id().clone())
        .collect::<Vec<_>>();
    CoverageDeclaration::for_complete_positions(snapshot.positions(), &position_ids, None, 0)
        .map_err(map_domain_error)
}

fn position_views_hash(
    snapshot: &PositionSnapshot,
    positions: &[PositionView],
    coverage: &CoverageDeclaration,
) -> ContentHash {
    let mut bytes = Vec::new();
    append(&mut bytes, b"ficant.research.position-views.v1");
    append(&mut bytes, snapshot.content_hash().as_bytes());
    for position in positions {
        append(&mut bytes, position.position_id.as_str().as_bytes());
        append_decimal(&mut bytes, &position.economic_value);
        append_decimal(&mut bytes, &position.economic_pnl);
        append_decimal(&mut bytes, &position.accounting_pnl);
        append(
            &mut bytes,
            &[
                u8::from(position.included_in_position_exposure),
                u8::from(position.included_in_available_liquidity),
                u8::from(position.collateral_fact),
            ],
        );
    }
    append(&mut bytes, &coverage.canonical_bytes());
    ContentHash::digest(&bytes)
}

fn capital_use_hash(
    snapshot: &PositionSnapshot,
    total: &DecimalValue,
    coverage: &CoverageDeclaration,
) -> ContentHash {
    let mut bytes = Vec::new();
    append(&mut bytes, b"ficant.research.capital-use.v1");
    append(&mut bytes, snapshot.content_hash().as_bytes());
    append_decimal(&mut bytes, total);
    append(&mut bytes, &coverage.canonical_bytes());
    ContentHash::digest(&bytes)
}

fn append_decimal(bytes: &mut Vec<u8>, value: &DecimalValue) {
    append(bytes, value.coefficient().as_bytes());
    append(bytes, &value.scale().to_be_bytes());
    append(bytes, value.unit().unit_id().as_str().as_bytes());
    append(bytes, &value.unit().version().get().to_be_bytes());
}

fn append(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}
