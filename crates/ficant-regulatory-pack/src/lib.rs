//! L3 parser for the typed `regulatory` `RulePack` payload (ADR-0026).
//!
//! The payload names the constraints of a Subject's constraint set and what each one measures.
//! It carries no ceilings; those are Subject state.

use ficant_application::ports::{ApplicationResult, RegulatoryRulePackParser};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_contracts::ficant::market::v1::{
    ConstraintMeasure as ProtoConstraintMeasure, RegulatoryRulePack,
};
use ficant_domain::constraint::{ConstraintDefinition, ConstraintMeasure, ConstraintSet};
use ficant_domain::market::RulePackContent;
use ficant_domain::primitives::VersionRef;
use prost::Message;

pub const MARKET: &str = "CN";
pub const RULE_TYPE: &str = "regulatory";
pub const TYPE_URL: &str = "type.googleapis.com/ficant.market.v1.RegulatoryRulePack";

/// Parses only the exact v1 regulatory-rule content schema.
#[derive(Clone, Copy, Debug, Default)]
pub struct RegulatoryRulePackV1Parser;

impl RegulatoryRulePackParser for RegulatoryRulePackV1Parser {
    fn market(&self) -> &'static str {
        MARKET
    }

    fn rule_type(&self) -> &'static str {
        RULE_TYPE
    }

    fn type_url(&self) -> &'static str {
        TYPE_URL
    }

    fn parse(
        &self,
        content: &RulePackContent,
        rule_pack: VersionRef,
    ) -> ApplicationResult<ConstraintSet> {
        if content.type_url() != TYPE_URL {
            return Err(invalid());
        }
        let payload = RegulatoryRulePack::decode(content.value()).map_err(|_| invalid())?;
        if payload.constraints.is_empty() {
            return Err(missing("constraints"));
        }
        let definitions = payload
            .constraints
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let measure = match ProtoConstraintMeasure::try_from(item.measure) {
                    Ok(ProtoConstraintMeasure::InstrumentFace) => ConstraintMeasure::InstrumentFace,
                    Ok(ProtoConstraintMeasure::PortfolioFace) => ConstraintMeasure::PortfolioFace,
                    Ok(ProtoConstraintMeasure::BorrowedCash) => ConstraintMeasure::BorrowedCash,
                    Ok(ProtoConstraintMeasure::Unspecified) | Err(_) => {
                        return Err(missing(&format!("constraints[{index}].measure")));
                    }
                };
                ConstraintDefinition::new(item.limit_code.clone(), measure)
                    .map_err(|_| missing(&format!("constraints[{index}].limit_code")))
            })
            .collect::<ApplicationResult<Vec<_>>>()?;
        ConstraintSet::new(rule_pack, definitions).map_err(map_domain_error)
    }
}

fn missing(path: &str) -> ApplicationError {
    ApplicationError::rule_pack_item_missing(format!("context.regulatory_rule_pack.content.{path}"))
}

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
