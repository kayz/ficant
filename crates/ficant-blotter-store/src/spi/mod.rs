//! Four open-source reference SPI adapters (strict RFC-4180 CSV & in-memory Mock).

pub mod csv_rfc4180;
pub mod market_quote;
pub mod nlp_parser;
pub mod position_base;
pub mod trade_feed;

pub use csv_rfc4180::CsvTable;
pub use market_quote::{CsvMarketQuoteStream, MockMarketQuoteStream};
pub use nlp_parser::{CsvTemplateNlpIntentParser, MockNlpIntentParser};
pub use position_base::{
    CsvPositionBaseProvider, MockPositionBaseProvider, default_reference_analysis_units,
};
pub use trade_feed::{CsvTradeFeedReconciler, MockTradeFeedReconciler};
