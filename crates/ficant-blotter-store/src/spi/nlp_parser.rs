use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use ficant_application::ports::{ApplicationResult, NlpIntentParserPort};
use ficant_application::use_cases::blotter::{
    DraftTargetState, NlpEarmarkProposal, SourceTextAnchor, validate_source_text_anchors,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::blotter::{QuoteBasis, QuoteTerms, SettlementBucket, TradeSide};
use ficant_domain::primitives::FixedDecimal;

use crate::spi::csv_rfc4180::{CsvTable, parse_csv_fixed, parse_csv_non_empty};

/// Open-source reference deterministic token-based NLP intent parser (`NlpIntentParserPort`).
///
/// Parses structured key/token phrases (zero proprietary vendor vocabularies) such as:
/// `"BUY T+0 INST=BOND_001 NOTIONAL=10000000 CLEAN=100.25 TARGET=HOLD"`
/// or `"SELL T+1 INST=BOND_001 NOTIONAL=5000000 YTM=0.025 SETTLE=5012000 TARGET=COMMITTED"`
/// and computes exact UTF-8 half-open byte offsets for every extracted [`SourceTextAnchor`].
#[derive(Clone, Debug)]
pub struct MockNlpIntentParser {
    parser_source_id: String,
    model_version: String,
    confidence_score: FixedDecimal,
}

impl Default for MockNlpIntentParser {
    fn default() -> Self {
        Self {
            parser_source_id: "reference-token-parser".to_owned(),
            model_version: "1.0.0".to_owned(),
            confidence_score: FixedDecimal::from_scaled(950_000_000_000), // 0.95
        }
    }
}

impl MockNlpIntentParser {
    pub fn new(
        parser_source_id: impl Into<String>,
        model_version: impl Into<String>,
        confidence_score: FixedDecimal,
    ) -> ApplicationResult<Self> {
        let parser_source_id = parse_csv_non_empty(&parser_source_id.into())?;
        let model_version = parse_csv_non_empty(&model_version.into())?;
        if !confidence_score.is_non_negative() || confidence_score > FixedDecimal::ONE {
            return Err(validation_failed());
        }
        Ok(Self {
            parser_source_id,
            model_version,
            confidence_score,
        })
    }
}

impl NlpIntentParserPort for MockNlpIntentParser {
    #[allow(clippy::too_many_lines)]
    fn parse_nlp_intent(
        &self,
        request_id: &str,
        account_id: &str,
        raw_source_text: &str,
    ) -> ApplicationResult<NlpEarmarkProposal> {
        parse_csv_non_empty(request_id)?;
        parse_csv_non_empty(account_id)?;
        if raw_source_text.trim().is_empty() {
            return Err(validation_failed());
        }

        let mut side: Option<TradeSide> = None;
        let mut bucket: Option<SettlementBucket> = None;
        let mut instrument_code: Option<String> = None;
        let mut notional: Option<FixedDecimal> = None;
        let mut clean_price: Option<FixedDecimal> = None;
        let mut ytm: Option<FixedDecimal> = None;
        let mut settlement_amount: Option<FixedDecimal> = None;
        let mut target_state: DraftTargetState = DraftTargetState::Hold;
        let mut anchors: Vec<SourceTextAnchor> = Vec::new();

        for (start, end, token) in tokenize_with_utf8_spans(raw_source_text) {
            let upper = token.to_ascii_uppercase();
            if upper == "BUY" {
                side = Some(TradeSide::Buy);
                anchors.push(make_anchor("side", start, end, token)?);
            } else if upper == "SELL" {
                side = Some(TradeSide::Sell);
                anchors.push(make_anchor("side", start, end, token)?);
            } else if upper == "T+0" || upper == "T_PLUS_0" {
                bucket = Some(SettlementBucket::TPlus0);
                anchors.push(make_anchor("settlement_bucket", start, end, token)?);
            } else if upper == "T+1" || upper == "T_PLUS_1" {
                bucket = Some(SettlementBucket::TPlus1);
                anchors.push(make_anchor("settlement_bucket", start, end, token)?);
            } else if let Some((k, v)) = token.split_once('=') {
                let key_upper = k.to_ascii_uppercase();
                let val_start = start + k.len() + 1;
                match key_upper.as_str() {
                    "INST" | "INSTRUMENT" => {
                        instrument_code = Some(parse_csv_non_empty(v)?);
                        anchors.push(make_anchor("instrument_code", val_start, end, v)?);
                    }
                    "NOTIONAL" | "QTY" => {
                        let parsed = parse_csv_fixed(v)?;
                        if !parsed.is_positive() {
                            return Err(validation_failed());
                        }
                        notional = Some(parsed);
                        anchors.push(make_anchor("notional", val_start, end, v)?);
                    }
                    "CLEAN" | "PRICE" => {
                        let parsed = parse_csv_fixed(v)?;
                        if !parsed.is_positive() {
                            return Err(validation_failed());
                        }
                        clean_price = Some(parsed);
                        anchors.push(make_anchor("clean_price_per_100", val_start, end, v)?);
                    }
                    "YTM" | "YIELD" => {
                        let parsed = parse_csv_fixed(v)?;
                        if !parsed.is_non_negative() {
                            return Err(validation_failed());
                        }
                        ytm = Some(parsed);
                        anchors.push(make_anchor("yield_to_maturity", val_start, end, v)?);
                    }
                    "SETTLE" | "SETTLEMENT_AMOUNT" => {
                        let parsed = parse_csv_fixed(v)?;
                        if !parsed.is_positive() {
                            return Err(validation_failed());
                        }
                        settlement_amount = Some(parsed);
                        anchors.push(make_anchor("settlement_amount", val_start, end, v)?);
                    }
                    "TARGET" => {
                        target_state = match v.to_ascii_uppercase().as_str() {
                            "HOLD" => DraftTargetState::Hold,
                            "COMMITTED" => DraftTargetState::Committed,
                            _ => return Err(validation_failed()),
                        };
                        anchors.push(make_anchor("suggested_target_state", val_start, end, v)?);
                    }
                    _ => {}
                }
            }
        }

        let side = side.ok_or_else(validation_failed)?;
        let settlement_bucket = bucket.ok_or_else(validation_failed)?;
        let instrument_code = instrument_code.ok_or_else(validation_failed)?;

        let basis = match (clean_price, ytm) {
            (Some(_), Some(_)) => return Err(validation_failed()),
            (Some(c), None) => Some(QuoteBasis::CleanPricePer100(c)),
            (None, Some(y)) => Some(QuoteBasis::YieldToMaturity(y)),
            (None, None) => None,
        };
        let quote_terms = if basis.is_some() || settlement_amount.is_some() {
            let qt = QuoteTerms {
                basis,
                settlement_amount,
            };
            qt.validate().map_err(|_| validation_failed())?;
            Some(qt)
        } else {
            None
        };

        validate_source_text_anchors(raw_source_text, &anchors)?;

        Ok(NlpEarmarkProposal {
            parser_source_id: self.parser_source_id.clone(),
            model_version: self.model_version.clone(),
            raw_source_text: raw_source_text.to_owned(),
            anchors,
            confidence_score: self.confidence_score,
            account_id: account_id.to_owned(),
            instrument_code,
            side,
            settlement_bucket,
            notional,
            quote_terms,
            suggested_target_state: target_state,
        })
    }
}

/// RFC-4180 CSV fixture-backed reference adapter for `NlpIntentParserPort`.
///
/// Expected CSV headers:
/// `account_id,raw_source_text,parser_source_id,model_version,confidence_score,instrument_code,side,settlement_bucket,notional,clean_price_per_100,yield_to_maturity,settlement_amount,suggested_target_state,anchor_field,anchor_span`
#[derive(Clone, Debug)]
pub struct CsvTemplateNlpIntentParser {
    entries: BTreeMap<(String, String), NlpEarmarkProposal>,
}

impl CsvTemplateNlpIntentParser {
    #[allow(clippy::too_many_lines)]
    pub fn from_csv_str(csv_text: &str) -> ApplicationResult<Self> {
        let table = CsvTable::parse(csv_text)?;
        table.require_headers(&[
            "account_id",
            "raw_source_text",
            "parser_source_id",
            "model_version",
            "confidence_score",
            "instrument_code",
            "side",
            "settlement_bucket",
            "notional",
            "clean_price_per_100",
            "yield_to_maturity",
            "settlement_amount",
            "suggested_target_state",
            "anchor_field",
            "anchor_span",
        ])?;

        let mut entries = BTreeMap::new();
        for row in table.rows() {
            let account_id = parse_csv_non_empty(table.field(row, "account_id")?)?;
            let raw_source_text = table.field(row, "raw_source_text")?.to_owned();
            if raw_source_text.trim().is_empty() {
                return Err(validation_failed());
            }
            let parser_source_id = parse_csv_non_empty(table.field(row, "parser_source_id")?)?;
            let model_version = parse_csv_non_empty(table.field(row, "model_version")?)?;
            let confidence_score = parse_csv_fixed(table.field(row, "confidence_score")?)?;
            if !confidence_score.is_non_negative() || confidence_score > FixedDecimal::ONE {
                return Err(validation_failed());
            }
            let instrument_code = parse_csv_non_empty(table.field(row, "instrument_code")?)?;
            let side = match table.field(row, "side")? {
                "BUY" => TradeSide::Buy,
                "SELL" => TradeSide::Sell,
                _ => return Err(validation_failed()),
            };
            let settlement_bucket = match table.field(row, "settlement_bucket")? {
                "T_PLUS_0" => SettlementBucket::TPlus0,
                "T_PLUS_1" => SettlementBucket::TPlus1,
                _ => return Err(validation_failed()),
            };
            let notional = table
                .optional_field(row, "notional")
                .map(parse_csv_fixed)
                .transpose()?;
            if let Some(n) = notional
                && !n.is_positive()
            {
                return Err(validation_failed());
            }
            let clean_opt = table
                .optional_field(row, "clean_price_per_100")
                .map(parse_csv_fixed)
                .transpose()?;
            let ytm_opt = table
                .optional_field(row, "yield_to_maturity")
                .map(parse_csv_fixed)
                .transpose()?;
            let settle_opt = table
                .optional_field(row, "settlement_amount")
                .map(parse_csv_fixed)
                .transpose()?;
            let basis = match (clean_opt, ytm_opt) {
                (Some(_), Some(_)) => return Err(validation_failed()),
                (Some(c), None) => Some(QuoteBasis::CleanPricePer100(c)),
                (None, Some(y)) => Some(QuoteBasis::YieldToMaturity(y)),
                (None, None) => None,
            };
            let quote_terms = if basis.is_some() || settle_opt.is_some() {
                let qt = QuoteTerms {
                    basis,
                    settlement_amount: settle_opt,
                };
                qt.validate().map_err(|_| validation_failed())?;
                Some(qt)
            } else {
                None
            };
            let suggested_target_state = match table.field(row, "suggested_target_state")? {
                "HOLD" => DraftTargetState::Hold,
                "COMMITTED" => DraftTargetState::Committed,
                _ => return Err(validation_failed()),
            };

            let anchor_field = parse_csv_non_empty(table.field(row, "anchor_field")?)?;
            let anchor_span = table.field(row, "anchor_span")?;
            let start = raw_source_text
                .find(anchor_span)
                .ok_or_else(validation_failed)?;
            let end = start + anchor_span.len();
            let anchors = vec![make_anchor(&anchor_field, start, end, anchor_span)?];
            validate_source_text_anchors(&raw_source_text, &anchors)?;

            let key = (account_id.clone(), raw_source_text.clone());
            if entries.contains_key(&key) {
                return Err(validation_failed());
            }
            entries.insert(
                key,
                NlpEarmarkProposal {
                    parser_source_id,
                    model_version,
                    raw_source_text,
                    anchors,
                    confidence_score,
                    account_id,
                    instrument_code,
                    side,
                    settlement_bucket,
                    notional,
                    quote_terms,
                    suggested_target_state,
                },
            );
        }

        Ok(Self { entries })
    }

    pub fn from_csv_file(path: impl AsRef<Path>) -> ApplicationResult<Self> {
        let raw = fs::read_to_string(path).map_err(|_| validation_failed())?;
        Self::from_csv_str(&raw)
    }
}

impl NlpIntentParserPort for CsvTemplateNlpIntentParser {
    fn parse_nlp_intent(
        &self,
        request_id: &str,
        account_id: &str,
        raw_source_text: &str,
    ) -> ApplicationResult<NlpEarmarkProposal> {
        parse_csv_non_empty(request_id)?;
        parse_csv_non_empty(account_id)?;
        if raw_source_text.trim().is_empty() {
            return Err(validation_failed());
        }
        self.entries
            .get(&(account_id.to_owned(), raw_source_text.to_owned()))
            .cloned()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))
    }
}

fn tokenize_with_utf8_spans(text: &str) -> Vec<(usize, usize, &str)> {
    let mut tokens = Vec::new();
    let mut start: Option<usize> = None;
    for (idx, ch) in text.char_indices() {
        let is_delim = ch.is_whitespace() || matches!(ch, '：' | ':' | ',' | ';' | '，' | '；');
        if is_delim {
            if let Some(s) = start.take() {
                tokens.push((s, idx, &text[s..idx]));
            }
        } else if start.is_none() {
            start = Some(idx);
        }
    }
    if let Some(s) = start {
        tokens.push((s, text.len(), &text[s..]));
    }
    tokens
}

fn make_anchor(
    field_name: &str,
    start: usize,
    end: usize,
    span_text: &str,
) -> ApplicationResult<SourceTextAnchor> {
    Ok(SourceTextAnchor {
        field_name: field_name.to_owned(),
        byte_start: u32::try_from(start).map_err(|_| validation_failed())?,
        byte_end: u32::try_from(end).map_err(|_| validation_failed())?,
        span_text: span_text.to_owned(),
    })
}

const fn validation_failed() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
