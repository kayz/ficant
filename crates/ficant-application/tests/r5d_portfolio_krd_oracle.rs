use std::collections::BTreeMap;
use std::ops::Index;

const INPUTS_JSON: &str =
    include_str!("../../../tests/golden-cases/china-rates/r5d-portfolio-krd-oracle-inputs.json");
const EXPECTED_JSON: &str = include_str!(
    "../../../tests/golden-cases/china-rates/expected/r5d-portfolio-krd-oracle-expected.json"
);

// Reuse only the already-integrated production adapter.  The independent formula lives solely in
// the Python Decimal oracle; this module merely proves that its JSON inputs exactly describe the
// real R4d-b application fixture before invoking CalculateBondKeyRateDv01.
mod production_fixture {
    include!("r4d_b_futures_krd_contracts.rs");

    pub(super) async fn execute_oracle_case(
        inputs: &super::Json,
    ) -> ficant_domain::research::PortfolioKeyRateExposure {
        assert_shared_input_contract(inputs);
        Fixture::new(true, false, Calls::default())
            .execute(true)
            .await
            .expect("the frozen mixed portfolio production path must succeed")
    }

    fn assert_shared_input_contract(inputs: &super::Json) {
        assert_eq!(
            inputs["schema"],
            "ficant.r5d-portfolio-krd-oracle.inputs.v1"
        );
        assert_eq!(inputs["decimal_scale"], 12);
        assert_eq!(inputs["dv01_unit"], "DV01_CNY");
        assert_reference_price_model(inputs);

        let fixture = Fixture::new(true, false, Calls::default());
        let input_positions = inputs["positions"]
            .as_array()
            .expect("positions must be an array");
        assert_positions(&fixture, input_positions);
        assert_factors(&fixture, inputs);
        assert_instrument_terms(&fixture, input_positions);
    }

    fn assert_reference_price_model(inputs: &super::Json) {
        assert_eq!(
            inputs["reference_price_model"]["registered_face_price"],
            "100.000000000000"
        );
        assert_eq!(
            inputs["reference_price_model"]["yield_price_multiplier"],
            "100.000000000000"
        );
        let weights = inputs["reference_price_model"]["curve_interpolation_weights"]
            .as_array()
            .expect("curve interpolation weights must be an array");
        assert_eq!(weights.len(), 3);
        assert_eq!(weights[0], "0.500000000000");
        assert_eq!(weights[1], "0.500000000000");
        assert_eq!(weights[2], "0.000000000000");
    }

    fn assert_positions(fixture: &Fixture, input_positions: &[super::Json]) {
        assert_eq!(input_positions.len(), 2);
        assert_eq!(fixture.snapshot.positions().len(), 2);
        for (input, actual) in input_positions.iter().zip(fixture.snapshot.positions()) {
            assert_eq!(input["position_id"], actual.id().as_str());
            assert_eq!(
                input["instrument_id"],
                actual.instrument_ref().id().as_str()
            );
            let quantity = actual.quantity();
            assert_decimal(
                input["quantity"].as_str().unwrap(),
                quantity.coefficient(),
                quantity.scale(),
            );
        }
        assert_eq!(input_positions[0]["kind"], "bond");
        assert_eq!(input_positions[1]["kind"], "futures");
        assert_eq!(
            input_positions[1]["ctd_instrument_id"],
            input_positions[0]["instrument_id"]
        );
    }

    fn assert_factors(fixture: &Fixture, inputs: &super::Json) {
        let input_factors = inputs["factors"]
            .as_array()
            .expect("factors must be an array");
        assert_eq!(input_factors.len(), 3);
        assert_eq!(fixture.factors.len(), 3);
        assert_eq!(fixture.nodes.len(), 3);
        assert_eq!(fixture.points.points().len(), 3);
        for (((input, factor), node), point) in input_factors
            .iter()
            .zip(&fixture.factors)
            .zip(&fixture.nodes)
            .zip(fixture.points.points())
        {
            assert_eq!(input["factor_id"], factor.factor_id());
            assert_eq!(input["curve_node_id"], node.curve_node_id());
            assert_eq!(input["direction"], "central");
            assert_eq!(
                factor.convention().direction(),
                SensitivityDirection::Central
            );
            assert_decimal(
                input["bump_yield"].as_str().unwrap(),
                factor.convention().bump().coefficient(),
                factor.convention().bump().scale(),
            );
            assert_decimal(
                input["base_yield"].as_str().unwrap(),
                point.yield_to_maturity().coefficient(),
                point.yield_to_maturity().scale(),
            );
        }
    }

    fn assert_instrument_terms(fixture: &Fixture, input_positions: &[super::Json]) {
        let bond = fixture
            .definitions
            .iter()
            .find_map(|definition| match definition {
                DefinitionValue::Instrument(value) if value.instrument().id() == &id('B') => {
                    match value.subtype() {
                        Some(ficant_application::ports::InstrumentSubtype::Bond(value)) => {
                            Some(value)
                        }
                        _ => None,
                    }
                }
                _ => None,
            })
            .expect("the shared production fixture must contain the exact Bond");
        assert_decimal(
            input_positions[0]["registered_face"].as_str().unwrap(),
            bond.face_value().coefficient(),
            bond.face_value().scale(),
        );

        let future = fixture
            .definitions
            .iter()
            .find_map(|definition| match definition {
                DefinitionValue::Instrument(value) if value.instrument().id() == &id('F') => {
                    match value.subtype() {
                        Some(ficant_application::ports::InstrumentSubtype::FuturesContract(
                            value,
                        )) => Some(value),
                        _ => None,
                    }
                }
                _ => None,
            })
            .expect("the shared production fixture must contain the exact FuturesContract");
        assert_eq!(future.product_code(), Some("T"));
        assert_eq!(future.price_unit(), Some(&unit_ref('P')));

        let rule = delivery_rule();
        assert_decimal_fixed(
            input_positions[1]["ctd_registered_face"].as_str().unwrap(),
            fixed(100),
        );
        assert_decimal_fixed(
            input_positions[1]["face_quote_basis"].as_str().unwrap(),
            rule.face_quote_basis(),
        );
        assert_eq!(
            input_positions[1]["contract_size_in_quote_units"],
            "10000.000000000000"
        );
        assert_eq!(rule.contract_size_in_quote_units(), Some(10_000));
        assert_eq!(input_positions[1]["conversion_factor"], "1.000000000000");
    }

    fn assert_decimal(rendered: &str, coefficient: &str, scale: u32) {
        let normalized = rendered.trim_end_matches('0').trim_end_matches('.');
        let expected = if normalized.is_empty() {
            "0"
        } else {
            normalized
        };
        let actual = if scale == 0 {
            coefficient.to_owned()
        } else {
            let negative = coefficient.starts_with('-');
            let digits = coefficient.trim_start_matches('-');
            let width = usize::try_from(scale).unwrap() + 1;
            let padded = format!("{digits:0>width$}");
            let split = padded.len() - usize::try_from(scale).unwrap();
            format!(
                "{}{}.{}",
                if negative { "-" } else { "" },
                &padded[..split],
                &padded[split..]
            )
        };
        assert_eq!(actual, expected);
    }

    fn assert_decimal_fixed(rendered: &str, actual: FixedDecimal) {
        assert_eq!(super::scaled_12(rendered), actual.scaled());
    }
}

#[tokio::test]
async fn production_portfolio_krd_matches_independent_decimal_oracle_exactly() {
    let inputs = Json::parse(INPUTS_JSON).expect("valid shared R5D input fixture");
    let expected = Json::parse(EXPECTED_JSON).expect("valid independent R5D expected fixture");
    let result = production_fixture::execute_oracle_case(&inputs).await;

    let expected_positions = expected["positions"]
        .as_array()
        .expect("expected positions must be an array");
    assert_eq!(result.positions().len(), expected_positions.len());
    for actual in result.positions() {
        let expected_position = expected_positions
            .iter()
            .find(|value| value["position_id"] == actual.position_id().as_str())
            .expect("every production position must have an independent expected witness");
        assert_eq!(
            expected_position["instrument_id"],
            actual.instrument().id().as_str()
        );
        let expected_nodes = expected_position["nodes"].as_array().unwrap();
        assert_eq!(actual.exposures().len(), expected_nodes.len());
        for (actual_node, expected_node) in actual.exposures().iter().zip(expected_nodes) {
            assert_eq!(expected_node["factor_id"], actual_node.factor_id());
            assert_eq!(
                actual_node.value().scaled(),
                scaled_12(expected_node["dv01"].as_str().unwrap()),
                "position {} factor {}",
                actual.position_id().as_str(),
                actual_node.factor_id()
            );
        }
        assert_eq!(
            actual
                .exposures()
                .iter()
                .map(|value| value.value().scaled())
                .sum::<i128>(),
            scaled_12(expected_position["node_sum_dv01"].as_str().unwrap())
        );
    }

    let expected_totals = expected["node_totals"].as_array().unwrap();
    assert_eq!(result.totals().len(), expected_totals.len());
    for (actual, expected_total) in result.totals().iter().zip(expected_totals) {
        assert_eq!(expected_total["factor_id"], actual.factor_id());
        assert_eq!(
            actual.value().scaled(),
            scaled_12(expected_total["dv01"].as_str().unwrap())
        );
    }
    assert_eq!(
        result
            .totals()
            .iter()
            .map(|value| value.value().scaled())
            .sum::<i128>(),
        scaled_12(expected["portfolio"]["node_sum_dv01"].as_str().unwrap())
    );
}

fn scaled_12(value: &str) -> i128 {
    let (negative, unsigned) = value
        .strip_prefix('-')
        .map_or((false, value), |unsigned| (true, unsigned));
    let (whole, fraction) = unsigned
        .split_once('.')
        .expect("oracle decimals must include an explicit fractional part");
    assert_eq!(fraction.len(), 12, "oracle output must use scale 12");
    let magnitude =
        whole.parse::<i128>().unwrap() * 1_000_000_000_000_i128 + fraction.parse::<i128>().unwrap();
    if negative { -magnitude } else { magnitude }
}

#[derive(Debug, PartialEq)]
enum Json {
    Object(BTreeMap<String, Json>),
    Array(Vec<Json>),
    String(String),
    Number(i64),
}

impl Json {
    fn parse(source: &str) -> Result<Self, String> {
        let mut parser = JsonParser {
            bytes: source.as_bytes(),
            cursor: 0,
        };
        let value = parser.value()?;
        parser.whitespace();
        if parser.cursor == parser.bytes.len() {
            Ok(value)
        } else {
            Err(format!(
                "unexpected trailing JSON at byte {}",
                parser.cursor
            ))
        }
    }

    fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }
}

impl Index<&str> for Json {
    type Output = Json;

    fn index(&self, key: &str) -> &Self::Output {
        match self {
            Self::Object(values) => values
                .get(key)
                .unwrap_or_else(|| panic!("JSON object is missing key {key:?}")),
            _ => panic!("cannot index a non-object JSON value with {key:?}"),
        }
    }
}

impl Index<usize> for Json {
    type Output = Json;

    fn index(&self, index: usize) -> &Self::Output {
        match self {
            Self::Array(values) => &values[index],
            _ => panic!("cannot index a non-array JSON value with {index}"),
        }
    }
}

impl PartialEq<&str> for Json {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == Some(*other)
    }
}

impl PartialEq<i32> for Json {
    fn eq(&self, other: &i32) -> bool {
        matches!(self, Self::Number(value) if *value == i64::from(*other))
    }
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl JsonParser<'_> {
    fn value(&mut self) -> Result<Json, String> {
        self.whitespace();
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => self.string().map(Json::String),
            Some(b'-' | b'0'..=b'9') => self.number().map(Json::Number),
            Some(byte) => Err(format!(
                "unsupported JSON token {:?} at byte {}",
                char::from(byte),
                self.cursor
            )),
            None => Err("unexpected end of JSON".to_owned()),
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.expect(b'{')?;
        let mut values = BTreeMap::new();
        self.whitespace();
        if self.take(b'}') {
            return Ok(Json::Object(values));
        }
        loop {
            self.whitespace();
            let key = self.string()?;
            self.whitespace();
            self.expect(b':')?;
            let value = self.value()?;
            if values.insert(key.clone(), value).is_some() {
                return Err(format!("duplicate JSON object key {key:?}"));
            }
            self.whitespace();
            if self.take(b'}') {
                return Ok(Json::Object(values));
            }
            self.expect(b',')?;
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.expect(b'[')?;
        let mut values = Vec::new();
        self.whitespace();
        if self.take(b']') {
            return Ok(Json::Array(values));
        }
        loop {
            values.push(self.value()?);
            self.whitespace();
            if self.take(b']') {
                return Ok(Json::Array(values));
            }
            self.expect(b',')?;
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut value = String::new();
        loop {
            let byte = self
                .next()
                .ok_or_else(|| "unterminated JSON string".to_owned())?;
            match byte {
                b'"' => return Ok(value),
                b'\\' => {
                    let escaped = self
                        .next()
                        .ok_or_else(|| "unterminated JSON escape".to_owned())?;
                    let decoded = match escaped {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{0008}',
                        b'f' => '\u{000c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        _ => {
                            return Err(format!(
                                "unsupported JSON escape at byte {}",
                                self.cursor - 1
                            ));
                        }
                    };
                    value.push(decoded);
                }
                0x00..=0x1f => {
                    return Err(format!(
                        "control character in JSON string at byte {}",
                        self.cursor - 1
                    ));
                }
                0x20..=0x7f => value.push(char::from(byte)),
                _ => {
                    return Err(format!(
                        "non-ASCII JSON string is outside this fixture parser at byte {}",
                        self.cursor - 1
                    ));
                }
            }
        }
    }

    fn number(&mut self) -> Result<i64, String> {
        let start = self.cursor;
        self.take(b'-');
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.cursor += 1;
        }
        let rendered = std::str::from_utf8(&self.bytes[start..self.cursor])
            .map_err(|error| error.to_string())?;
        rendered
            .parse::<i64>()
            .map_err(|error| format!("invalid JSON integer {rendered:?}: {error}"))
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.cursor += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        if self.take(expected) {
            Ok(())
        } else {
            Err(format!(
                "expected {:?} at byte {}",
                char::from(expected),
                self.cursor
            ))
        }
    }

    fn take(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.cursor).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let value = self.peek()?;
        self.cursor += 1;
        Some(value)
    }
}
