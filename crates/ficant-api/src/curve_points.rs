use ficant_application::ports::{CurvePointSetDecoder, DecodedCurvePoint, DecodedCurvePointSet};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::market::v1 as pb;
use ficant_domain::primitives::{ContentHash, DecimalValue, Ulid, UnitRef, Version};
use prost::Message;

#[derive(Clone, Copy, Debug, Default)]
pub struct CanonicalCurvePointSetDecoder;

impl CurvePointSetDecoder for CanonicalCurvePointSetDecoder {
    fn decode_canonical(&self, bytes: &[u8]) -> Result<DecodedCurvePointSet, ApplicationError> {
        let value = pb::CurvePointSet::decode(bytes).map_err(|_| invalid())?;
        if value.encode_to_vec() != bytes {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::HashMismatch,
                false,
            ));
        }
        let points = value
            .points
            .iter()
            .map(|point| {
                DecodedCurvePoint::new(
                    point.curve_node_id.clone(),
                    parse_hash(point.curve_node_content_hash.as_ref())?,
                    parse_decimal(point.yield_to_maturity.as_ref())?,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        DecodedCurvePointSet::new(value.curve_family_id, points)
    }
}

fn parse_decimal(value: Option<&core::DecimalValue>) -> Result<DecimalValue, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    DecimalValue::new(
        value.coefficient.clone(),
        value.scale,
        parse_unit(value.unit.as_ref())?,
    )
    .map_err(map_domain_error)
}

fn parse_unit(value: Option<&core::UnitRef>) -> Result<UnitRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(UnitRef::new(
        parse_ulid(value.unit_id.as_ref())?,
        Version::new(value.version).map_err(map_domain_error)?,
    ))
}

fn parse_hash(value: Option<&core::Sha256>) -> Result<ContentHash, ApplicationError> {
    ContentHash::from_bytes(&value.ok_or_else(invalid)?.value).map_err(map_domain_error)
}

fn parse_ulid(value: Option<&core::Ulid>) -> Result<Ulid, ApplicationError> {
    Ulid::new(value.ok_or_else(invalid)?.value.clone()).map_err(map_domain_error)
}

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
