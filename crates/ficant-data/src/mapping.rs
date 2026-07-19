use ficant_domain::primitives::{EffectivePeriod, MarketTime, OwnerRef, VersionRef};

use crate::{DataError, DataResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstrumentMappingEntry {
    source_instrument_key: String,
    effective: EffectivePeriod,
    instrument: VersionRef,
}

impl InstrumentMappingEntry {
    pub fn new(
        source_instrument_key: impl Into<String>,
        effective: EffectivePeriod,
        instrument: VersionRef,
    ) -> DataResult<Self> {
        let source_instrument_key = source_instrument_key.into();
        if source_instrument_key.trim().is_empty()
            || source_instrument_key != source_instrument_key.trim()
            || source_instrument_key.len() > 128
        {
            return Err(DataError::InvalidConfiguration);
        }
        Ok(Self {
            source_instrument_key,
            effective,
            instrument,
        })
    }

    pub fn source_instrument_key(&self) -> &str {
        &self.source_instrument_key
    }

    pub fn effective(&self) -> &EffectivePeriod {
        &self.effective
    }

    pub fn instrument(&self) -> &VersionRef {
        &self.instrument
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstrumentMapping {
    owner: OwnerRef,
    source: VersionRef,
    entries: Vec<InstrumentMappingEntry>,
}

impl InstrumentMapping {
    pub fn new(
        owner: OwnerRef,
        source: VersionRef,
        mut entries: Vec<InstrumentMappingEntry>,
    ) -> DataResult<Self> {
        if entries.is_empty() {
            return Err(DataError::InvalidConfiguration);
        }
        entries.sort_by(|left, right| {
            left.source_instrument_key
                .cmp(&right.source_instrument_key)
                .then_with(|| {
                    left.effective
                        .from()
                        .instant()
                        .cmp(&right.effective.from().instant())
                })
        });
        for pair in entries.windows(2) {
            let left = &pair[0];
            let right = &pair[1];
            if left.source_instrument_key == right.source_instrument_key
                && left.effective.to().instant() > right.effective.from().instant()
            {
                return Err(DataError::InvalidConfiguration);
            }
        }
        Ok(Self {
            owner,
            source,
            entries,
        })
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn source(&self) -> &VersionRef {
        &self.source
    }

    pub fn resolve(
        &self,
        source_instrument_key: &str,
        observed_at: &MarketTime,
    ) -> DataResult<&VersionRef> {
        let mut resolved = self.entries.iter().filter(|entry| {
            entry.source_instrument_key == source_instrument_key
                && entry.effective.from().instant() <= observed_at.instant()
                && observed_at.instant() < entry.effective.to().instant()
        });
        let value = resolved.next().ok_or(DataError::QualityRuleFailed)?;
        if resolved.next().is_some() {
            return Err(DataError::QualityRuleFailed);
        }
        Ok(&value.instrument)
    }
}
