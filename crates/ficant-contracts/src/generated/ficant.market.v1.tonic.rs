// @generated
/// Generated client implementations.
pub mod market_definition_service_client {
    #![allow(
        unused_variables,
        dead_code,
        missing_docs,
        clippy::wildcard_imports,
        clippy::let_unit_value,
    )]
    use tonic::codegen::*;
    use tonic::codegen::http::Uri;
    ///
    #[derive(Debug, Clone)]
    pub struct MarketDefinitionServiceClient<T> {
        inner: tonic::client::Grpc<T>,
    }
    impl MarketDefinitionServiceClient<tonic::transport::Channel> {
        /// Attempt to create a new client by connecting to a given endpoint.
        pub async fn connect<D>(dst: D) -> Result<Self, tonic::transport::Error>
        where
            D: TryInto<tonic::transport::Endpoint>,
            D::Error: Into<StdError>,
        {
            let conn = tonic::transport::Endpoint::new(dst)?.connect().await?;
            Ok(Self::new(conn))
        }
    }
    impl<T> MarketDefinitionServiceClient<T>
    where
        T: tonic::client::GrpcService<tonic::body::Body>,
        T::Error: Into<StdError>,
        T::ResponseBody: Body<Data = Bytes> + std::marker::Send + 'static,
        <T::ResponseBody as Body>::Error: Into<StdError> + std::marker::Send,
    {
        pub fn new(inner: T) -> Self {
            let inner = tonic::client::Grpc::new(inner);
            Self { inner }
        }
        pub fn with_origin(inner: T, origin: Uri) -> Self {
            let inner = tonic::client::Grpc::with_origin(inner, origin);
            Self { inner }
        }
        pub fn with_interceptor<F>(
            inner: T,
            interceptor: F,
        ) -> MarketDefinitionServiceClient<InterceptedService<T, F>>
        where
            F: tonic::service::Interceptor,
            T::ResponseBody: Default,
            T: tonic::codegen::Service<
                http::Request<tonic::body::Body>,
                Response = http::Response<
                    <T as tonic::client::GrpcService<tonic::body::Body>>::ResponseBody,
                >,
            >,
            <T as tonic::codegen::Service<
                http::Request<tonic::body::Body>,
            >>::Error: Into<StdError> + std::marker::Send + std::marker::Sync,
        {
            MarketDefinitionServiceClient::new(
                InterceptedService::new(inner, interceptor),
            )
        }
        /// Compress requests with the given encoding.
        ///
        /// This requires the server to support it otherwise it might respond with an
        /// error.
        #[must_use]
        pub fn send_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.inner = self.inner.send_compressed(encoding);
            self
        }
        /// Enable decompressing responses.
        #[must_use]
        pub fn accept_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.inner = self.inner.accept_compressed(encoding);
            self
        }
        /// Limits the maximum size of a decoded message.
        ///
        /// Default: `4MB`
        #[must_use]
        pub fn max_decoding_message_size(mut self, limit: usize) -> Self {
            self.inner = self.inner.max_decoding_message_size(limit);
            self
        }
        /// Limits the maximum size of an encoded message.
        ///
        /// Default: `usize::MAX`
        #[must_use]
        pub fn max_encoding_message_size(mut self, limit: usize) -> Self {
            self.inner = self.inner.max_encoding_message_size(limit);
            self
        }
        ///
        pub async fn append_instrument(
            &mut self,
            request: impl tonic::IntoRequest<super::AppendInstrumentRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendInstrumentResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/ficant.market.v1.MarketDefinitionService/AppendInstrument",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.market.v1.MarketDefinitionService",
                        "AppendInstrument",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn append_bond(
            &mut self,
            request: impl tonic::IntoRequest<super::AppendBondRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendBondResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/ficant.market.v1.MarketDefinitionService/AppendBond",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.market.v1.MarketDefinitionService",
                        "AppendBond",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn append_futures_contract(
            &mut self,
            request: impl tonic::IntoRequest<super::AppendFuturesContractRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendFuturesContractResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/ficant.market.v1.MarketDefinitionService/AppendFuturesContract",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.market.v1.MarketDefinitionService",
                        "AppendFuturesContract",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn append_calendar(
            &mut self,
            request: impl tonic::IntoRequest<super::AppendCalendarRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendCalendarResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/ficant.market.v1.MarketDefinitionService/AppendCalendar",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.market.v1.MarketDefinitionService",
                        "AppendCalendar",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn append_unit(
            &mut self,
            request: impl tonic::IntoRequest<super::AppendUnitRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendUnitResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/ficant.market.v1.MarketDefinitionService/AppendUnit",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.market.v1.MarketDefinitionService",
                        "AppendUnit",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn append_market_rule_pack(
            &mut self,
            request: impl tonic::IntoRequest<super::AppendMarketRulePackRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendMarketRulePackResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/ficant.market.v1.MarketDefinitionService/AppendMarketRulePack",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.market.v1.MarketDefinitionService",
                        "AppendMarketRulePack",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn get_definition_version(
            &mut self,
            request: impl tonic::IntoRequest<super::GetDefinitionVersionRequest>,
        ) -> std::result::Result<
            tonic::Response<super::GetDefinitionVersionResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/ficant.market.v1.MarketDefinitionService/GetDefinitionVersion",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.market.v1.MarketDefinitionService",
                        "GetDefinitionVersion",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn resolve_definition_as_of(
            &mut self,
            request: impl tonic::IntoRequest<super::ResolveDefinitionAsOfRequest>,
        ) -> std::result::Result<
            tonic::Response<super::ResolveDefinitionAsOfResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/ficant.market.v1.MarketDefinitionService/ResolveDefinitionAsOf",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.market.v1.MarketDefinitionService",
                        "ResolveDefinitionAsOf",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn list_definition_versions(
            &mut self,
            request: impl tonic::IntoRequest<super::ListDefinitionVersionsRequest>,
        ) -> std::result::Result<
            tonic::Response<super::ListDefinitionVersionsResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/ficant.market.v1.MarketDefinitionService/ListDefinitionVersions",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.market.v1.MarketDefinitionService",
                        "ListDefinitionVersions",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
    }
}
/// Generated server implementations.
pub mod market_definition_service_server {
    #![allow(
        unused_variables,
        dead_code,
        missing_docs,
        clippy::wildcard_imports,
        clippy::let_unit_value,
    )]
    use tonic::codegen::*;
    /// Generated trait containing gRPC methods that should be implemented for use with MarketDefinitionServiceServer.
    #[async_trait]
    pub trait MarketDefinitionService: std::marker::Send + std::marker::Sync + 'static {
        ///
        async fn append_instrument(
            &self,
            request: tonic::Request<super::AppendInstrumentRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendInstrumentResponse>,
            tonic::Status,
        >;
        ///
        async fn append_bond(
            &self,
            request: tonic::Request<super::AppendBondRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendBondResponse>,
            tonic::Status,
        >;
        ///
        async fn append_futures_contract(
            &self,
            request: tonic::Request<super::AppendFuturesContractRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendFuturesContractResponse>,
            tonic::Status,
        >;
        ///
        async fn append_calendar(
            &self,
            request: tonic::Request<super::AppendCalendarRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendCalendarResponse>,
            tonic::Status,
        >;
        ///
        async fn append_unit(
            &self,
            request: tonic::Request<super::AppendUnitRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendUnitResponse>,
            tonic::Status,
        >;
        ///
        async fn append_market_rule_pack(
            &self,
            request: tonic::Request<super::AppendMarketRulePackRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendMarketRulePackResponse>,
            tonic::Status,
        >;
        ///
        async fn get_definition_version(
            &self,
            request: tonic::Request<super::GetDefinitionVersionRequest>,
        ) -> std::result::Result<
            tonic::Response<super::GetDefinitionVersionResponse>,
            tonic::Status,
        >;
        ///
        async fn resolve_definition_as_of(
            &self,
            request: tonic::Request<super::ResolveDefinitionAsOfRequest>,
        ) -> std::result::Result<
            tonic::Response<super::ResolveDefinitionAsOfResponse>,
            tonic::Status,
        >;
        ///
        async fn list_definition_versions(
            &self,
            request: tonic::Request<super::ListDefinitionVersionsRequest>,
        ) -> std::result::Result<
            tonic::Response<super::ListDefinitionVersionsResponse>,
            tonic::Status,
        >;
    }
    ///
    #[derive(Debug)]
    pub struct MarketDefinitionServiceServer<T> {
        inner: Arc<T>,
        accept_compression_encodings: EnabledCompressionEncodings,
        send_compression_encodings: EnabledCompressionEncodings,
        max_decoding_message_size: Option<usize>,
        max_encoding_message_size: Option<usize>,
    }
    impl<T> MarketDefinitionServiceServer<T> {
        pub fn new(inner: T) -> Self {
            Self::from_arc(Arc::new(inner))
        }
        pub fn from_arc(inner: Arc<T>) -> Self {
            Self {
                inner,
                accept_compression_encodings: Default::default(),
                send_compression_encodings: Default::default(),
                max_decoding_message_size: None,
                max_encoding_message_size: None,
            }
        }
        pub fn with_interceptor<F>(
            inner: T,
            interceptor: F,
        ) -> InterceptedService<Self, F>
        where
            F: tonic::service::Interceptor,
        {
            InterceptedService::new(Self::new(inner), interceptor)
        }
        /// Enable decompressing requests with the given encoding.
        #[must_use]
        pub fn accept_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.accept_compression_encodings.enable(encoding);
            self
        }
        /// Compress responses with the given encoding, if the client supports it.
        #[must_use]
        pub fn send_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.send_compression_encodings.enable(encoding);
            self
        }
        /// Limits the maximum size of a decoded message.
        ///
        /// Default: `4MB`
        #[must_use]
        pub fn max_decoding_message_size(mut self, limit: usize) -> Self {
            self.max_decoding_message_size = Some(limit);
            self
        }
        /// Limits the maximum size of an encoded message.
        ///
        /// Default: `usize::MAX`
        #[must_use]
        pub fn max_encoding_message_size(mut self, limit: usize) -> Self {
            self.max_encoding_message_size = Some(limit);
            self
        }
    }
    impl<T, B> tonic::codegen::Service<http::Request<B>>
    for MarketDefinitionServiceServer<T>
    where
        T: MarketDefinitionService,
        B: Body + std::marker::Send + 'static,
        B::Error: Into<StdError> + std::marker::Send + 'static,
    {
        type Response = http::Response<tonic::body::Body>;
        type Error = std::convert::Infallible;
        type Future = BoxFuture<Self::Response, Self::Error>;
        fn poll_ready(
            &mut self,
            _cx: &mut Context<'_>,
        ) -> Poll<std::result::Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, req: http::Request<B>) -> Self::Future {
            match req.uri().path() {
                "/ficant.market.v1.MarketDefinitionService/AppendInstrument" => {
                    #[allow(non_camel_case_types)]
                    struct AppendInstrumentSvc<T: MarketDefinitionService>(pub Arc<T>);
                    impl<
                        T: MarketDefinitionService,
                    > tonic::server::UnaryService<super::AppendInstrumentRequest>
                    for AppendInstrumentSvc<T> {
                        type Response = super::AppendInstrumentResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::AppendInstrumentRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as MarketDefinitionService>::append_instrument(
                                        &inner,
                                        request,
                                    )
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = AppendInstrumentSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/ficant.market.v1.MarketDefinitionService/AppendBond" => {
                    #[allow(non_camel_case_types)]
                    struct AppendBondSvc<T: MarketDefinitionService>(pub Arc<T>);
                    impl<
                        T: MarketDefinitionService,
                    > tonic::server::UnaryService<super::AppendBondRequest>
                    for AppendBondSvc<T> {
                        type Response = super::AppendBondResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::AppendBondRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as MarketDefinitionService>::append_bond(&inner, request)
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = AppendBondSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/ficant.market.v1.MarketDefinitionService/AppendFuturesContract" => {
                    #[allow(non_camel_case_types)]
                    struct AppendFuturesContractSvc<T: MarketDefinitionService>(
                        pub Arc<T>,
                    );
                    impl<
                        T: MarketDefinitionService,
                    > tonic::server::UnaryService<super::AppendFuturesContractRequest>
                    for AppendFuturesContractSvc<T> {
                        type Response = super::AppendFuturesContractResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::AppendFuturesContractRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as MarketDefinitionService>::append_futures_contract(
                                        &inner,
                                        request,
                                    )
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = AppendFuturesContractSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/ficant.market.v1.MarketDefinitionService/AppendCalendar" => {
                    #[allow(non_camel_case_types)]
                    struct AppendCalendarSvc<T: MarketDefinitionService>(pub Arc<T>);
                    impl<
                        T: MarketDefinitionService,
                    > tonic::server::UnaryService<super::AppendCalendarRequest>
                    for AppendCalendarSvc<T> {
                        type Response = super::AppendCalendarResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::AppendCalendarRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as MarketDefinitionService>::append_calendar(
                                        &inner,
                                        request,
                                    )
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = AppendCalendarSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/ficant.market.v1.MarketDefinitionService/AppendUnit" => {
                    #[allow(non_camel_case_types)]
                    struct AppendUnitSvc<T: MarketDefinitionService>(pub Arc<T>);
                    impl<
                        T: MarketDefinitionService,
                    > tonic::server::UnaryService<super::AppendUnitRequest>
                    for AppendUnitSvc<T> {
                        type Response = super::AppendUnitResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::AppendUnitRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as MarketDefinitionService>::append_unit(&inner, request)
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = AppendUnitSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/ficant.market.v1.MarketDefinitionService/AppendMarketRulePack" => {
                    #[allow(non_camel_case_types)]
                    struct AppendMarketRulePackSvc<T: MarketDefinitionService>(
                        pub Arc<T>,
                    );
                    impl<
                        T: MarketDefinitionService,
                    > tonic::server::UnaryService<super::AppendMarketRulePackRequest>
                    for AppendMarketRulePackSvc<T> {
                        type Response = super::AppendMarketRulePackResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::AppendMarketRulePackRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as MarketDefinitionService>::append_market_rule_pack(
                                        &inner,
                                        request,
                                    )
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = AppendMarketRulePackSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/ficant.market.v1.MarketDefinitionService/GetDefinitionVersion" => {
                    #[allow(non_camel_case_types)]
                    struct GetDefinitionVersionSvc<T: MarketDefinitionService>(
                        pub Arc<T>,
                    );
                    impl<
                        T: MarketDefinitionService,
                    > tonic::server::UnaryService<super::GetDefinitionVersionRequest>
                    for GetDefinitionVersionSvc<T> {
                        type Response = super::GetDefinitionVersionResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::GetDefinitionVersionRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as MarketDefinitionService>::get_definition_version(
                                        &inner,
                                        request,
                                    )
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = GetDefinitionVersionSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/ficant.market.v1.MarketDefinitionService/ResolveDefinitionAsOf" => {
                    #[allow(non_camel_case_types)]
                    struct ResolveDefinitionAsOfSvc<T: MarketDefinitionService>(
                        pub Arc<T>,
                    );
                    impl<
                        T: MarketDefinitionService,
                    > tonic::server::UnaryService<super::ResolveDefinitionAsOfRequest>
                    for ResolveDefinitionAsOfSvc<T> {
                        type Response = super::ResolveDefinitionAsOfResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::ResolveDefinitionAsOfRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as MarketDefinitionService>::resolve_definition_as_of(
                                        &inner,
                                        request,
                                    )
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = ResolveDefinitionAsOfSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/ficant.market.v1.MarketDefinitionService/ListDefinitionVersions" => {
                    #[allow(non_camel_case_types)]
                    struct ListDefinitionVersionsSvc<T: MarketDefinitionService>(
                        pub Arc<T>,
                    );
                    impl<
                        T: MarketDefinitionService,
                    > tonic::server::UnaryService<super::ListDefinitionVersionsRequest>
                    for ListDefinitionVersionsSvc<T> {
                        type Response = super::ListDefinitionVersionsResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::ListDefinitionVersionsRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as MarketDefinitionService>::list_definition_versions(
                                        &inner,
                                        request,
                                    )
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = ListDefinitionVersionsSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                _ => {
                    Box::pin(async move {
                        let mut response = http::Response::new(
                            tonic::body::Body::default(),
                        );
                        let headers = response.headers_mut();
                        headers
                            .insert(
                                tonic::Status::GRPC_STATUS,
                                (tonic::Code::Unimplemented as i32).into(),
                            );
                        headers
                            .insert(
                                http::header::CONTENT_TYPE,
                                tonic::metadata::GRPC_CONTENT_TYPE,
                            );
                        Ok(response)
                    })
                }
            }
        }
    }
    impl<T> Clone for MarketDefinitionServiceServer<T> {
        fn clone(&self) -> Self {
            let inner = self.inner.clone();
            Self {
                inner,
                accept_compression_encodings: self.accept_compression_encodings,
                send_compression_encodings: self.send_compression_encodings,
                max_decoding_message_size: self.max_decoding_message_size,
                max_encoding_message_size: self.max_encoding_message_size,
            }
        }
    }
    /// Generated gRPC service name
    pub const SERVICE_NAME: &str = "ficant.market.v1.MarketDefinitionService";
    impl<T> tonic::server::NamedService for MarketDefinitionServiceServer<T> {
        const NAME: &'static str = SERVICE_NAME;
    }
}
/// Generated client implementations.
pub mod market_fact_service_client {
    #![allow(
        unused_variables,
        dead_code,
        missing_docs,
        clippy::wildcard_imports,
        clippy::let_unit_value,
    )]
    use tonic::codegen::*;
    use tonic::codegen::http::Uri;
    ///
    #[derive(Debug, Clone)]
    pub struct MarketFactServiceClient<T> {
        inner: tonic::client::Grpc<T>,
    }
    impl MarketFactServiceClient<tonic::transport::Channel> {
        /// Attempt to create a new client by connecting to a given endpoint.
        pub async fn connect<D>(dst: D) -> Result<Self, tonic::transport::Error>
        where
            D: TryInto<tonic::transport::Endpoint>,
            D::Error: Into<StdError>,
        {
            let conn = tonic::transport::Endpoint::new(dst)?.connect().await?;
            Ok(Self::new(conn))
        }
    }
    impl<T> MarketFactServiceClient<T>
    where
        T: tonic::client::GrpcService<tonic::body::Body>,
        T::Error: Into<StdError>,
        T::ResponseBody: Body<Data = Bytes> + std::marker::Send + 'static,
        <T::ResponseBody as Body>::Error: Into<StdError> + std::marker::Send,
    {
        pub fn new(inner: T) -> Self {
            let inner = tonic::client::Grpc::new(inner);
            Self { inner }
        }
        pub fn with_origin(inner: T, origin: Uri) -> Self {
            let inner = tonic::client::Grpc::with_origin(inner, origin);
            Self { inner }
        }
        pub fn with_interceptor<F>(
            inner: T,
            interceptor: F,
        ) -> MarketFactServiceClient<InterceptedService<T, F>>
        where
            F: tonic::service::Interceptor,
            T::ResponseBody: Default,
            T: tonic::codegen::Service<
                http::Request<tonic::body::Body>,
                Response = http::Response<
                    <T as tonic::client::GrpcService<tonic::body::Body>>::ResponseBody,
                >,
            >,
            <T as tonic::codegen::Service<
                http::Request<tonic::body::Body>,
            >>::Error: Into<StdError> + std::marker::Send + std::marker::Sync,
        {
            MarketFactServiceClient::new(InterceptedService::new(inner, interceptor))
        }
        /// Compress requests with the given encoding.
        ///
        /// This requires the server to support it otherwise it might respond with an
        /// error.
        #[must_use]
        pub fn send_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.inner = self.inner.send_compressed(encoding);
            self
        }
        /// Enable decompressing responses.
        #[must_use]
        pub fn accept_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.inner = self.inner.accept_compressed(encoding);
            self
        }
        /// Limits the maximum size of a decoded message.
        ///
        /// Default: `4MB`
        #[must_use]
        pub fn max_decoding_message_size(mut self, limit: usize) -> Self {
            self.inner = self.inner.max_decoding_message_size(limit);
            self
        }
        /// Limits the maximum size of an encoded message.
        ///
        /// Default: `usize::MAX`
        #[must_use]
        pub fn max_encoding_message_size(mut self, limit: usize) -> Self {
            self.inner = self.inner.max_encoding_message_size(limit);
            self
        }
        ///
        pub async fn append_cashflow(
            &mut self,
            request: impl tonic::IntoRequest<super::AppendCashflowRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendCashflowResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/ficant.market.v1.MarketFactService/AppendCashflow",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.market.v1.MarketFactService",
                        "AppendCashflow",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn append_quote(
            &mut self,
            request: impl tonic::IntoRequest<super::AppendQuoteRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendQuoteResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/ficant.market.v1.MarketFactService/AppendQuote",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("ficant.market.v1.MarketFactService", "AppendQuote"),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn append_trade(
            &mut self,
            request: impl tonic::IntoRequest<super::AppendTradeRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendTradeResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/ficant.market.v1.MarketFactService/AppendTrade",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("ficant.market.v1.MarketFactService", "AppendTrade"),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn append_valuation(
            &mut self,
            request: impl tonic::IntoRequest<super::AppendValuationRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendValuationResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/ficant.market.v1.MarketFactService/AppendValuation",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.market.v1.MarketFactService",
                        "AppendValuation",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn publish_curve_snapshot(
            &mut self,
            request: impl tonic::IntoRequest<super::PublishCurveSnapshotRequest>,
        ) -> std::result::Result<
            tonic::Response<super::PublishCurveSnapshotResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/ficant.market.v1.MarketFactService/PublishCurveSnapshot",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.market.v1.MarketFactService",
                        "PublishCurveSnapshot",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn query_instrument_facts(
            &mut self,
            request: impl tonic::IntoRequest<super::QueryInstrumentFactsRequest>,
        ) -> std::result::Result<
            tonic::Response<super::QueryInstrumentFactsResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/ficant.market.v1.MarketFactService/QueryInstrumentFacts",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.market.v1.MarketFactService",
                        "QueryInstrumentFacts",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn get_curve_snapshot(
            &mut self,
            request: impl tonic::IntoRequest<super::GetCurveSnapshotRequest>,
        ) -> std::result::Result<
            tonic::Response<super::GetCurveSnapshotResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/ficant.market.v1.MarketFactService/GetCurveSnapshot",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.market.v1.MarketFactService",
                        "GetCurveSnapshot",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
    }
}
/// Generated server implementations.
pub mod market_fact_service_server {
    #![allow(
        unused_variables,
        dead_code,
        missing_docs,
        clippy::wildcard_imports,
        clippy::let_unit_value,
    )]
    use tonic::codegen::*;
    /// Generated trait containing gRPC methods that should be implemented for use with MarketFactServiceServer.
    #[async_trait]
    pub trait MarketFactService: std::marker::Send + std::marker::Sync + 'static {
        ///
        async fn append_cashflow(
            &self,
            request: tonic::Request<super::AppendCashflowRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendCashflowResponse>,
            tonic::Status,
        >;
        ///
        async fn append_quote(
            &self,
            request: tonic::Request<super::AppendQuoteRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendQuoteResponse>,
            tonic::Status,
        >;
        ///
        async fn append_trade(
            &self,
            request: tonic::Request<super::AppendTradeRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendTradeResponse>,
            tonic::Status,
        >;
        ///
        async fn append_valuation(
            &self,
            request: tonic::Request<super::AppendValuationRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AppendValuationResponse>,
            tonic::Status,
        >;
        ///
        async fn publish_curve_snapshot(
            &self,
            request: tonic::Request<super::PublishCurveSnapshotRequest>,
        ) -> std::result::Result<
            tonic::Response<super::PublishCurveSnapshotResponse>,
            tonic::Status,
        >;
        ///
        async fn query_instrument_facts(
            &self,
            request: tonic::Request<super::QueryInstrumentFactsRequest>,
        ) -> std::result::Result<
            tonic::Response<super::QueryInstrumentFactsResponse>,
            tonic::Status,
        >;
        ///
        async fn get_curve_snapshot(
            &self,
            request: tonic::Request<super::GetCurveSnapshotRequest>,
        ) -> std::result::Result<
            tonic::Response<super::GetCurveSnapshotResponse>,
            tonic::Status,
        >;
    }
    ///
    #[derive(Debug)]
    pub struct MarketFactServiceServer<T> {
        inner: Arc<T>,
        accept_compression_encodings: EnabledCompressionEncodings,
        send_compression_encodings: EnabledCompressionEncodings,
        max_decoding_message_size: Option<usize>,
        max_encoding_message_size: Option<usize>,
    }
    impl<T> MarketFactServiceServer<T> {
        pub fn new(inner: T) -> Self {
            Self::from_arc(Arc::new(inner))
        }
        pub fn from_arc(inner: Arc<T>) -> Self {
            Self {
                inner,
                accept_compression_encodings: Default::default(),
                send_compression_encodings: Default::default(),
                max_decoding_message_size: None,
                max_encoding_message_size: None,
            }
        }
        pub fn with_interceptor<F>(
            inner: T,
            interceptor: F,
        ) -> InterceptedService<Self, F>
        where
            F: tonic::service::Interceptor,
        {
            InterceptedService::new(Self::new(inner), interceptor)
        }
        /// Enable decompressing requests with the given encoding.
        #[must_use]
        pub fn accept_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.accept_compression_encodings.enable(encoding);
            self
        }
        /// Compress responses with the given encoding, if the client supports it.
        #[must_use]
        pub fn send_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.send_compression_encodings.enable(encoding);
            self
        }
        /// Limits the maximum size of a decoded message.
        ///
        /// Default: `4MB`
        #[must_use]
        pub fn max_decoding_message_size(mut self, limit: usize) -> Self {
            self.max_decoding_message_size = Some(limit);
            self
        }
        /// Limits the maximum size of an encoded message.
        ///
        /// Default: `usize::MAX`
        #[must_use]
        pub fn max_encoding_message_size(mut self, limit: usize) -> Self {
            self.max_encoding_message_size = Some(limit);
            self
        }
    }
    impl<T, B> tonic::codegen::Service<http::Request<B>> for MarketFactServiceServer<T>
    where
        T: MarketFactService,
        B: Body + std::marker::Send + 'static,
        B::Error: Into<StdError> + std::marker::Send + 'static,
    {
        type Response = http::Response<tonic::body::Body>;
        type Error = std::convert::Infallible;
        type Future = BoxFuture<Self::Response, Self::Error>;
        fn poll_ready(
            &mut self,
            _cx: &mut Context<'_>,
        ) -> Poll<std::result::Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, req: http::Request<B>) -> Self::Future {
            match req.uri().path() {
                "/ficant.market.v1.MarketFactService/AppendCashflow" => {
                    #[allow(non_camel_case_types)]
                    struct AppendCashflowSvc<T: MarketFactService>(pub Arc<T>);
                    impl<
                        T: MarketFactService,
                    > tonic::server::UnaryService<super::AppendCashflowRequest>
                    for AppendCashflowSvc<T> {
                        type Response = super::AppendCashflowResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::AppendCashflowRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as MarketFactService>::append_cashflow(&inner, request)
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = AppendCashflowSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/ficant.market.v1.MarketFactService/AppendQuote" => {
                    #[allow(non_camel_case_types)]
                    struct AppendQuoteSvc<T: MarketFactService>(pub Arc<T>);
                    impl<
                        T: MarketFactService,
                    > tonic::server::UnaryService<super::AppendQuoteRequest>
                    for AppendQuoteSvc<T> {
                        type Response = super::AppendQuoteResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::AppendQuoteRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as MarketFactService>::append_quote(&inner, request)
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = AppendQuoteSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/ficant.market.v1.MarketFactService/AppendTrade" => {
                    #[allow(non_camel_case_types)]
                    struct AppendTradeSvc<T: MarketFactService>(pub Arc<T>);
                    impl<
                        T: MarketFactService,
                    > tonic::server::UnaryService<super::AppendTradeRequest>
                    for AppendTradeSvc<T> {
                        type Response = super::AppendTradeResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::AppendTradeRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as MarketFactService>::append_trade(&inner, request)
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = AppendTradeSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/ficant.market.v1.MarketFactService/AppendValuation" => {
                    #[allow(non_camel_case_types)]
                    struct AppendValuationSvc<T: MarketFactService>(pub Arc<T>);
                    impl<
                        T: MarketFactService,
                    > tonic::server::UnaryService<super::AppendValuationRequest>
                    for AppendValuationSvc<T> {
                        type Response = super::AppendValuationResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::AppendValuationRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as MarketFactService>::append_valuation(&inner, request)
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = AppendValuationSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/ficant.market.v1.MarketFactService/PublishCurveSnapshot" => {
                    #[allow(non_camel_case_types)]
                    struct PublishCurveSnapshotSvc<T: MarketFactService>(pub Arc<T>);
                    impl<
                        T: MarketFactService,
                    > tonic::server::UnaryService<super::PublishCurveSnapshotRequest>
                    for PublishCurveSnapshotSvc<T> {
                        type Response = super::PublishCurveSnapshotResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::PublishCurveSnapshotRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as MarketFactService>::publish_curve_snapshot(
                                        &inner,
                                        request,
                                    )
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = PublishCurveSnapshotSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/ficant.market.v1.MarketFactService/QueryInstrumentFacts" => {
                    #[allow(non_camel_case_types)]
                    struct QueryInstrumentFactsSvc<T: MarketFactService>(pub Arc<T>);
                    impl<
                        T: MarketFactService,
                    > tonic::server::UnaryService<super::QueryInstrumentFactsRequest>
                    for QueryInstrumentFactsSvc<T> {
                        type Response = super::QueryInstrumentFactsResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::QueryInstrumentFactsRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as MarketFactService>::query_instrument_facts(
                                        &inner,
                                        request,
                                    )
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = QueryInstrumentFactsSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/ficant.market.v1.MarketFactService/GetCurveSnapshot" => {
                    #[allow(non_camel_case_types)]
                    struct GetCurveSnapshotSvc<T: MarketFactService>(pub Arc<T>);
                    impl<
                        T: MarketFactService,
                    > tonic::server::UnaryService<super::GetCurveSnapshotRequest>
                    for GetCurveSnapshotSvc<T> {
                        type Response = super::GetCurveSnapshotResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::GetCurveSnapshotRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as MarketFactService>::get_curve_snapshot(
                                        &inner,
                                        request,
                                    )
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = GetCurveSnapshotSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                _ => {
                    Box::pin(async move {
                        let mut response = http::Response::new(
                            tonic::body::Body::default(),
                        );
                        let headers = response.headers_mut();
                        headers
                            .insert(
                                tonic::Status::GRPC_STATUS,
                                (tonic::Code::Unimplemented as i32).into(),
                            );
                        headers
                            .insert(
                                http::header::CONTENT_TYPE,
                                tonic::metadata::GRPC_CONTENT_TYPE,
                            );
                        Ok(response)
                    })
                }
            }
        }
    }
    impl<T> Clone for MarketFactServiceServer<T> {
        fn clone(&self) -> Self {
            let inner = self.inner.clone();
            Self {
                inner,
                accept_compression_encodings: self.accept_compression_encodings,
                send_compression_encodings: self.send_compression_encodings,
                max_decoding_message_size: self.max_decoding_message_size,
                max_encoding_message_size: self.max_encoding_message_size,
            }
        }
    }
    /// Generated gRPC service name
    pub const SERVICE_NAME: &str = "ficant.market.v1.MarketFactService";
    impl<T> tonic::server::NamedService for MarketFactServiceServer<T> {
        const NAME: &'static str = SERVICE_NAME;
    }
}
