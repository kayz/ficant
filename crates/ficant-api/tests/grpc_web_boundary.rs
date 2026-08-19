use ficant_api::{
    GrpcWebServerConfig, PlatformApplication, PlatformGrpcService, SessionPolicy, SystemClock,
    TrustedIdentity, serve_grpc_web,
};
use ficant_domain::governance::PlatformRole;
use ficant_domain::primitives::Ulid;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

const SIGNING_KEY: &[u8] = b"0123456789abcdef0123456789abcdef";
const TRACE_KEY: &[u8] = b"trace-key-0123456789abcdef-00001";
const ALLOWED_ORIGIN: &str = "http://127.0.0.1:4174";

#[tokio::test(flavor = "multi_thread")]
async fn grpc_web_endpoint_serves_real_session_with_exact_cors_boundary() {
    let address = free_address();
    let application = PlatformApplication::try_new(
        Arc::new(SystemClock),
        SessionPolicy::new(900, 60).expect("valid policy"),
        SIGNING_KEY,
        vec![],
        Some(
            TrustedIdentity::implicit(
                "browser-user",
                id('A'),
                id('T'),
                vec![id('B')],
                PlatformRole::Researcher,
                ["rates:read"],
            )
            .expect("valid implicit identity"),
        ),
        vec![],
    )
    .expect("valid application");
    let service =
        PlatformGrpcService::new(Arc::new(application), TRACE_KEY).expect("valid service");
    let server = tokio::spawn(serve_grpc_web(
        GrpcWebServerConfig {
            bind: address,
            allowed_origins: vec![ALLOWED_ORIGIN.to_owned()],
        },
        service,
    ));
    wait_until_listening(address).await;

    let preflight = exchange(
        address,
        format!(
            "OPTIONS /ficant.app.v1.PlatformService/GetCurrentSession HTTP/1.1\r\nHost: {address}\r\nOrigin: {ALLOWED_ORIGIN}\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: content-type,x-grpc-web\r\nConnection: close\r\n\r\n"
        )
        .into_bytes(),
    )
    .await;
    assert!(preflight.starts_with("http/1.1 204 no content\r\n"));
    assert!(preflight.contains(&format!(
        "access-control-allow-origin: {ALLOWED_ORIGIN}\r\n"
    )));
    assert!(!preflight.contains("access-control-allow-origin: *"));

    let denied = exchange(
        address,
        format!(
            "OPTIONS /ficant.app.v1.PlatformService/GetCurrentSession HTTP/1.1\r\nHost: {address}\r\nOrigin: https://evil.example\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: content-type,x-grpc-web\r\nConnection: close\r\n\r\n"
        )
        .into_bytes(),
    )
    .await;
    assert!(denied.starts_with("http/1.1 403 forbidden\r\n"));
    assert!(!denied.contains("access-control-allow-origin"));

    let mut request = format!(
        "POST /ficant.app.v1.PlatformService/GetCurrentSession HTTP/1.1\r\nHost: {address}\r\nOrigin: {ALLOWED_ORIGIN}\r\nContent-Type: application/grpc-web+proto\r\nX-Grpc-Web: 1\r\nContent-Length: 5\r\nConnection: close\r\n\r\n"
    )
    .into_bytes();
    request.extend_from_slice(&[0, 0, 0, 0, 0]);
    let response = exchange(address, request).await;
    assert!(response.starts_with("http/1.1 200 ok\r\n"));
    assert!(response.contains("content-type: application/grpc-web+proto\r\n"));
    assert!(response.contains(&format!(
        "access-control-allow-origin: {ALLOWED_ORIGIN}\r\n"
    )));

    server.abort();
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5F0{suffix}")).unwrap()
}

fn free_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral listener binds");
    listener.local_addr().expect("listener has address")
}

async fn wait_until_listening(address: SocketAddr) {
    for _ in 0..100 {
        if TcpStream::connect_timeout(&address, Duration::from_millis(20)).is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("gRPC-Web server did not listen on {address}");
}

async fn exchange(address: SocketAddr, request: Vec<u8>) -> String {
    tokio::task::spawn_blocking(move || {
        let mut stream =
            TcpStream::connect_timeout(&address, Duration::from_secs(2)).expect("client connects");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout configured");
        stream.write_all(&request).expect("request writes");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("response reads");
        String::from_utf8_lossy(&response).to_ascii_lowercase()
    })
    .await
    .expect("blocking exchange joins")
}
