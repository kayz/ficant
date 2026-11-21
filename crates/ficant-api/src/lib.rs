//! Transport layer for Ficant Rates Analytics, Desk Blotter, and versioned Blotter SPIs.

#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

pub mod auth;
pub mod blotter;
pub mod blotter_codec;
pub mod blotter_spi;
pub mod error;
pub mod rates;

pub use auth::{
    AUTHORIZATION_HEADER, CredentialConfigFile, CredentialEntry, CredentialEntryConfig,
    FICANT_CREDENTIAL_HEADER, StaticCredentialVerifier, constant_time_eq_32, decode_digest_hex,
    encode_digest_hex, extract_bearer_token, sha256_token_digest,
};
pub use blotter::{
    BlotterStorePort, BlotterWriteOutcome, DEFAULT_MAX_EVENT_ENVELOPE_BYTES,
    DEFAULT_REVERSAL_HEADROOM_BYTES, DEFAULT_STREAM_HEARTBEAT_INTERVAL,
    DEFAULT_SUBSCRIBE_REPLAY_PAGE_SIZE, DEFAULT_SUBSCRIBE_STREAM_CHANNEL_CAPACITY,
    DeskBlotterGrpcService, DeskBlotterServiceConfig, DisabledDeskBlotterGrpcService,
    OwnedBlotterEngines, SubscribeBarrierSnapshot, SubscribeBarrierTestHook, TxUlidGenerator,
    WatchdogController, WriteAuthorizeFn, WriteTransitionFn, spawn_blotter_watchdog,
};
pub use blotter_spi::{
    IntranetNlpIntentParserGrpcService, MarketQuoteStreamGrpcService,
    PositionBaseProviderGrpcService, TradeFeedReconcilerGrpcService,
};
pub use error::{to_error_detail, to_tonic_status};
pub use rates::RatesGrpcService;
