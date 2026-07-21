use ficant_application::ports::{
    DefinitionValue, InstrumentDefinition, InstrumentSubtype, MarketFact, SnapshotValue,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::market::{
    ArtifactInputKind, Bond, Calendar, CalendarInput, CalendarSession, Cashflow, CashflowInput,
    CashflowType, CurveSnapshot, CurveSnapshotInput, FactSource, FuturesContract, Instrument,
    InstrumentInput, InstrumentKind, MarketRulePack, MarketRulePackInput, Quote, QuoteInput, Trade,
    TradeInput, Unit, UnitInput, Valuation, ValuationInput, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, MarketTime, OwnerRef, Ulid, UnitRef, Version,
    VersionRef,
};
use ficant_domain::research::{
    Artifact, ArtifactKind, DataSnapshot, DataSnapshotInput, ExperimentRun, ExperimentRunInput,
    JournalEventType, RunJournal, RunJournalInput, RunState, SignalSet, SignalSetInput,
    UniverseSnapshot,
};
use ficant_domain::{ContentAddressed, Lineaged, VersionedDefinition};
use sqlx::types::chrono::{DateTime, NaiveDate, NaiveTime, Utc};

type CodecResult<T> = Result<T, ApplicationError>;

pub(crate) fn encode_definition(value: &DefinitionValue) -> Vec<u8> {
    let mut encoder = Encoder::new();
    match value {
        DefinitionValue::Instrument(value) => {
            encoder.u8(1);
            encode_instrument(&mut encoder, value.instrument());
            match value.subtype() {
                None => encoder.u8(0),
                Some(InstrumentSubtype::Bond(value)) => {
                    encoder.u8(1);
                    encoder.string(&value.issue_date().to_string());
                    encoder.string(&value.maturity_date().to_string());
                    encode_decimal(&mut encoder, value.face_value());
                }
                Some(InstrumentSubtype::FuturesContract(value)) => {
                    encoder.u8(2);
                    encode_market_time(&mut encoder, value.last_trade_time());
                    encode_market_time(&mut encoder, value.expiry_time());
                    encode_market_time(&mut encoder, value.settlement_time());
                    encode_decimal(&mut encoder, value.multiplier());
                    encode_version_ref(&mut encoder, value.rule_pack());
                }
            }
        }
        DefinitionValue::Calendar(value) => {
            encoder.u8(4);
            encoder.string(value.identity());
            encoder.u64(value.version());
            encode_owner(&mut encoder, value.owner());
            encoder.string(value.market());
            encoder.string(value.market_timezone());
            encode_period(&mut encoder, value.effective());
            encoder.len(value.sessions().len());
            for session in value.sessions() {
                encoder.string(&session.local_date().to_string());
                match (session.open_local_time(), session.close_local_time()) {
                    (Some(open), Some(close)) => {
                        encoder.bool(true);
                        encoder.string(&open.to_string());
                        encoder.string(&close.to_string());
                    }
                    (None, None) => encoder.bool(false),
                    _ => unreachable!("CalendarSession construction keeps open/close paired"),
                }
            }
        }
        DefinitionValue::Unit(value) => {
            encoder.u8(5);
            encoder.string(value.identity());
            encoder.u64(value.version());
            encode_owner(&mut encoder, value.owner());
            encoder.string(value.code());
            encoder.string(value.dimension());
            encoder.u32(value.scale());
            encoder.u32(value.precision());
        }
        DefinitionValue::MarketRulePack(value) => {
            encoder.u8(6);
            encoder.string(value.identity());
            encoder.u64(value.version());
            encode_owner(&mut encoder, value.owner());
            encoder.string(value.market());
            encoder.string(value.rule_type());
            encoder.string(value.source());
            encode_period(&mut encoder, value.effective());
            encoder.u8(verification_status_code(value.verification_status()));
            encoder.bytes(value.content_hash().as_bytes());
        }
    }
    encoder.finish()
}

pub(crate) fn decode_definition(bytes: &[u8]) -> CodecResult<DefinitionValue> {
    let mut decoder = Decoder::new(bytes)?;
    let value = match decoder.u8()? {
        1 => {
            let instrument = decode_instrument(&mut decoder)?;
            let subtype = match decoder.u8()? {
                0 => None,
                1 => Some(InstrumentSubtype::Bond(
                    Bond::new(
                        &instrument,
                        parse_date(&decoder.string()?)?,
                        parse_date(&decoder.string()?)?,
                        decode_decimal(&mut decoder)?,
                    )
                    .map_err(ficant_application::map_domain_error)?,
                )),
                2 => Some(InstrumentSubtype::FuturesContract(
                    FuturesContract::new(
                        &instrument,
                        decode_market_time(&mut decoder)?,
                        decode_market_time(&mut decoder)?,
                        decode_market_time(&mut decoder)?,
                        decode_decimal(&mut decoder)?,
                        decode_version_ref(&mut decoder)?,
                    )
                    .map_err(ficant_application::map_domain_error)?,
                )),
                _ => return Err(codec_error()),
            };
            DefinitionValue::Instrument(InstrumentDefinition::new(instrument, subtype)?)
        }
        4 => {
            let calendar_id = decode_ulid(&mut decoder)?;
            let version = decode_version(&mut decoder)?;
            let owner = decode_owner(&mut decoder)?;
            let market = decoder.string()?;
            let market_timezone = decoder.string()?;
            let effective = decode_period(&mut decoder)?;
            let session_count = decoder.len()?;
            let mut sessions = Vec::with_capacity(session_count);
            for _ in 0..session_count {
                let local_date = parse_date(&decoder.string()?)?;
                let session = if decoder.bool()? {
                    CalendarSession::open(
                        local_date,
                        parse_time(&decoder.string()?)?,
                        parse_time(&decoder.string()?)?,
                    )
                    .map_err(ficant_application::map_domain_error)?
                } else {
                    CalendarSession::closed(local_date)
                };
                sessions.push(session);
            }
            DefinitionValue::Calendar(
                Calendar::new(CalendarInput {
                    calendar_id,
                    version,
                    owner,
                    market,
                    market_timezone,
                    effective,
                    sessions,
                })
                .map_err(ficant_application::map_domain_error)?,
            )
        }
        5 => DefinitionValue::Unit(
            Unit::new(UnitInput {
                unit_id: decode_ulid(&mut decoder)?,
                version: decode_version(&mut decoder)?,
                owner: decode_owner(&mut decoder)?,
                code: decoder.string()?,
                dimension: decoder.string()?,
                scale: decoder.u32()?,
                precision: decoder.u32()?,
            })
            .map_err(ficant_application::map_domain_error)?,
        ),
        6 => DefinitionValue::MarketRulePack(
            MarketRulePack::new(MarketRulePackInput {
                rule_pack_id: decode_ulid(&mut decoder)?,
                version: decode_version(&mut decoder)?,
                owner: decode_owner(&mut decoder)?,
                market: decoder.string()?,
                rule_type: decoder.string()?,
                source: decoder.string()?,
                effective: decode_period(&mut decoder)?,
                verification_status: decode_verification_status(decoder.u8()?)?,
                content_hash: decode_hash(&mut decoder)?,
            })
            .map_err(ficant_application::map_domain_error)?,
        ),
        _ => return Err(codec_error()),
    };
    decoder.end()?;
    Ok(value)
}

pub(crate) fn encode_fact(value: &MarketFact) -> Vec<u8> {
    let mut encoder = Encoder::new();
    match value {
        MarketFact::Cashflow(value) => {
            encoder.u8(1);
            encoder.string(value.id().as_str());
            encode_version_ref(&mut encoder, value.bond());
            encode_market_time(&mut encoder, value.payment_time());
            encode_decimal(&mut encoder, value.amount());
            encode_owner(&mut encoder, value.owner());
            encode_source(&mut encoder, value.source());
            encode_optional_id(&mut encoder, value.supersedes_id());
            encoder.u8(cashflow_type_code(value.cashflow_type()));
            encoder.string(value.schedule_id());
            encoder.u64(value.sequence());
        }
        MarketFact::Quote(value) => {
            encoder.u8(2);
            encoder.string(value.id().as_str());
            encode_version_ref(&mut encoder, value.instrument());
            encode_owner(&mut encoder, value.owner());
            encode_source(&mut encoder, value.source());
            encode_market_time(&mut encoder, value.observed_at());
            encode_market_time(&mut encoder, value.received_at());
            encode_optional_decimal(&mut encoder, value.bid());
            encode_optional_decimal(&mut encoder, value.ask());
            encode_optional_id(&mut encoder, value.supersedes_id());
        }
        MarketFact::Trade(value) => {
            encoder.u8(3);
            encoder.string(value.id().as_str());
            encode_version_ref(&mut encoder, value.instrument());
            encode_owner(&mut encoder, value.owner());
            encode_source(&mut encoder, value.source());
            encode_market_time(&mut encoder, value.executed_at());
            encode_decimal(&mut encoder, value.price());
            encode_decimal(&mut encoder, value.quantity());
            encode_optional_id(&mut encoder, value.supersedes_id());
        }
        MarketFact::Valuation(value) => {
            encoder.u8(4);
            encoder.string(value.id().as_str());
            encode_version_ref(&mut encoder, value.instrument());
            encode_owner(&mut encoder, value.owner());
            encode_source(&mut encoder, value.source());
            encode_market_time(&mut encoder, value.valuation_at());
            encoder.string(value.method());
            encode_version_ref(&mut encoder, value.rule_pack());
            encoder.len(value.values().len());
            for item in value.values() {
                encode_decimal(&mut encoder, item);
            }
            encode_optional_id(&mut encoder, value.supersedes_id());
        }
    }
    encoder.finish()
}

pub(crate) fn decode_fact(bytes: &[u8]) -> CodecResult<MarketFact> {
    let mut decoder = Decoder::new(bytes)?;
    let value = match decoder.u8()? {
        1 => MarketFact::Cashflow(
            Cashflow::new(CashflowInput {
                cashflow_id: decode_ulid(&mut decoder)?,
                bond: decode_version_ref(&mut decoder)?,
                payment_time: decode_market_time(&mut decoder)?,
                amount: decode_decimal(&mut decoder)?,
                owner: decode_owner(&mut decoder)?,
                source: decode_source(&mut decoder)?,
                supersedes_id: decode_optional_id(&mut decoder)?,
                cashflow_type: decode_cashflow_type(decoder.u8()?)?,
                schedule_id: decoder.string()?,
                sequence: decoder.u64()?,
            })
            .map_err(ficant_application::map_domain_error)?,
        ),
        2 => MarketFact::Quote(
            Quote::new(QuoteInput {
                quote_id: decode_ulid(&mut decoder)?,
                instrument: decode_version_ref(&mut decoder)?,
                owner: decode_owner(&mut decoder)?,
                source: decode_source(&mut decoder)?,
                observed_at: decode_market_time(&mut decoder)?,
                received_at: decode_market_time(&mut decoder)?,
                bid: decode_optional_decimal(&mut decoder)?,
                ask: decode_optional_decimal(&mut decoder)?,
                supersedes_id: decode_optional_id(&mut decoder)?,
            })
            .map_err(ficant_application::map_domain_error)?,
        ),
        3 => MarketFact::Trade(
            Trade::new(TradeInput {
                trade_id: decode_ulid(&mut decoder)?,
                instrument: decode_version_ref(&mut decoder)?,
                owner: decode_owner(&mut decoder)?,
                source: decode_source(&mut decoder)?,
                executed_at: decode_market_time(&mut decoder)?,
                price: decode_decimal(&mut decoder)?,
                quantity: decode_decimal(&mut decoder)?,
                supersedes_id: decode_optional_id(&mut decoder)?,
            })
            .map_err(ficant_application::map_domain_error)?,
        ),
        4 => {
            let valuation_id = decode_ulid(&mut decoder)?;
            let instrument = decode_version_ref(&mut decoder)?;
            let owner = decode_owner(&mut decoder)?;
            let source = decode_source(&mut decoder)?;
            let valuation_at = decode_market_time(&mut decoder)?;
            let method = decoder.string()?;
            let rule_pack = decode_version_ref(&mut decoder)?;
            let value_count = decoder.len()?;
            let mut values = Vec::with_capacity(value_count);
            for _ in 0..value_count {
                values.push(decode_decimal(&mut decoder)?);
            }
            let supersedes_id = decode_optional_id(&mut decoder)?;
            MarketFact::Valuation(
                Valuation::new(ValuationInput {
                    valuation_id,
                    instrument,
                    owner,
                    source,
                    valuation_at,
                    method,
                    rule_pack,
                    values,
                    supersedes_id,
                })
                .map_err(ficant_application::map_domain_error)?,
            )
        }
        _ => return Err(codec_error()),
    };
    decoder.end()?;
    Ok(value)
}

pub(crate) fn encode_curve_snapshot(value: &CurveSnapshot) -> Vec<u8> {
    let mut encoder = Encoder::new();
    encoder.string(value.id().as_str());
    encode_owner(&mut encoder, value.owner());
    encode_market_time(&mut encoder, value.as_of());
    encode_unit_ref(&mut encoder, value.currency());
    encoder.string(value.curve_kind());
    encode_version_ref(&mut encoder, value.calendar());
    encode_version_ref(&mut encoder, value.rule_pack());
    encoder.string(value.point_schema());
    encoder.bytes(value.content_hash().as_bytes());
    encode_lineage(&mut encoder, value.lineage());
    encoder.u8(match value.input_kind() {
        ArtifactInputKind::ExternalFixture => 1,
    });
    encoder.finish()
}

pub(crate) fn decode_curve_snapshot(bytes: &[u8]) -> CodecResult<CurveSnapshot> {
    let mut decoder = Decoder::new(bytes)?;
    let input = CurveSnapshotInput {
        curve_snapshot_id: decode_ulid(&mut decoder)?,
        owner: decode_owner(&mut decoder)?,
        as_of: decode_market_time(&mut decoder)?,
        currency: decode_unit_ref(&mut decoder)?,
        curve_kind: decoder.string()?,
        calendar: decode_version_ref(&mut decoder)?,
        rule_pack: decode_version_ref(&mut decoder)?,
        point_schema: decoder.string()?,
        content_hash: decode_hash(&mut decoder)?,
        lineage: decode_lineage(&mut decoder)?,
        input_kind: match decoder.u8()? {
            1 => ArtifactInputKind::ExternalFixture,
            _ => return Err(codec_error()),
        },
    };
    decoder.end()?;
    CurveSnapshot::new(input).map_err(ficant_application::map_domain_error)
}

pub(crate) fn encode_snapshot(value: &SnapshotValue) -> Vec<u8> {
    let mut encoder = Encoder::new();
    match value {
        SnapshotValue::Data(value) => {
            encoder.u8(1);
            encoder.string(value.id().as_str());
            encode_owner(&mut encoder, value.owner());
            encode_market_time(&mut encoder, value.visible_at());
            encode_market_time(&mut encoder, value.as_of());
            encoder.bytes(value.schema_hash().as_bytes());
            encoder.bytes(value.manifest_hash().as_bytes());
            encoder.bytes(value.content_hash().as_bytes());
            encode_lineage(&mut encoder, value.lineage());
        }
        SnapshotValue::Universe(value) => {
            encoder.u8(2);
            encoder.string(value.id().as_str());
            encode_owner(&mut encoder, value.owner());
            encoder.len(value.instrument_versions().len());
            for instrument in value.instrument_versions() {
                encode_version_ref(&mut encoder, instrument);
            }
            encoder.bytes(value.filter_digest().as_bytes());
            encoder.bytes(value.content_hash().as_bytes());
            encode_lineage(&mut encoder, value.lineage());
        }
    }
    encoder.finish()
}

pub(crate) fn decode_snapshot(bytes: &[u8]) -> CodecResult<SnapshotValue> {
    let mut decoder = Decoder::new(bytes)?;
    let value = match decoder.u8()? {
        1 => SnapshotValue::Data(
            DataSnapshot::new(DataSnapshotInput {
                data_snapshot_id: decode_ulid(&mut decoder)?,
                owner: decode_owner(&mut decoder)?,
                visible_at: decode_market_time(&mut decoder)?,
                as_of: decode_market_time(&mut decoder)?,
                schema_hash: decode_hash(&mut decoder)?,
                manifest_hash: decode_hash(&mut decoder)?,
                blob_content_hash: decode_hash(&mut decoder)?,
                lineage: decode_lineage(&mut decoder)?,
            })
            .map_err(ficant_application::map_domain_error)?,
        ),
        2 => {
            let universe_snapshot_id = decode_ulid(&mut decoder)?;
            let owner = decode_owner(&mut decoder)?;
            let instrument_count = decoder.len()?;
            let mut instruments = Vec::with_capacity(instrument_count);
            for _ in 0..instrument_count {
                instruments.push(decode_version_ref(&mut decoder)?);
            }
            SnapshotValue::Universe(
                UniverseSnapshot::new(
                    universe_snapshot_id,
                    owner,
                    instruments,
                    decode_hash(&mut decoder)?,
                    decode_hash(&mut decoder)?,
                    decode_lineage(&mut decoder)?,
                )
                .map_err(ficant_application::map_domain_error)?,
            )
        }
        _ => return Err(codec_error()),
    };
    decoder.end()?;
    Ok(value)
}

pub(crate) fn encode_run(value: &ExperimentRun) -> Vec<u8> {
    let mut encoder = Encoder::new();
    encoder.string(value.id().as_str());
    encode_owner(&mut encoder, value.owner());
    encode_lineage_ref(&mut encoder, value.data_snapshot());
    encode_lineage_ref(&mut encoder, value.universe_snapshot());
    encoder.len(value.rule_packs().len());
    for rule_pack in value.rule_packs() {
        encode_version_ref(&mut encoder, rule_pack);
    }
    encoder.bytes(value.runtime_image_digest().as_bytes());
    encoder.bytes(value.parameters_hash().as_bytes());
    encoder.u64(value.seed());
    encoder.u8(run_state_code(value.state()));
    encoder.u64(value.revision());
    encoder.finish()
}

pub(crate) fn decode_run(bytes: &[u8]) -> CodecResult<ExperimentRun> {
    let mut decoder = Decoder::new(bytes)?;
    let experiment_run_id = decode_ulid(&mut decoder)?;
    let owner = decode_owner(&mut decoder)?;
    let data_snapshot = decode_lineage_ref(&mut decoder)?;
    let universe_snapshot = decode_lineage_ref(&mut decoder)?;
    let rule_pack_count = decoder.len()?;
    let mut rule_packs = Vec::with_capacity(rule_pack_count);
    for _ in 0..rule_pack_count {
        rule_packs.push(decode_version_ref(&mut decoder)?);
    }
    let runtime_image_digest = decode_hash(&mut decoder)?;
    let parameters_hash = decode_hash(&mut decoder)?;
    let seed = decoder.u64()?;
    let state = decode_run_state(decoder.u8()?)?;
    let revision = decoder.u64()?;
    decoder.end()?;
    let mut value = ExperimentRun::new(ExperimentRunInput {
        experiment_run_id,
        owner,
        data_snapshot,
        universe_snapshot,
        rule_packs,
        runtime_image_digest,
        parameters_hash,
        seed,
    })
    .map_err(ficant_application::map_domain_error)?;
    while value.revision() < revision {
        let next = if value.revision() + 1 == revision {
            state
        } else {
            RunState::Running
        };
        value = value
            .transition(next, value.revision())
            .map_err(ficant_application::map_domain_error)?;
    }
    if value.state() != state || value.revision() != revision {
        return Err(codec_error());
    }
    Ok(value)
}

pub(crate) fn encode_artifact(value: &Artifact) -> Vec<u8> {
    let mut encoder = Encoder::new();
    encoder.string(value.id().as_str());
    encode_owner(&mut encoder, value.owner());
    encoder.u8(artifact_kind_code(value.kind()));
    encoder.string(value.media_type());
    encoder.bytes(value.content_hash().as_bytes());
    encoder.u64(value.blob_size());
    encode_lineage(&mut encoder, value.lineage());
    encoder.finish()
}

pub(crate) fn decode_artifact(bytes: &[u8]) -> CodecResult<Artifact> {
    let mut decoder = Decoder::new(bytes)?;
    let value = Artifact::new(
        decode_ulid(&mut decoder)?,
        decode_owner(&mut decoder)?,
        decode_artifact_kind(decoder.u8()?)?,
        decoder.string()?,
        decode_hash(&mut decoder)?,
        decoder.u64()?,
        decode_lineage(&mut decoder)?,
    )
    .map_err(ficant_application::map_domain_error)?;
    decoder.end()?;
    Ok(value)
}

pub(crate) fn encode_signal(value: &SignalSet) -> Vec<u8> {
    let mut encoder = Encoder::new();
    encoder.string(value.id().as_str());
    encode_owner(&mut encoder, value.owner());
    encode_lineage_ref(&mut encoder, value.artifact());
    encoder.string(value.experiment_run_id().as_str());
    encode_lineage_ref(&mut encoder, value.data_snapshot());
    encode_lineage_ref(&mut encoder, value.universe_snapshot());
    encoder.len(value.rule_packs().len());
    for rule_pack in value.rule_packs() {
        encode_version_ref(&mut encoder, rule_pack);
    }
    encode_lineage(&mut encoder, value.input_artifacts());
    encode_period(&mut encoder, value.valid());
    encoder.finish()
}

pub(crate) fn decode_signal(bytes: &[u8]) -> CodecResult<SignalSet> {
    let mut decoder = Decoder::new(bytes)?;
    let signal_set_id = decode_ulid(&mut decoder)?;
    let owner = decode_owner(&mut decoder)?;
    let artifact = decode_lineage_ref(&mut decoder)?;
    let experiment_run_id = decode_ulid(&mut decoder)?;
    let data_snapshot = decode_lineage_ref(&mut decoder)?;
    let universe_snapshot = decode_lineage_ref(&mut decoder)?;
    let rule_pack_count = decoder.len()?;
    let mut rule_packs = Vec::with_capacity(rule_pack_count);
    for _ in 0..rule_pack_count {
        rule_packs.push(decode_version_ref(&mut decoder)?);
    }
    let input_artifacts = decode_lineage(&mut decoder)?;
    let valid = decode_period(&mut decoder)?;
    decoder.end()?;
    SignalSet::new(SignalSetInput {
        signal_set_id,
        owner,
        artifact,
        experiment_run_id,
        data_snapshot,
        universe_snapshot,
        rule_packs,
        input_artifacts,
        valid,
    })
    .map_err(ficant_application::map_domain_error)
}

pub(crate) fn encode_journal(value: &RunJournal) -> Vec<u8> {
    let mut encoder = Encoder::new();
    encoder.string(value.id().as_str());
    encoder.string(value.run_id().as_str());
    encoder.u64(value.sequence());
    encoder.u8(journal_event_type_code(value.event_type()));
    encode_market_time(&mut encoder, value.occurred_at());
    encoder.string(value.payload_type());
    encoder.string(value.payload_schema());
    encoder.bytes(value.payload());
    encode_optional_hash(&mut encoder, value.prev_hash());
    encoder.bytes(value.content_hash().as_bytes());
    encoder.finish()
}

pub(crate) fn decode_journal(bytes: &[u8]) -> CodecResult<RunJournal> {
    let mut decoder = Decoder::new(bytes)?;
    let input = RunJournalInput {
        journal_event_id: decode_ulid(&mut decoder)?,
        run_id: decode_ulid(&mut decoder)?,
        sequence: decoder.u64()?,
        event_type: decode_journal_event_type(decoder.u8()?)?,
        occurred_at: decode_market_time(&mut decoder)?,
        payload_type: decoder.string()?,
        payload_schema: decoder.string()?,
        payload: decoder.bytes()?,
        prev_hash: decode_optional_hash(&mut decoder)?,
    };
    let claimed_hash = decode_hash(&mut decoder)?;
    decoder.end()?;
    RunJournal::new(input, &claimed_hash).map_err(ficant_application::map_domain_error)
}

fn encode_instrument(encoder: &mut Encoder, value: &Instrument) {
    encoder.string(value.id().as_str());
    encoder.u64(value.version());
    encode_owner(encoder, value.owner());
    encoder.u8(instrument_kind_code(value.kind()));
    encoder.string(value.market());
    encoder.string(value.symbol());
    encode_unit_ref(encoder, value.currency());
    encode_version_ref(encoder, value.calendar());
}

fn decode_instrument(decoder: &mut Decoder<'_>) -> CodecResult<Instrument> {
    Instrument::new(InstrumentInput {
        instrument_id: decode_ulid(decoder)?,
        version: decode_version(decoder)?,
        owner: decode_owner(decoder)?,
        kind: decode_instrument_kind(decoder.u8()?)?,
        market: decoder.string()?,
        symbol: decoder.string()?,
        currency: decode_unit_ref(decoder)?,
        calendar: decode_version_ref(decoder)?,
    })
    .map_err(ficant_application::map_domain_error)
}

fn encode_owner(encoder: &mut Encoder, value: &OwnerRef) {
    encoder.string(value.tenant_id().as_str());
    encoder.string(value.owner_id().as_str());
}

fn decode_owner(decoder: &mut Decoder<'_>) -> CodecResult<OwnerRef> {
    Ok(OwnerRef::new(decode_ulid(decoder)?, decode_ulid(decoder)?))
}

fn encode_version_ref(encoder: &mut Encoder, value: &VersionRef) {
    encoder.string(value.id().as_str());
    encoder.u64(value.version().get());
}

fn decode_version_ref(decoder: &mut Decoder<'_>) -> CodecResult<VersionRef> {
    Ok(VersionRef::new(
        decode_ulid(decoder)?,
        decode_version(decoder)?,
    ))
}

fn encode_unit_ref(encoder: &mut Encoder, value: &UnitRef) {
    encoder.string(value.unit_id().as_str());
    encoder.u64(value.version().get());
}

fn decode_unit_ref(decoder: &mut Decoder<'_>) -> CodecResult<UnitRef> {
    Ok(UnitRef::new(
        decode_ulid(decoder)?,
        decode_version(decoder)?,
    ))
}

fn encode_decimal(encoder: &mut Encoder, value: &DecimalValue) {
    encoder.string(value.coefficient());
    encoder.u32(value.scale());
    encode_unit_ref(encoder, value.unit());
}

fn decode_decimal(decoder: &mut Decoder<'_>) -> CodecResult<DecimalValue> {
    DecimalValue::new(decoder.string()?, decoder.u32()?, decode_unit_ref(decoder)?)
        .map_err(ficant_application::map_domain_error)
}

fn encode_optional_decimal(encoder: &mut Encoder, value: Option<&DecimalValue>) {
    encoder.bool(value.is_some());
    if let Some(value) = value {
        encode_decimal(encoder, value);
    }
}

fn decode_optional_decimal(decoder: &mut Decoder<'_>) -> CodecResult<Option<DecimalValue>> {
    decoder.bool()?.then(|| decode_decimal(decoder)).transpose()
}

fn encode_source(encoder: &mut Encoder, value: &FactSource) {
    encoder.string(value.source_id());
    encoder.string(value.external_id());
    encoder.u64(value.source_revision());
}

fn decode_source(decoder: &mut Decoder<'_>) -> CodecResult<FactSource> {
    FactSource::new(decoder.string()?, decoder.string()?, decoder.u64()?)
        .map_err(ficant_application::map_domain_error)
}

fn encode_optional_id(encoder: &mut Encoder, value: Option<&Ulid>) {
    encoder.bool(value.is_some());
    if let Some(value) = value {
        encoder.string(value.as_str());
    }
}

fn decode_optional_id(decoder: &mut Decoder<'_>) -> CodecResult<Option<Ulid>> {
    decoder.bool()?.then(|| decode_ulid(decoder)).transpose()
}

fn encode_market_time(encoder: &mut Encoder, value: &MarketTime) {
    encoder.i64(value.instant().timestamp());
    encoder.u32(value.instant().timestamp_subsec_nanos());
    encoder.string(value.market_timezone());
    encoder.string(&value.local_trading_date().to_string());
}

fn decode_market_time(decoder: &mut Decoder<'_>) -> CodecResult<MarketTime> {
    let instant =
        DateTime::<Utc>::from_timestamp(decoder.i64()?, decoder.u32()?).ok_or_else(codec_error)?;
    MarketTime::new(instant, decoder.string()?, parse_date(&decoder.string()?)?)
        .map_err(ficant_application::map_domain_error)
}

fn encode_period(encoder: &mut Encoder, value: &EffectivePeriod) {
    encode_market_time(encoder, value.from());
    encode_market_time(encoder, value.to());
}

fn decode_period(decoder: &mut Decoder<'_>) -> CodecResult<EffectivePeriod> {
    EffectivePeriod::new(decode_market_time(decoder)?, decode_market_time(decoder)?)
        .map_err(ficant_application::map_domain_error)
}

fn decode_ulid(decoder: &mut Decoder<'_>) -> CodecResult<Ulid> {
    Ulid::new(decoder.string()?).map_err(ficant_application::map_domain_error)
}

fn decode_version(decoder: &mut Decoder<'_>) -> CodecResult<Version> {
    Version::new(decoder.u64()?).map_err(ficant_application::map_domain_error)
}

fn decode_hash(decoder: &mut Decoder<'_>) -> CodecResult<ContentHash> {
    ContentHash::from_bytes(&decoder.bytes()?).map_err(ficant_application::map_domain_error)
}

fn encode_optional_hash(encoder: &mut Encoder, value: Option<&ContentHash>) {
    encoder.bool(value.is_some());
    if let Some(value) = value {
        encoder.bytes(value.as_bytes());
    }
}

fn decode_optional_hash(decoder: &mut Decoder<'_>) -> CodecResult<Option<ContentHash>> {
    decoder.bool()?.then(|| decode_hash(decoder)).transpose()
}

fn encode_lineage(encoder: &mut Encoder, lineage: &[ficant_domain::primitives::LineageRef]) {
    encoder.len(lineage.len());
    for reference in lineage {
        encode_lineage_ref(encoder, reference);
    }
}

fn decode_lineage(
    decoder: &mut Decoder<'_>,
) -> CodecResult<Vec<ficant_domain::primitives::LineageRef>> {
    let count = decoder.len()?;
    let mut lineage = Vec::with_capacity(count);
    for _ in 0..count {
        lineage.push(decode_lineage_ref(decoder)?);
    }
    Ok(lineage)
}

fn encode_lineage_ref(encoder: &mut Encoder, value: &ficant_domain::primitives::LineageRef) {
    encoder.string(value.object_id().as_str());
    encoder.bool(value.version().is_some());
    if let Some(version) = value.version() {
        encoder.u64(version.get());
    }
    encode_optional_hash(encoder, value.content_hash());
}

fn decode_lineage_ref(
    decoder: &mut Decoder<'_>,
) -> CodecResult<ficant_domain::primitives::LineageRef> {
    let object_id = decode_ulid(decoder)?;
    let version = decoder
        .bool()?
        .then(|| decode_version(decoder))
        .transpose()?;
    let content_hash = decode_optional_hash(decoder)?;
    ficant_domain::primitives::LineageRef::new(object_id, version, content_hash)
        .map_err(ficant_application::map_domain_error)
}

fn parse_date(value: &str) -> CodecResult<NaiveDate> {
    value.parse().map_err(|_| codec_error())
}

fn parse_time(value: &str) -> CodecResult<NaiveTime> {
    value.parse().map_err(|_| codec_error())
}

const fn instrument_kind_code(value: InstrumentKind) -> u8 {
    match value {
        InstrumentKind::Bond => 1,
        InstrumentKind::Futures => 2,
        InstrumentKind::Other => 3,
    }
}

fn decode_instrument_kind(value: u8) -> CodecResult<InstrumentKind> {
    match value {
        1 => Ok(InstrumentKind::Bond),
        2 => Ok(InstrumentKind::Futures),
        3 => Ok(InstrumentKind::Other),
        _ => Err(codec_error()),
    }
}

const fn verification_status_code(value: VerificationStatus) -> u8 {
    match value {
        VerificationStatus::Unverified => 1,
        VerificationStatus::Verified => 2,
        VerificationStatus::Rejected => 3,
    }
}

fn decode_verification_status(value: u8) -> CodecResult<VerificationStatus> {
    match value {
        1 => Ok(VerificationStatus::Unverified),
        2 => Ok(VerificationStatus::Verified),
        3 => Ok(VerificationStatus::Rejected),
        _ => Err(codec_error()),
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

fn decode_cashflow_type(value: u8) -> CodecResult<CashflowType> {
    match value {
        1 => Ok(CashflowType::Coupon),
        2 => Ok(CashflowType::Principal),
        3 => Ok(CashflowType::Fee),
        4 => Ok(CashflowType::Other),
        _ => Err(codec_error()),
    }
}

const fn journal_event_type_code(value: JournalEventType) -> u8 {
    match value {
        JournalEventType::RunCreated => 1,
        JournalEventType::RunStarted => 2,
        JournalEventType::RunSucceeded => 3,
        JournalEventType::RunFailed => 4,
        JournalEventType::RunCancelled => 5,
        JournalEventType::ArtifactPublished => 6,
        JournalEventType::SignalSetPublished => 7,
        JournalEventType::NodeStarted => 8,
        JournalEventType::NodeSucceeded => 9,
        JournalEventType::NodeFailed => 10,
        JournalEventType::NodeCheckpointed => 11,
    }
}

fn decode_journal_event_type(value: u8) -> CodecResult<JournalEventType> {
    match value {
        1 => Ok(JournalEventType::RunCreated),
        2 => Ok(JournalEventType::RunStarted),
        3 => Ok(JournalEventType::RunSucceeded),
        4 => Ok(JournalEventType::RunFailed),
        5 => Ok(JournalEventType::RunCancelled),
        6 => Ok(JournalEventType::ArtifactPublished),
        7 => Ok(JournalEventType::SignalSetPublished),
        8 => Ok(JournalEventType::NodeStarted),
        9 => Ok(JournalEventType::NodeSucceeded),
        10 => Ok(JournalEventType::NodeFailed),
        11 => Ok(JournalEventType::NodeCheckpointed),
        _ => Err(codec_error()),
    }
}

const fn run_state_code(value: RunState) -> u8 {
    match value {
        RunState::Created => 1,
        RunState::Running => 2,
        RunState::Succeeded => 3,
        RunState::Failed => 4,
        RunState::Cancelled => 5,
    }
}

fn decode_run_state(value: u8) -> CodecResult<RunState> {
    match value {
        1 => Ok(RunState::Created),
        2 => Ok(RunState::Running),
        3 => Ok(RunState::Succeeded),
        4 => Ok(RunState::Failed),
        5 => Ok(RunState::Cancelled),
        _ => Err(codec_error()),
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

fn decode_artifact_kind(value: u8) -> CodecResult<ArtifactKind> {
    match value {
        1 => Ok(ArtifactKind::Generic),
        2 => Ok(ArtifactKind::CurveSnapshot),
        3 => Ok(ArtifactKind::DataSnapshot),
        4 => Ok(ArtifactKind::UniverseSnapshot),
        5 => Ok(ArtifactKind::SignalSet),
        _ => Err(codec_error()),
    }
}

fn codec_error() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, false)
}

struct Encoder(Vec<u8>);

impl Encoder {
    fn new() -> Self {
        Self(b"FSTO\x00\x01".to_vec())
    }

    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    fn bool(&mut self, value: bool) {
        self.u8(u8::from(value));
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn i64(&mut self, value: i64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn len(&mut self, value: usize) {
        self.u64(u64::try_from(value).expect("domain collection length fits u64"));
    }

    fn bytes(&mut self, value: &[u8]) {
        self.len(value.len());
        self.0.extend_from_slice(value);
    }

    fn string(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }

    fn finish(self) -> Vec<u8> {
        self.0
    }
}

struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8]) -> CodecResult<Self> {
        if !bytes.starts_with(b"FSTO\x00\x01") {
            return Err(codec_error());
        }
        Ok(Self { bytes, offset: 6 })
    }

    fn take(&mut self, length: usize) -> CodecResult<&'a [u8]> {
        let end = self.offset.checked_add(length).ok_or_else(codec_error)?;
        let value = self.bytes.get(self.offset..end).ok_or_else(codec_error)?;
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> CodecResult<u8> {
        Ok(self.take(1)?[0])
    }

    fn bool(&mut self) -> CodecResult<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(codec_error()),
        }
    }

    fn u32(&mut self) -> CodecResult<u32> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().map_err(|_| codec_error())?,
        ))
    }

    fn u64(&mut self) -> CodecResult<u64> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().map_err(|_| codec_error())?,
        ))
    }

    fn i64(&mut self) -> CodecResult<i64> {
        Ok(i64::from_be_bytes(
            self.take(8)?.try_into().map_err(|_| codec_error())?,
        ))
    }

    fn len(&mut self) -> CodecResult<usize> {
        let value = usize::try_from(self.u64()?).map_err(|_| codec_error())?;
        if value > self.bytes.len().saturating_sub(self.offset) {
            return Err(codec_error());
        }
        Ok(value)
    }

    fn bytes(&mut self) -> CodecResult<Vec<u8>> {
        let length = self.len()?;
        Ok(self.take(length)?.to_vec())
    }

    fn string(&mut self) -> CodecResult<String> {
        String::from_utf8(self.bytes()?).map_err(|_| codec_error())
    }

    fn end(&self) -> CodecResult<()> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(codec_error())
        }
    }
}

#[cfg(test)]
mod tests {
    use ficant_application::ports::{DefinitionValue, MarketFact};
    use ficant_domain::ContentAddressed;
    use ficant_domain::market::{FactSource, Quote, QuoteInput, Unit, UnitInput};
    use ficant_domain::primitives::{
        ContentHash, DecimalValue, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
    };
    use ficant_domain::research::{JournalEventType, RunJournal, RunJournalInput};
    use sqlx::types::chrono::{NaiveDate, TimeZone, Utc};

    use super::{
        decode_definition, decode_fact, decode_journal, encode_definition, encode_fact,
        encode_journal,
    };

    #[test]
    fn definition_codec_round_trips_domain_value_without_a_parallel_dto() {
        let unit = Unit::new(UnitInput {
            unit_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F03").unwrap(),
            version: Version::new(1).unwrap(),
            owner: OwnerRef::new(
                Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
                Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
            ),
            code: "CNY".to_owned(),
            dimension: "currency".to_owned(),
            scale: 2,
            precision: 18,
        })
        .unwrap();
        let value = DefinitionValue::Unit(unit);

        let encoded = encode_definition(&value);
        let decoded = decode_definition(&encoded).unwrap();

        assert_eq!(decoded, value);
    }

    #[test]
    fn market_fact_codec_round_trips_domain_value_without_a_parallel_dto() {
        let owner = OwnerRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
        );
        let instrument = VersionRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F09").unwrap(),
            Version::new(1).unwrap(),
        );
        let unit = UnitRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F04").unwrap(),
            Version::new(1).unwrap(),
        );
        let observed = MarketTime::new(
            Utc.with_ymd_and_hms(2025, 1, 15, 1, 1, 0).unwrap(),
            "Asia/Shanghai",
            NaiveDate::from_ymd_opt(2025, 1, 15).unwrap(),
        )
        .unwrap();
        let received = MarketTime::new(
            Utc.with_ymd_and_hms(2025, 1, 15, 1, 1, 1).unwrap(),
            "Asia/Shanghai",
            NaiveDate::from_ymd_opt(2025, 1, 15).unwrap(),
        )
        .unwrap();
        let value = MarketFact::Quote(
            Quote::new(QuoteInput {
                quote_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F11").unwrap(),
                instrument,
                owner,
                source: FactSource::new("china-rates-fixture-v1", "QUOTE-001", 1).unwrap(),
                observed_at: observed,
                received_at: received,
                bid: Some(DecimalValue::new("1012345", 4, unit.clone()).unwrap()),
                ask: Some(DecimalValue::new("1012500", 4, unit).unwrap()),
                supersedes_id: None,
            })
            .unwrap(),
        );

        let encoded = encode_fact(&value);
        let decoded = decode_fact(&encoded).unwrap();

        assert_eq!(decoded, value);
    }

    #[test]
    fn journal_codec_round_trips_hash_chained_evidence() {
        let occurred_at = MarketTime::new(
            Utc.with_ymd_and_hms(2025, 1, 15, 7, 5, 0).unwrap(),
            "Asia/Shanghai",
            NaiveDate::from_ymd_opt(2025, 1, 15).unwrap(),
        )
        .unwrap();
        let input = RunJournalInput {
            journal_event_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F21").unwrap(),
            run_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F17").unwrap(),
            sequence: 1,
            event_type: JournalEventType::RunCreated,
            occurred_at,
            payload_type: "ficant.run.created.v1".to_owned(),
            payload_schema: "ficant.fixture.v1".to_owned(),
            payload: b"run-created".to_vec(),
            prev_hash: None,
        };
        let event = RunJournal::new(input.clone(), &input.canonical_hash().unwrap()).unwrap();

        let encoded = encode_journal(&event);
        let decoded = decode_journal(&encoded).unwrap();

        assert_eq!(decoded, event);
        assert_eq!(decoded.content_hash(), event.content_hash());
        assert_ne!(decoded.content_hash(), &ContentHash::digest(b"different"));

        for event_type in [
            JournalEventType::NodeStarted,
            JournalEventType::NodeSucceeded,
            JournalEventType::NodeFailed,
            JournalEventType::NodeCheckpointed,
        ] {
            let mut node_input = input.clone();
            node_input.event_type = event_type;
            let node_event =
                RunJournal::new(node_input.clone(), &node_input.canonical_hash().unwrap()).unwrap();
            assert_eq!(
                decode_journal(&encode_journal(&node_event)).unwrap(),
                node_event
            );
        }
    }
}
