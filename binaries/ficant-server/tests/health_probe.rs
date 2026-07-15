use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::process::{Command, Output};

const SECRET_SENTINEL: &str = "must-never-be-read-or-printed";

#[test]
fn health_check_succeeds_when_configured_endpoint_accepts_connections() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback listener binds");
    let address = listener.local_addr().expect("listener has address");

    let output = run(&["--health-check"], &address.to_string(), &[]);

    assert_success(&output);
}

#[test]
fn health_check_normalizes_unspecified_addresses_to_loopback() {
    let ipv4 = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("IPv4 loopback listener binds");
    let ipv4_port = ipv4.local_addr().expect("listener has address").port();
    assert_success(&run(
        &["--health-check"],
        &SocketAddr::from((Ipv4Addr::UNSPECIFIED, ipv4_port)).to_string(),
        &[],
    ));

    let ipv6 = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).expect("IPv6 loopback listener binds");
    let ipv6_port = ipv6.local_addr().expect("listener has address").port();
    assert_success(&run(
        &["--health-check"],
        &SocketAddr::from((Ipv6Addr::UNSPECIFIED, ipv6_port)).to_string(),
        &[],
    ));
}

#[test]
fn health_check_fails_for_closed_or_invalid_endpoint() {
    let closed = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("port reservation binds");
    let closed_address = closed.local_addr().expect("listener has address");
    drop(closed);

    let closed_output = run(&["--health-check"], &closed_address.to_string(), &[]);
    assert!(!closed_output.status.success());
    assert!(stderr(&closed_output).contains("health check failed"));

    let invalid_output = run(&["--health-check"], "not-a-socket", &[]);
    assert!(!invalid_output.status.success());
    assert!(stderr(&invalid_output).contains("FICANT_GRPC_BIND"));
}

#[test]
fn health_check_does_not_read_or_disclose_platform_secrets() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback listener binds");
    let address = listener.local_addr().expect("listener has address");
    let output = run(
        &["--health-check"],
        &address.to_string(),
        &[
            ("FICANT_PLATFORM_SIGNING_KEY_HEX", SECRET_SENTINEL),
            ("FICANT_PLATFORM_TRACE_KEY_HEX", SECRET_SENTINEL),
            ("FICANT_BOOTSTRAP_BEARER_TOKEN", SECRET_SENTINEL),
        ],
    );

    assert_success(&output);
    assert!(!stderr(&output).contains(SECRET_SENTINEL));
}

#[test]
fn unknown_arguments_fail_with_usage_without_reading_secrets() {
    let output = run(
        &["--unknown"],
        "not-a-socket",
        &[("FICANT_PLATFORM_SIGNING_KEY_HEX", SECRET_SENTINEL)],
    );

    assert!(!output.status.success());
    assert!(stderr(&output).contains("usage: ficant-server [--health-check]"));
    assert!(!stderr(&output).contains(SECRET_SENTINEL));
}

fn run(arguments: &[&str], bind: &str, extra_environment: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ficant-server"));
    command
        .args(arguments)
        .env_clear()
        .env("FICANT_GRPC_BIND", bind);
    #[cfg(windows)]
    for key in ["SystemRoot", "WINDIR"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    for (key, value) in extra_environment {
        command.env(key, value);
    }
    command.output().expect("server process runs")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected success, stderr was: {}",
        stderr(output)
    );
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
