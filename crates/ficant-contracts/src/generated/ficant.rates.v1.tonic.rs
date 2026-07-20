// @generated
/// Generated client implementations.
pub mod rates_analytics_service_client {
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
    pub struct RatesAnalyticsServiceClient<T> {
        inner: tonic::client::Grpc<T>,
    }
    impl RatesAnalyticsServiceClient<tonic::transport::Channel> {
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
    impl<T> RatesAnalyticsServiceClient<T>
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
        ) -> RatesAnalyticsServiceClient<InterceptedService<T, F>>
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
            RatesAnalyticsServiceClient::new(InterceptedService::new(inner, interceptor))
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
        pub async fn analyze_bond(
            &mut self,
            request: impl tonic::IntoRequest<super::AnalyzeBondRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AnalyzeBondResponse>,
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
                "/ficant.rates.v1.RatesAnalyticsService/AnalyzeBond",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.rates.v1.RatesAnalyticsService",
                        "AnalyzeBond",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn interpolate_yield_curve(
            &mut self,
            request: impl tonic::IntoRequest<super::InterpolateYieldCurveRequest>,
        ) -> std::result::Result<
            tonic::Response<super::InterpolateYieldCurveResponse>,
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
                "/ficant.rates.v1.RatesAnalyticsService/InterpolateYieldCurve",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.rates.v1.RatesAnalyticsService",
                        "InterpolateYieldCurve",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn analyze_carry_roll(
            &mut self,
            request: impl tonic::IntoRequest<super::AnalyzeCarryRollRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AnalyzeCarryRollResponse>,
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
                "/ficant.rates.v1.RatesAnalyticsService/AnalyzeCarryRoll",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.rates.v1.RatesAnalyticsService",
                        "AnalyzeCarryRoll",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn analyze_futures_delivery(
            &mut self,
            request: impl tonic::IntoRequest<super::AnalyzeFuturesDeliveryRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AnalyzeFuturesDeliveryResponse>,
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
                "/ficant.rates.v1.RatesAnalyticsService/AnalyzeFuturesDelivery",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.rates.v1.RatesAnalyticsService",
                        "AnalyzeFuturesDelivery",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn analyze_futures_hedge(
            &mut self,
            request: impl tonic::IntoRequest<super::AnalyzeFuturesHedgeRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AnalyzeFuturesHedgeResponse>,
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
                "/ficant.rates.v1.RatesAnalyticsService/AnalyzeFuturesHedge",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "ficant.rates.v1.RatesAnalyticsService",
                        "AnalyzeFuturesHedge",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
    }
}
/// Generated server implementations.
pub mod rates_analytics_service_server {
    #![allow(
        unused_variables,
        dead_code,
        missing_docs,
        clippy::wildcard_imports,
        clippy::let_unit_value,
    )]
    use tonic::codegen::*;
    /// Generated trait containing gRPC methods that should be implemented for use with RatesAnalyticsServiceServer.
    #[async_trait]
    pub trait RatesAnalyticsService: std::marker::Send + std::marker::Sync + 'static {
        ///
        async fn analyze_bond(
            &self,
            request: tonic::Request<super::AnalyzeBondRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AnalyzeBondResponse>,
            tonic::Status,
        >;
        ///
        async fn interpolate_yield_curve(
            &self,
            request: tonic::Request<super::InterpolateYieldCurveRequest>,
        ) -> std::result::Result<
            tonic::Response<super::InterpolateYieldCurveResponse>,
            tonic::Status,
        >;
        ///
        async fn analyze_carry_roll(
            &self,
            request: tonic::Request<super::AnalyzeCarryRollRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AnalyzeCarryRollResponse>,
            tonic::Status,
        >;
        ///
        async fn analyze_futures_delivery(
            &self,
            request: tonic::Request<super::AnalyzeFuturesDeliveryRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AnalyzeFuturesDeliveryResponse>,
            tonic::Status,
        >;
        ///
        async fn analyze_futures_hedge(
            &self,
            request: tonic::Request<super::AnalyzeFuturesHedgeRequest>,
        ) -> std::result::Result<
            tonic::Response<super::AnalyzeFuturesHedgeResponse>,
            tonic::Status,
        >;
    }
    ///
    #[derive(Debug)]
    pub struct RatesAnalyticsServiceServer<T> {
        inner: Arc<T>,
        accept_compression_encodings: EnabledCompressionEncodings,
        send_compression_encodings: EnabledCompressionEncodings,
        max_decoding_message_size: Option<usize>,
        max_encoding_message_size: Option<usize>,
    }
    impl<T> RatesAnalyticsServiceServer<T> {
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
    for RatesAnalyticsServiceServer<T>
    where
        T: RatesAnalyticsService,
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
                "/ficant.rates.v1.RatesAnalyticsService/AnalyzeBond" => {
                    #[allow(non_camel_case_types)]
                    struct AnalyzeBondSvc<T: RatesAnalyticsService>(pub Arc<T>);
                    impl<
                        T: RatesAnalyticsService,
                    > tonic::server::UnaryService<super::AnalyzeBondRequest>
                    for AnalyzeBondSvc<T> {
                        type Response = super::AnalyzeBondResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::AnalyzeBondRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as RatesAnalyticsService>::analyze_bond(&inner, request)
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
                        let method = AnalyzeBondSvc(inner);
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
                "/ficant.rates.v1.RatesAnalyticsService/InterpolateYieldCurve" => {
                    #[allow(non_camel_case_types)]
                    struct InterpolateYieldCurveSvc<T: RatesAnalyticsService>(
                        pub Arc<T>,
                    );
                    impl<
                        T: RatesAnalyticsService,
                    > tonic::server::UnaryService<super::InterpolateYieldCurveRequest>
                    for InterpolateYieldCurveSvc<T> {
                        type Response = super::InterpolateYieldCurveResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::InterpolateYieldCurveRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as RatesAnalyticsService>::interpolate_yield_curve(
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
                        let method = InterpolateYieldCurveSvc(inner);
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
                "/ficant.rates.v1.RatesAnalyticsService/AnalyzeCarryRoll" => {
                    #[allow(non_camel_case_types)]
                    struct AnalyzeCarryRollSvc<T: RatesAnalyticsService>(pub Arc<T>);
                    impl<
                        T: RatesAnalyticsService,
                    > tonic::server::UnaryService<super::AnalyzeCarryRollRequest>
                    for AnalyzeCarryRollSvc<T> {
                        type Response = super::AnalyzeCarryRollResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::AnalyzeCarryRollRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as RatesAnalyticsService>::analyze_carry_roll(
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
                        let method = AnalyzeCarryRollSvc(inner);
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
                "/ficant.rates.v1.RatesAnalyticsService/AnalyzeFuturesDelivery" => {
                    #[allow(non_camel_case_types)]
                    struct AnalyzeFuturesDeliverySvc<T: RatesAnalyticsService>(
                        pub Arc<T>,
                    );
                    impl<
                        T: RatesAnalyticsService,
                    > tonic::server::UnaryService<super::AnalyzeFuturesDeliveryRequest>
                    for AnalyzeFuturesDeliverySvc<T> {
                        type Response = super::AnalyzeFuturesDeliveryResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::AnalyzeFuturesDeliveryRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as RatesAnalyticsService>::analyze_futures_delivery(
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
                        let method = AnalyzeFuturesDeliverySvc(inner);
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
                "/ficant.rates.v1.RatesAnalyticsService/AnalyzeFuturesHedge" => {
                    #[allow(non_camel_case_types)]
                    struct AnalyzeFuturesHedgeSvc<T: RatesAnalyticsService>(pub Arc<T>);
                    impl<
                        T: RatesAnalyticsService,
                    > tonic::server::UnaryService<super::AnalyzeFuturesHedgeRequest>
                    for AnalyzeFuturesHedgeSvc<T> {
                        type Response = super::AnalyzeFuturesHedgeResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::AnalyzeFuturesHedgeRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as RatesAnalyticsService>::analyze_futures_hedge(
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
                        let method = AnalyzeFuturesHedgeSvc(inner);
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
    impl<T> Clone for RatesAnalyticsServiceServer<T> {
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
    pub const SERVICE_NAME: &str = "ficant.rates.v1.RatesAnalyticsService";
    impl<T> tonic::server::NamedService for RatesAnalyticsServiceServer<T> {
        const NAME: &'static str = SERVICE_NAME;
    }
}
