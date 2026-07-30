use async_trait::async_trait;
use chrono_tz::Tz;
use ficant_application::ports::{ApplicationResult, CanonicalQuote, CanonicalSnapshotDecoder};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_data::CanonicalSnapshotCodec;
use ficant_domain::analytics::{DECIMAL_SCALE, FixedDecimal};
use ficant_domain::primitives::{DecimalValue, MarketTime};
use ficant_domain::research::DataSnapshot;

#[derive(Clone, Copy, Debug, Default)]
pub struct CanonicalSnapshotCodecAdapter;

#[async_trait]
impl CanonicalSnapshotDecoder for CanonicalSnapshotCodecAdapter {
    async fn decode_quotes(
        &self,
        snapshot: &DataSnapshot,
        parquet: &[u8],
        manifest: &[u8],
    ) -> ApplicationResult<Vec<CanonicalQuote>> {
        let verified = CanonicalSnapshotCodec
            .decode_verified(snapshot.clone(), parquet, manifest)
            .map_err(integrity_failure)?;
        let visible_timezone = snapshot
            .visible_at()
            .market_timezone()
            .parse::<Tz>()
            .map_err(|_| integrity_failure(ficant_data::DataError::SnapshotIntegrityFailed))?;
        let _: Tz = snapshot
            .as_of()
            .market_timezone()
            .parse::<Tz>()
            .map_err(|_| integrity_failure(ficant_data::DataError::SnapshotIntegrityFailed))?;
        verified
            .quotes()
            .map_err(integrity_failure)?
            .into_iter()
            .map(|quote| {
                let observed_at = MarketTime::new(
                    quote.observed_at(),
                    snapshot.as_of().market_timezone(),
                    quote.local_trading_date(),
                )
                .map_err(|_| integrity_failure(ficant_data::DataError::SnapshotIntegrityFailed))?;
                let visible_at = MarketTime::new(
                    quote.visible_at(),
                    snapshot.visible_at().market_timezone(),
                    quote
                        .visible_at()
                        .with_timezone(&visible_timezone)
                        .date_naive(),
                )
                .map_err(|_| integrity_failure(ficant_data::DataError::SnapshotIntegrityFailed))?;
                Ok(CanonicalQuote::new(
                    quote.instrument().clone(),
                    observed_at,
                    visible_at,
                    quote.local_trading_date(),
                    quote.bid().map(fixed_decimal).transpose()?,
                    quote.ask().map(fixed_decimal).transpose()?,
                    quote.unit().clone(),
                ))
            })
            .collect()
    }
}

fn fixed_decimal(value: &DecimalValue) -> ApplicationResult<FixedDecimal> {
    let coefficient = value
        .coefficient()
        .parse::<i128>()
        .map_err(|_| integrity_failure(ficant_data::DataError::SnapshotIntegrityFailed))?;
    let scaled = match value.scale().cmp(&DECIMAL_SCALE) {
        std::cmp::Ordering::Equal => coefficient,
        std::cmp::Ordering::Less => coefficient
            .checked_mul(
                10_i128
                    .checked_pow(DECIMAL_SCALE - value.scale())
                    .ok_or_else(|| {
                        integrity_failure(ficant_data::DataError::SnapshotIntegrityFailed)
                    })?,
            )
            .ok_or_else(|| integrity_failure(ficant_data::DataError::SnapshotIntegrityFailed))?,
        std::cmp::Ordering::Greater => {
            let divisor = 10_i128
                .checked_pow(value.scale() - DECIMAL_SCALE)
                .ok_or_else(|| {
                    integrity_failure(ficant_data::DataError::SnapshotIntegrityFailed)
                })?;
            if coefficient % divisor != 0 {
                return Err(integrity_failure(
                    ficant_data::DataError::SnapshotIntegrityFailed,
                ));
            }
            coefficient / divisor
        }
    };
    Ok(FixedDecimal::from_scaled(scaled))
}

fn integrity_failure(_: ficant_data::DataError) -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::HashMismatch, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ficant_domain::primitives::{Ulid, UnitRef, Version};

    #[test]
    fn decimal_projection_is_exact_or_fails_closed() {
        assert_eq!(
            fixed_decimal(&decimal("10125", 2)).unwrap(),
            FixedDecimal::from_scaled(101_250_000_000_000)
        );
        assert_eq!(
            fixed_decimal(&decimal("101250000000000", 12)).unwrap(),
            FixedDecimal::from_scaled(101_250_000_000_000)
        );
        assert_eq!(
            fixed_decimal(&decimal("1012500000000000", 13)).unwrap(),
            FixedDecimal::from_scaled(101_250_000_000_000)
        );
        assert_eq!(
            fixed_decimal(&decimal("1012500000000001", 13))
                .unwrap_err()
                .category(),
            ApplicationErrorCategory::HashMismatch
        );
    }

    fn decimal(coefficient: &str, scale: u32) -> DecimalValue {
        DecimalValue::new(
            coefficient,
            scale,
            UnitRef::new(
                Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FAC").unwrap(),
                Version::new(1).unwrap(),
            ),
        )
        .unwrap()
    }
}
