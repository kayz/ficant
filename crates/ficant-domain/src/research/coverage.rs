use std::collections::{BTreeMap, BTreeSet};

use crate::market::PriceSourceType;
use crate::primitives::{DecimalValue, Ulid, UnitRef};
use crate::{DomainErrorCode, DomainResult};

use super::{Position, PriceSourceSummary};

/// The exact imported and participating boundary for one multi-position result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoverageDeclaration {
    imported_position_count: u64,
    participating_position_count: u64,
    imported_gross_economic_value_by_unit: Vec<DecimalValue>,
    participating_gross_economic_value_by_unit: Vec<DecimalValue>,
    missing_critical_field_record_count: u64,
    source_confidence: Option<PriceSourceSummary>,
    distinct_external_data_source_version_count: u64,
}

impl CoverageDeclaration {
    /// Derives a complete-input declaration from one verified position snapshot.
    ///
    /// Participating ids must be unique, sorted, and a non-empty subset of the imported
    /// positions. Current R5b result paths fail closed on critical input omissions, so this
    /// constructor fixes the missing-critical count at zero.
    pub fn for_complete_positions(
        imported: &[Position],
        participating_position_ids: &[Ulid],
        source_confidence: Option<PriceSourceSummary>,
        distinct_external_data_source_version_count: u64,
    ) -> DomainResult<Self> {
        if imported.is_empty()
            || participating_position_ids.is_empty()
            || participating_position_ids
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err(DomainErrorCode::InvalidValue);
        }

        let participating_ids = participating_position_ids.iter().collect::<BTreeSet<_>>();
        let participating = imported
            .iter()
            .filter(|position| participating_ids.contains(position.id()))
            .collect::<Vec<_>>();
        if participating.len() != participating_position_ids.len() {
            return Err(DomainErrorCode::InvalidValue);
        }

        validate_source_confidence(
            source_confidence.as_ref(),
            distinct_external_data_source_version_count,
        )?;
        let imported_position_count =
            u64::try_from(imported.len()).map_err(|_| DomainErrorCode::InvalidValue)?;
        let participating_position_count =
            u64::try_from(participating.len()).map_err(|_| DomainErrorCode::InvalidValue)?;
        let imported_gross_economic_value_by_unit = aggregate_gross(imported.iter())?;
        let participating_gross_economic_value_by_unit = aggregate_gross(participating)?;

        for participating_value in &participating_gross_economic_value_by_unit {
            let imported_value = imported_gross_economic_value_by_unit
                .iter()
                .find(|value| value.unit() == participating_value.unit())
                .ok_or(DomainErrorCode::InvalidUnit)?;
            if participating_value.compare(imported_value)?.is_gt() {
                return Err(DomainErrorCode::InvalidValue);
            }
        }

        Ok(Self {
            imported_position_count,
            participating_position_count,
            imported_gross_economic_value_by_unit,
            participating_gross_economic_value_by_unit,
            missing_critical_field_record_count: 0,
            source_confidence,
            distinct_external_data_source_version_count,
        })
    }

    pub const fn imported_position_count(&self) -> u64 {
        self.imported_position_count
    }

    pub const fn participating_position_count(&self) -> u64 {
        self.participating_position_count
    }

    pub fn imported_gross_economic_value_by_unit(&self) -> &[DecimalValue] {
        &self.imported_gross_economic_value_by_unit
    }

    pub fn participating_gross_economic_value_by_unit(&self) -> &[DecimalValue] {
        &self.participating_gross_economic_value_by_unit
    }

    pub const fn missing_critical_field_record_count(&self) -> u64 {
        self.missing_critical_field_record_count
    }

    pub fn source_confidence(&self) -> Option<&PriceSourceSummary> {
        self.source_confidence.as_ref()
    }

    pub const fn distinct_external_data_source_version_count(&self) -> u64 {
        self.distinct_external_data_source_version_count
    }

    /// Returns the deterministic coverage payload committed by aggregate result hashes.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        append(&mut bytes, &self.imported_position_count.to_be_bytes());
        append(&mut bytes, &self.participating_position_count.to_be_bytes());
        append_amounts(&mut bytes, &self.imported_gross_economic_value_by_unit);
        append_amounts(&mut bytes, &self.participating_gross_economic_value_by_unit);
        append(
            &mut bytes,
            &self.missing_critical_field_record_count.to_be_bytes(),
        );
        match &self.source_confidence {
            Some(summary) => {
                append(&mut bytes, &[1]);
                for count in summary.counts() {
                    append(&mut bytes, &[price_source_type_code(count.source_type())]);
                    append(&mut bytes, &count.record_count().to_be_bytes());
                }
                append(&mut bytes, &[u8::from(summary.mixed())]);
            }
            None => append(&mut bytes, &[0]),
        }
        append(
            &mut bytes,
            &self
                .distinct_external_data_source_version_count
                .to_be_bytes(),
        );
        bytes
    }
}

fn validate_source_confidence(
    source_confidence: Option<&PriceSourceSummary>,
    distinct_external_data_source_version_count: u64,
) -> DomainResult<()> {
    let Some(summary) = source_confidence else {
        return (distinct_external_data_source_version_count == 0)
            .then_some(())
            .ok_or(DomainErrorCode::InvalidValue);
    };
    let external_record_count = summary
        .counts()
        .iter()
        .filter(|count| count.source_type() != PriceSourceType::CurveInterpolation)
        .try_fold(0_u64, |total, count| {
            total
                .checked_add(count.record_count())
                .ok_or(DomainErrorCode::InvalidValue)
        })?;
    if external_record_count == 0 {
        if distinct_external_data_source_version_count != 0 {
            return Err(DomainErrorCode::InvalidValue);
        }
    } else if distinct_external_data_source_version_count == 0
        || distinct_external_data_source_version_count > external_record_count
    {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(())
}

fn aggregate_gross<'a>(
    positions: impl IntoIterator<Item = &'a Position>,
) -> DomainResult<Vec<DecimalValue>> {
    let mut totals = BTreeMap::<UnitRef, DecimalValue>::new();
    for position in positions {
        let value = gross(position.economic_value())?;
        match totals.get_mut(value.unit()) {
            Some(total) => *total = total.checked_add(&value)?,
            None => {
                totals.insert(value.unit().clone(), value);
            }
        }
    }
    if totals.is_empty() {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(totals.into_values().collect())
}

fn gross(value: &DecimalValue) -> DomainResult<DecimalValue> {
    let coefficient = value
        .coefficient()
        .parse::<i128>()
        .map_err(|_| DomainErrorCode::InvalidValue)?
        .checked_abs()
        .ok_or(DomainErrorCode::InvalidValue)?;
    DecimalValue::new(coefficient.to_string(), value.scale(), value.unit().clone())
}

fn append_amounts(bytes: &mut Vec<u8>, values: &[DecimalValue]) {
    for value in values {
        append(bytes, value.coefficient().as_bytes());
        append(bytes, &value.scale().to_be_bytes());
        append(bytes, value.unit().unit_id().as_str().as_bytes());
        append(bytes, &value.unit().version().get().to_be_bytes());
    }
}

fn append(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}

const fn price_source_type_code(source_type: PriceSourceType) -> u8 {
    match source_type {
        PriceSourceType::RealTrade => 1,
        PriceSourceType::ActiveQuote => 2,
        PriceSourceType::ModelValuation => 3,
        PriceSourceType::CurveInterpolation => 4,
    }
}
