//! gRPC / gRPC-Web service implementations for the four versioned Desk Blotter SPIs:
//! - `PositionBaseProviderService`
//! - `TradeFeedReconcilerService`
//! - `MarketQuoteStreamService`
//! - `IntranetNlpIntentParserService`
//!
//! Each service enforces bearer credential authentication, role & scope authorization,
//! strict page-size bounds (`1..=1024`), and single-terminal-error semantics on streams.

use std::pin::Pin;
use std::sync::Arc;

use ficant_application::ports::{
    ApplicationResult, BlotterClock, CredentialVerifier, MAX_SPI_PAGE_SIZE, MarketQuoteStreamPort,
    NlpIntentParserPort, PositionBaseProviderPort, PrincipalRole, TradeFeedReconcilerPort,
    VerifiedPrincipal, validate_spi_page_size,
};
use ficant_application::use_cases::blotter::AnalysisUnits;
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_contracts::ficant::blotter::v1 as pb;
use ficant_contracts::ficant::blotter::v1::intranet_nlp_intent_parser_service_server::IntranetNlpIntentParserService;
use ficant_contracts::ficant::blotter::v1::market_quote_stream_service_server::MarketQuoteStreamService;
use ficant_contracts::ficant::blotter::v1::position_base_provider_service_server::PositionBaseProviderService;
use ficant_contracts::ficant::blotter::v1::trade_feed_reconciler_service_server::TradeFeedReconcilerService;
use tokio::sync::{mpsc, watch};
use tokio_stream::Stream;
use tonic::{Request, Response, Status};

use crate::auth::extract_bearer_token;
use crate::blotter::{ShutdownAwareStream, is_shutting_down, send_or_shutdown};
use crate::blotter_codec::{
    parse_date, parse_non_empty, parse_quote_cursor, to_pb_market_quote_tick, to_pb_market_time,
    to_pb_nlp_proposal, to_pb_opening_position_base, to_pb_pull_market_quotes_result,
    to_pb_pull_trade_feed_result, to_pb_quote_cursor, to_pb_trade_fill_record,
};
use crate::to_error_detail;

// ============================================================================
// 1. PositionBaseProviderService
// ============================================================================

pub struct PositionBaseProviderGrpcService {
    backend: Option<Arc<dyn PositionBaseProviderPort>>,
    credentials: Option<Arc<dyn CredentialVerifier>>,
    clock: Arc<dyn BlotterClock>,
}

impl PositionBaseProviderGrpcService {
    #[must_use]
    pub fn new(
        backend: Arc<dyn PositionBaseProviderPort>,
        credentials: Arc<dyn CredentialVerifier>,
        clock: Arc<dyn BlotterClock>,
    ) -> Self {
        Self {
            backend: Some(backend),
            credentials: Some(credentials),
            clock,
        }
    }

    #[must_use]
    pub fn disabled(clock: Arc<dyn BlotterClock>) -> Self {
        Self {
            backend: None,
            credentials: None,
            clock,
        }
    }

    fn execute_fetch(
        &self,
        metadata: &tonic::metadata::MetadataMap,
        req: &pb::FetchOpeningPositionBaseRequest,
    ) -> ApplicationResult<pb::OpeningPositionBase> {
        let backend = self.backend.as_ref().ok_or_else(storage_unavailable)?;
        let verifier = self.credentials.as_ref().ok_or_else(storage_unavailable)?;
        let principal = authenticate(metadata, verifier.as_ref(), self.clock.as_ref())?;
        require_role(
            &principal,
            &[PrincipalRole::FeedAdapter, PrincipalRole::Operator],
        )?;

        let request_id = parse_non_empty(&req.request_id)?;
        let account_id = parse_non_empty(&req.account_id)?;
        let trading_date = parse_date(&req.trading_date)?;

        if !principal.can_access_account(&account_id) {
            return Err(forbidden());
        }

        let base = backend.fetch_opening_position_base(&request_id, &account_id, trading_date)?;
        if !principal.can_access_source(&base.core.source_id) {
            return Err(forbidden());
        }
        Ok(to_pb_opening_position_base(&base))
    }
}

#[tonic::async_trait]
impl PositionBaseProviderService for PositionBaseProviderGrpcService {
    async fn fetch_opening_position_base(
        &self,
        request: Request<pb::FetchOpeningPositionBaseRequest>,
    ) -> Result<Response<pb::FetchOpeningPositionBaseResponse>, Status> {
        let result = match self.execute_fetch(request.metadata(), request.get_ref()) {
            Ok(value) => pb::fetch_opening_position_base_response::Result::Value(value),
            Err(err) => {
                pb::fetch_opening_position_base_response::Result::Error(to_error_detail(&err))
            }
        };
        Ok(Response::new(pb::FetchOpeningPositionBaseResponse {
            result: Some(result),
        }))
    }
}

// ============================================================================
// 2. TradeFeedReconcilerService
// ============================================================================

pub struct TradeFeedReconcilerGrpcService {
    backend: Option<Arc<dyn TradeFeedReconcilerPort>>,
    credentials: Option<Arc<dyn CredentialVerifier>>,
    clock: Arc<dyn BlotterClock>,
    units: AnalysisUnits,
    shutdown_rx: Option<watch::Receiver<bool>>,
    stream_drain_tx: Option<mpsc::Sender<()>>,
}

impl TradeFeedReconcilerGrpcService {
    #[must_use]
    pub fn new(
        backend: Arc<dyn TradeFeedReconcilerPort>,
        credentials: Arc<dyn CredentialVerifier>,
        clock: Arc<dyn BlotterClock>,
        units: AnalysisUnits,
    ) -> Self {
        Self {
            backend: Some(backend),
            credentials: Some(credentials),
            clock,
            units,
            shutdown_rx: None,
            stream_drain_tx: None,
        }
    }

    #[must_use]
    pub fn disabled(clock: Arc<dyn BlotterClock>, units: AnalysisUnits) -> Self {
        Self {
            backend: None,
            credentials: None,
            clock,
            units,
            shutdown_rx: None,
            stream_drain_tx: None,
        }
    }

    #[must_use]
    pub fn with_shutdown_signal(mut self, shutdown_rx: watch::Receiver<bool>) -> Self {
        self.shutdown_rx = Some(shutdown_rx);
        self
    }

    #[must_use]
    pub fn with_stream_drain_guard(mut self, drain_tx: mpsc::Sender<()>) -> Self {
        self.stream_drain_tx = Some(drain_tx);
        self
    }

    fn execute_pull(
        &self,
        metadata: &tonic::metadata::MetadataMap,
        req: &pb::PullTradeFeedBatchRequest,
    ) -> ApplicationResult<pb::PullTradeFeedBatchResult> {
        let backend = self.backend.as_ref().ok_or_else(storage_unavailable)?;
        let verifier = self.credentials.as_ref().ok_or_else(storage_unavailable)?;
        let principal = authenticate(metadata, verifier.as_ref(), self.clock.as_ref())?;
        require_role(
            &principal,
            &[PrincipalRole::FeedAdapter, PrincipalRole::Operator],
        )?;

        let source_id = parse_non_empty(&req.source_id)?;
        validate_spi_page_size(req.max_records)?;
        if !principal.can_access_source(&source_id) {
            return Err(forbidden());
        }

        let batch =
            backend.pull_trade_feed_batch(&source_id, req.after_feed_seq, req.max_records)?;
        for fill in &batch.fills {
            if !principal.can_access_account(&fill.account_id) {
                return Err(forbidden());
            }
        }
        Ok(to_pb_pull_trade_feed_result(&batch, &self.units))
    }

    fn execute_fetch_revision(
        &self,
        metadata: &tonic::metadata::MetadataMap,
        req: &pb::FetchTradeRevisionRequest,
    ) -> ApplicationResult<pb::TradeFillRecord> {
        let backend = self.backend.as_ref().ok_or_else(storage_unavailable)?;
        let verifier = self.credentials.as_ref().ok_or_else(storage_unavailable)?;
        let principal = authenticate(metadata, verifier.as_ref(), self.clock.as_ref())?;
        require_role(
            &principal,
            &[PrincipalRole::FeedAdapter, PrincipalRole::Operator],
        )?;

        let source_id = parse_non_empty(&req.source_id)?;
        let trade_id = parse_non_empty(&req.external_trade_id)?;
        if req.source_revision == 0 {
            return Err(validation_failed());
        }
        if !principal.can_access_source(&source_id) {
            return Err(forbidden());
        }

        let fill = backend.fetch_trade_revision(&source_id, &trade_id, req.source_revision)?;
        if !principal.can_access_account(&fill.account_id) {
            return Err(forbidden());
        }
        Ok(to_pb_trade_fill_record(&fill, &self.units))
    }
}

#[allow(clippy::too_many_lines)]
#[tonic::async_trait]
impl TradeFeedReconcilerService for TradeFeedReconcilerGrpcService {
    type StreamTradeFeedStream =
        Pin<Box<dyn Stream<Item = Result<pb::StreamTradeFeedResponse, Status>> + Send>>;

    async fn pull_trade_feed_batch(
        &self,
        request: Request<pb::PullTradeFeedBatchRequest>,
    ) -> Result<Response<pb::PullTradeFeedBatchResponse>, Status> {
        let result = match self.execute_pull(request.metadata(), request.get_ref()) {
            Ok(value) => pb::pull_trade_feed_batch_response::Result::Value(value),
            Err(err) => pb::pull_trade_feed_batch_response::Result::Error(to_error_detail(&err)),
        };
        Ok(Response::new(pb::PullTradeFeedBatchResponse {
            result: Some(result),
        }))
    }

    async fn stream_trade_feed(
        &self,
        request: Request<pb::StreamTradeFeedRequest>,
    ) -> Result<Response<Self::StreamTradeFeedStream>, Status> {
        let (tx, rx) = mpsc::channel(64);
        let backend = self.backend.clone();
        let credentials = self.credentials.clone();
        let clock = self.clock.clone();
        let units = self.units.clone();
        let mut producer_shutdown = self.shutdown_rx.clone();
        let stream_shutdown = self.shutdown_rx.clone();
        let drain_guard_clone = self.stream_drain_tx.clone();
        let metadata = request.metadata().clone();
        let req = request.into_inner();

        tokio::spawn(async move {
            let _drain_guard = drain_guard_clone;
            if is_shutting_down(producer_shutdown.as_ref()) {
                return;
            }
            let Some(backend) = backend else {
                send_trade_stream_err(&tx, &mut producer_shutdown, &storage_unavailable()).await;
                return;
            };
            let Some(verifier) = credentials else {
                send_trade_stream_err(&tx, &mut producer_shutdown, &storage_unavailable()).await;
                return;
            };

            let token = match extract_bearer_token(&metadata) {
                Ok(tok) => tok,
                Err(err) => {
                    send_trade_stream_err(&tx, &mut producer_shutdown, &err).await;
                    return;
                }
            };
            let principal = match verifier.verify_bearer(&token, &clock.now()) {
                Ok(p) => p,
                Err(err) => {
                    send_trade_stream_err(&tx, &mut producer_shutdown, &err).await;
                    return;
                }
            };
            if let Err(err) = require_role(
                &principal,
                &[PrincipalRole::FeedAdapter, PrincipalRole::Operator],
            ) {
                send_trade_stream_err(&tx, &mut producer_shutdown, &err).await;
                return;
            }
            let source_id = match parse_non_empty(&req.source_id) {
                Ok(s) => s,
                Err(err) => {
                    send_trade_stream_err(&tx, &mut producer_shutdown, &err).await;
                    return;
                }
            };
            if !principal.can_access_source(&source_id) {
                send_trade_stream_err(&tx, &mut producer_shutdown, &forbidden()).await;
                return;
            }

            let mut cursor = req.after_feed_seq;
            loop {
                if is_shutting_down(producer_shutdown.as_ref()) {
                    return;
                }
                let batch =
                    match backend.pull_trade_feed_batch(&source_id, cursor, MAX_SPI_PAGE_SIZE) {
                        Ok(b) => b,
                        Err(err) => {
                            send_trade_stream_err(&tx, &mut producer_shutdown, &err).await;
                            return;
                        }
                    };
                for fill in &batch.fills {
                    if is_shutting_down(producer_shutdown.as_ref()) {
                        return;
                    }
                    let live_principal = match verifier.verify_bearer(&token, &clock.now()) {
                        Ok(p) => p,
                        Err(err) => {
                            send_trade_stream_err(&tx, &mut producer_shutdown, &err).await;
                            return;
                        }
                    };
                    if !live_principal.can_access_source(&source_id)
                        || !live_principal.can_access_account(&fill.account_id)
                    {
                        send_trade_stream_err(&tx, &mut producer_shutdown, &forbidden()).await;
                        return;
                    }
                    if !send_or_shutdown(
                        &tx,
                        &mut producer_shutdown,
                        Ok(pb::StreamTradeFeedResponse {
                            payload: Some(pb::stream_trade_feed_response::Payload::Fill(
                                to_pb_trade_fill_record(fill, &units),
                            )),
                        }),
                    )
                    .await
                    {
                        return;
                    }
                }
                cursor = batch.scanned_through_feed_seq;
                if !batch.has_more {
                    break;
                }
            }

            let _ = send_or_shutdown(
                &tx,
                &mut producer_shutdown,
                Ok(pb::StreamTradeFeedResponse {
                    payload: Some(pb::stream_trade_feed_response::Payload::Heartbeat(
                        pb::FeedHeartbeat {
                            source_id,
                            contiguous_feed_seq: cursor,
                            server_time: Some(to_pb_market_time(&clock.now())),
                        },
                    )),
                }),
            )
            .await;
        });

        Ok(Response::new(Box::pin(ShutdownAwareStream::new(
            rx,
            stream_shutdown,
        ))))
    }

    async fn fetch_trade_revision(
        &self,
        request: Request<pb::FetchTradeRevisionRequest>,
    ) -> Result<Response<pb::FetchTradeRevisionResponse>, Status> {
        let result = match self.execute_fetch_revision(request.metadata(), request.get_ref()) {
            Ok(value) => pb::fetch_trade_revision_response::Result::Value(value),
            Err(err) => pb::fetch_trade_revision_response::Result::Error(to_error_detail(&err)),
        };
        Ok(Response::new(pb::FetchTradeRevisionResponse {
            result: Some(result),
        }))
    }
}

async fn send_trade_stream_err(
    tx: &mpsc::Sender<Result<pb::StreamTradeFeedResponse, Status>>,
    shutdown_rx: &mut Option<watch::Receiver<bool>>,
    err: &ApplicationError,
) {
    let _ = send_or_shutdown(
        tx,
        shutdown_rx,
        Ok(pb::StreamTradeFeedResponse {
            payload: Some(pb::stream_trade_feed_response::Payload::Error(
                to_error_detail(err),
            )),
        }),
    )
    .await;
}

// ============================================================================
// 3. MarketQuoteStreamService
// ============================================================================

pub struct MarketQuoteStreamGrpcService {
    backend: Option<Arc<dyn MarketQuoteStreamPort>>,
    credentials: Option<Arc<dyn CredentialVerifier>>,
    clock: Arc<dyn BlotterClock>,
    units: AnalysisUnits,
    shutdown_rx: Option<watch::Receiver<bool>>,
    stream_drain_tx: Option<mpsc::Sender<()>>,
}

impl MarketQuoteStreamGrpcService {
    #[must_use]
    pub fn new(
        backend: Arc<dyn MarketQuoteStreamPort>,
        credentials: Arc<dyn CredentialVerifier>,
        clock: Arc<dyn BlotterClock>,
        units: AnalysisUnits,
    ) -> Self {
        Self {
            backend: Some(backend),
            credentials: Some(credentials),
            clock,
            units,
            shutdown_rx: None,
            stream_drain_tx: None,
        }
    }

    #[must_use]
    pub fn disabled(clock: Arc<dyn BlotterClock>, units: AnalysisUnits) -> Self {
        Self {
            backend: None,
            credentials: None,
            clock,
            units,
            shutdown_rx: None,
            stream_drain_tx: None,
        }
    }

    #[must_use]
    pub fn with_shutdown_signal(mut self, shutdown_rx: watch::Receiver<bool>) -> Self {
        self.shutdown_rx = Some(shutdown_rx);
        self
    }

    #[must_use]
    pub fn with_stream_drain_guard(mut self, drain_tx: mpsc::Sender<()>) -> Self {
        self.stream_drain_tx = Some(drain_tx);
        self
    }

    fn execute_pull(
        &self,
        metadata: &tonic::metadata::MetadataMap,
        req: &pb::PullMarketQuotesRequest,
    ) -> ApplicationResult<pb::PullMarketQuotesResult> {
        let backend = self.backend.as_ref().ok_or_else(storage_unavailable)?;
        let verifier = self.credentials.as_ref().ok_or_else(storage_unavailable)?;
        let principal = authenticate(metadata, verifier.as_ref(), self.clock.as_ref())?;
        require_role(
            &principal,
            &[PrincipalRole::FeedAdapter, PrincipalRole::Operator],
        )?;

        let cursor = parse_quote_cursor(req.cursor.as_ref())?;
        validate_spi_page_size(req.max_ticks)?;
        if !principal.can_access_source(&cursor.source_id) {
            return Err(forbidden());
        }

        let res = backend.pull_market_quotes(&cursor, req.max_ticks)?;
        Ok(to_pb_pull_market_quotes_result(&res, &self.units))
    }
}

#[tonic::async_trait]
impl MarketQuoteStreamService for MarketQuoteStreamGrpcService {
    type StreamMarketQuotesStream =
        Pin<Box<dyn Stream<Item = Result<pb::StreamMarketQuotesResponse, Status>> + Send>>;

    async fn pull_market_quotes(
        &self,
        request: Request<pb::PullMarketQuotesRequest>,
    ) -> Result<Response<pb::PullMarketQuotesResponse>, Status> {
        let result = match self.execute_pull(request.metadata(), request.get_ref()) {
            Ok(value) => pb::pull_market_quotes_response::Result::Value(value),
            Err(err) => pb::pull_market_quotes_response::Result::Error(to_error_detail(&err)),
        };
        Ok(Response::new(pb::PullMarketQuotesResponse {
            result: Some(result),
        }))
    }

    #[allow(clippy::too_many_lines)]
    async fn stream_market_quotes(
        &self,
        request: Request<pb::StreamMarketQuotesRequest>,
    ) -> Result<Response<Self::StreamMarketQuotesStream>, Status> {
        let (tx, rx) = mpsc::channel(64);
        let backend = self.backend.clone();
        let credentials = self.credentials.clone();
        let clock = self.clock.clone();
        let units = self.units.clone();
        let mut producer_shutdown = self.shutdown_rx.clone();
        let stream_shutdown = self.shutdown_rx.clone();
        let drain_guard_clone = self.stream_drain_tx.clone();
        let metadata = request.metadata().clone();
        let req = request.into_inner();

        tokio::spawn(async move {
            let _drain_guard = drain_guard_clone;
            if is_shutting_down(producer_shutdown.as_ref()) {
                return;
            }
            let Some(backend) = backend else {
                send_quote_stream_err(&tx, &mut producer_shutdown, &storage_unavailable()).await;
                return;
            };
            let Some(verifier) = credentials else {
                send_quote_stream_err(&tx, &mut producer_shutdown, &storage_unavailable()).await;
                return;
            };

            let token = match extract_bearer_token(&metadata) {
                Ok(tok) => tok,
                Err(err) => {
                    send_quote_stream_err(&tx, &mut producer_shutdown, &err).await;
                    return;
                }
            };
            let principal = match verifier.verify_bearer(&token, &clock.now()) {
                Ok(p) => p,
                Err(err) => {
                    send_quote_stream_err(&tx, &mut producer_shutdown, &err).await;
                    return;
                }
            };
            if let Err(err) = require_role(
                &principal,
                &[PrincipalRole::FeedAdapter, PrincipalRole::Operator],
            ) {
                send_quote_stream_err(&tx, &mut producer_shutdown, &err).await;
                return;
            }
            let mut cursor = match parse_quote_cursor(req.cursor.as_ref()) {
                Ok(c) => c,
                Err(err) => {
                    send_quote_stream_err(&tx, &mut producer_shutdown, &err).await;
                    return;
                }
            };
            if !principal.can_access_source(&cursor.source_id) {
                send_quote_stream_err(&tx, &mut producer_shutdown, &forbidden()).await;
                return;
            }

            loop {
                if is_shutting_down(producer_shutdown.as_ref()) {
                    return;
                }
                let page = match backend.pull_market_quotes(&cursor, MAX_SPI_PAGE_SIZE) {
                    Ok(p) => p,
                    Err(err) => {
                        send_quote_stream_err(&tx, &mut producer_shutdown, &err).await;
                        return;
                    }
                };
                for tick in &page.ticks {
                    if is_shutting_down(producer_shutdown.as_ref()) {
                        return;
                    }
                    let live_principal = match verifier.verify_bearer(&token, &clock.now()) {
                        Ok(p) => p,
                        Err(err) => {
                            send_quote_stream_err(&tx, &mut producer_shutdown, &err).await;
                            return;
                        }
                    };
                    if !live_principal.can_access_source(&cursor.source_id) {
                        send_quote_stream_err(&tx, &mut producer_shutdown, &forbidden()).await;
                        return;
                    }
                    if !send_or_shutdown(
                        &tx,
                        &mut producer_shutdown,
                        Ok(pb::StreamMarketQuotesResponse {
                            payload: Some(pb::stream_market_quotes_response::Payload::Tick(
                                to_pb_market_quote_tick(tick, &units),
                            )),
                        }),
                    )
                    .await
                    {
                        return;
                    }
                }
                cursor = page.next_cursor;
                if !page.has_more {
                    break;
                }
            }

            let _ = send_or_shutdown(
                &tx,
                &mut producer_shutdown,
                Ok(pb::StreamMarketQuotesResponse {
                    payload: Some(pb::stream_market_quotes_response::Payload::Heartbeat(
                        pb::QuoteHeartbeat {
                            cursor: Some(to_pb_quote_cursor(&cursor)),
                            server_time: Some(to_pb_market_time(&clock.now())),
                        },
                    )),
                }),
            )
            .await;
        });

        Ok(Response::new(Box::pin(ShutdownAwareStream::new(
            rx,
            stream_shutdown,
        ))))
    }
}

async fn send_quote_stream_err(
    tx: &mpsc::Sender<Result<pb::StreamMarketQuotesResponse, Status>>,
    shutdown_rx: &mut Option<watch::Receiver<bool>>,
    err: &ApplicationError,
) {
    let _ = send_or_shutdown(
        tx,
        shutdown_rx,
        Ok(pb::StreamMarketQuotesResponse {
            payload: Some(pb::stream_market_quotes_response::Payload::Error(
                to_error_detail(err),
            )),
        }),
    )
    .await;
}

// ============================================================================
// 4. IntranetNlpIntentParserService
// ============================================================================

pub struct IntranetNlpIntentParserGrpcService {
    backend: Option<Arc<dyn NlpIntentParserPort>>,
    credentials: Option<Arc<dyn CredentialVerifier>>,
    clock: Arc<dyn BlotterClock>,
    units: AnalysisUnits,
}

impl IntranetNlpIntentParserGrpcService {
    #[must_use]
    pub fn new(
        backend: Arc<dyn NlpIntentParserPort>,
        credentials: Arc<dyn CredentialVerifier>,
        clock: Arc<dyn BlotterClock>,
        units: AnalysisUnits,
    ) -> Self {
        Self {
            backend: Some(backend),
            credentials: Some(credentials),
            clock,
            units,
        }
    }

    #[must_use]
    pub fn disabled(clock: Arc<dyn BlotterClock>, units: AnalysisUnits) -> Self {
        Self {
            backend: None,
            credentials: None,
            clock,
            units,
        }
    }

    fn execute_parse(
        &self,
        metadata: &tonic::metadata::MetadataMap,
        req: &pb::ParseNlpIntentRequest,
    ) -> ApplicationResult<pb::NlpEarmarkProposal> {
        let backend = self.backend.as_ref().ok_or_else(storage_unavailable)?;
        let verifier = self.credentials.as_ref().ok_or_else(storage_unavailable)?;
        let principal = authenticate(metadata, verifier.as_ref(), self.clock.as_ref())?;
        require_role(
            &principal,
            &[PrincipalRole::NlpParser, PrincipalRole::Trader],
        )?;

        let request_id = parse_non_empty(&req.request_id)?;
        let account_id = parse_non_empty(&req.account_id)?;
        if req.raw_source_text.trim().is_empty() {
            return Err(validation_failed());
        }
        if !principal.can_access_account(&account_id) {
            return Err(forbidden());
        }

        let proposal = backend.parse_nlp_intent(&request_id, &account_id, &req.raw_source_text)?;
        Ok(to_pb_nlp_proposal(&proposal, &self.units))
    }
}

#[tonic::async_trait]
impl IntranetNlpIntentParserService for IntranetNlpIntentParserGrpcService {
    async fn parse_nlp_intent(
        &self,
        request: Request<pb::ParseNlpIntentRequest>,
    ) -> Result<Response<pb::ParseNlpIntentResponse>, Status> {
        let result = match self.execute_parse(request.metadata(), request.get_ref()) {
            Ok(value) => pb::parse_nlp_intent_response::Result::Value(value),
            Err(err) => pb::parse_nlp_intent_response::Result::Error(to_error_detail(&err)),
        };
        Ok(Response::new(pb::ParseNlpIntentResponse {
            result: Some(result),
        }))
    }
}

// ============================================================================
// Shared SPI helpers
// ============================================================================

fn authenticate(
    metadata: &tonic::metadata::MetadataMap,
    verifier: &dyn CredentialVerifier,
    clock: &dyn BlotterClock,
) -> ApplicationResult<VerifiedPrincipal> {
    let token = extract_bearer_token(metadata)?;
    verifier.verify_bearer(&token, &clock.now())
}

fn require_role(principal: &VerifiedPrincipal, allowed: &[PrincipalRole]) -> ApplicationResult<()> {
    if allowed.contains(&principal.role) {
        Ok(())
    } else {
        Err(forbidden())
    }
}

const fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}

const fn validation_failed() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

const fn storage_unavailable() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, true)
}
