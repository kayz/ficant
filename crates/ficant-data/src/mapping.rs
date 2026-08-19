use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, VersionRef,
};

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
    mapping_id: Ulid,
    owner: OwnerRef,
    source: VersionRef,
    entries: Vec<InstrumentMappingEntry>,
    content_hash: ContentHash,
}

impl InstrumentMapping {
    pub fn new(
        mapping_id: Ulid,
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
        let content_hash = mapping_content_hash(&mapping_id, &owner, &source, &entries);
        Ok(Self {
            mapping_id,
            owner,
            source,
            entries,
            content_hash,
        })
    }

    pub fn id(&self) -> &Ulid {
        &self.mapping_id
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn source(&self) -> &VersionRef {
        &self.source
    }

    pub fn entries(&self) -> &[InstrumentMappingEntry] {
        &self.entries
    }

    pub fn contract_hash(&self) -> ContentHash {
        self.content_hash.clone()
    }

    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
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

fn mapping_content_hash(
    mapping_id: &Ulid,
    owner: &OwnerRef,
    source: &VersionRef,
    entries: &[InstrumentMappingEntry],
) -> ContentHash {
    let mut bytes = b"ficant-instrument-mapping/v2\0".to_vec();
    append(&mut bytes, mapping_id.as_str());
    append(&mut bytes, owner.tenant_id().as_str());
    append(&mut bytes, owner.owner_id().as_str());
    append(&mut bytes, source.id().as_str());
    bytes.extend_from_slice(&source.version().get().to_be_bytes());
    for entry in entries {
        append(&mut bytes, entry.source_instrument_key());
        append_market_time(&mut bytes, entry.effective().from());
        append_market_time(&mut bytes, entry.effective().to());
        append(&mut bytes, entry.instrument().id().as_str());
        bytes.extend_from_slice(&entry.instrument().version().get().to_be_bytes());
    }
    ContentHash::digest(&bytes)
}

fn append_market_time(bytes: &mut Vec<u8>, value: &MarketTime) {
    bytes.extend_from_slice(&value.instant().timestamp().to_be_bytes());
    bytes.extend_from_slice(&value.instant().timestamp_subsec_nanos().to_be_bytes());
    append(bytes, value.market_timezone());
    append(bytes, &value.local_trading_date().to_string());
}

fn append(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(
        &u64::try_from(value.len())
            .expect("mapping token length fits u64")
            .to_be_bytes(),
    );
    bytes.extend_from_slice(value.as_bytes());
}
