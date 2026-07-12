use ficant_api::{
    AppRegistration, Clock, CspPolicy, PlatformApplication, PlatformGrpcService, SafeErrorMapper,
    SessionPolicy, TrustedIdentity,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_contracts::ficant::app::v1::{
    AuthorizeAppLaunchRequest, ErrorCode, GetAppRegistryRequest, GetCurrentSessionRequest,
    RefreshAppLaunchRequest, RefreshSessionRequest, RevokeAppLaunchRequest, RevokeSessionRequest,
    app_launch_authorization_response, get_app_registry_response, get_current_session_response,
    platform_service_server::PlatformService, refresh_session_response, revoke_app_launch_response,
    revoke_session_response,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use tonic::Request;

const SIGNING_KEY: &[u8] = b"0123456789abcdef0123456789abcdef";
const TRACE_KEY: &[u8] = b"trace-key-0123456789abcdef-00001";
const MAIN_TOKEN: &str = "primary-token-that-must-not-leak";

#[derive(Default)]
struct TestClock(AtomicI64);

impl TestClock {
    fn at(seconds: i64) -> Self {
        Self(AtomicI64::new(seconds))
    }

    fn set(&self, seconds: i64) {
        self.0.store(seconds, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now_unix_seconds(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

fn request<T>(message: T, token: &str) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid metadata"),
    );
    request
}

fn app(allowed_subjects: &[&str]) -> AppRegistration {
    AppRegistration::try_new(
        "fixture.rates",
        "Rates fixture",
        "/rates/index.html",
        "https://apps.example.test",
        ["rates:read", "rates:write"],
        allowed_subjects.iter().copied(),
        ["rates:read"],
        [
            CspPolicy::new("default-src", ["'none'"]).expect("valid default CSP"),
            CspPolicy::new("connect-src", ["'self'"]).expect("valid connect CSP"),
        ],
        ["allow-scripts"],
    )
    .expect("valid app fixture")
}

fn identity(subject: &str, token: &str) -> TrustedIdentity {
    TrustedIdentity::bearer(subject, token.as_bytes(), ["rates:read", "other:read"])
        .expect("valid identity")
}

fn service(
    clock: Arc<TestClock>,
    identities: Vec<TrustedIdentity>,
    implicit: Option<TrustedIdentity>,
    apps: Vec<AppRegistration>,
) -> PlatformGrpcService {
    let application = PlatformApplication::try_new(
        clock,
        SessionPolicy::new(900, 60).expect("valid policy"),
        SIGNING_KEY,
        identities,
        implicit,
        apps,
    )
    .expect("valid platform application");
    PlatformGrpcService::new(Arc::new(application), TRACE_KEY).expect("valid service")
}

#[tokio::test]
async fn authorized_subject_receives_real_empty_registry() {
    let service = service(
        Arc::new(TestClock::at(1_000)),
        vec![identity("analyst-1", MAIN_TOKEN)],
        None,
        vec![],
    );

    let current = service
        .get_current_session(request(GetCurrentSessionRequest {}, MAIN_TOKEN))
        .await
        .expect("transport succeeds")
        .into_inner();
    assert!(matches!(
        current.result,
        Some(get_current_session_response::Result::Session(_))
    ));

    let registry = service
        .get_app_registry(request(GetAppRegistryRequest {}, MAIN_TOKEN))
        .await
        .expect("transport succeeds")
        .into_inner();
    let Some(get_app_registry_response::Result::Registry(registry)) = registry.result else {
        panic!("authorized request must return a registry result");
    };
    assert!(registry.apps.is_empty());
}

#[tokio::test]
async fn registry_filters_denied_subject_and_grant_is_scoped_short_lived() {
    let clock = Arc::new(TestClock::at(2_000));
    let service = service(
        Arc::clone(&clock),
        vec![
            identity("analyst-1", MAIN_TOKEN),
            identity("analyst-2", "denied-primary-token"),
        ],
        None,
        vec![app(&["analyst-1"])],
    );

    service
        .get_current_session(request(GetCurrentSessionRequest {}, MAIN_TOKEN))
        .await
        .expect("authorized session is issued");
    service
        .get_current_session(request(GetCurrentSessionRequest {}, "denied-primary-token"))
        .await
        .expect("denied subject still has a valid platform session");

    let allowed = service
        .get_app_registry(request(GetAppRegistryRequest {}, MAIN_TOKEN))
        .await
        .expect("transport succeeds")
        .into_inner();
    let Some(get_app_registry_response::Result::Registry(allowed)) = allowed.result else {
        panic!("allowed subject must receive registry");
    };
    assert_eq!(allowed.apps.len(), 1);

    let denied = service
        .get_app_registry(request(GetAppRegistryRequest {}, "denied-primary-token"))
        .await
        .expect("transport succeeds")
        .into_inner();
    let Some(get_app_registry_response::Result::Registry(denied)) = denied.result else {
        panic!("denied subject receives a filtered registry");
    };
    assert!(denied.apps.is_empty());

    let grant = service
        .authorize_app_launch(request(
            AuthorizeAppLaunchRequest {
                app_id: "fixture.rates".to_owned(),
            },
            MAIN_TOKEN,
        ))
        .await
        .expect("transport succeeds")
        .into_inner();
    let Some(app_launch_authorization_response::Result::Grant(grant)) = grant.result else {
        panic!("allowed subject receives a launch grant");
    };
    assert_eq!(grant.scopes, ["rates:read"]);
    assert_eq!(
        grant.issued_at.as_ref().map(|value| value.seconds),
        Some(2_000)
    );
    assert_eq!(
        grant.expires_at.as_ref().map(|value| value.seconds),
        Some(2_060)
    );
    assert!(!grant.launch_credential.is_empty());
    assert_ne!(grant.launch_credential, MAIN_TOKEN.as_bytes());
    assert!(!grant.entrypoint.contains("token"));
    assert!(!grant.entrypoint.contains("credential"));

    let denied_grant = service
        .authorize_app_launch(request(
            AuthorizeAppLaunchRequest {
                app_id: "fixture.rates".to_owned(),
            },
            "denied-primary-token",
        ))
        .await
        .expect("transport succeeds")
        .into_inner();
    let Some(app_launch_authorization_response::Result::Error(error)) = denied_grant.result else {
        panic!("denied subject must not receive a grant");
    };
    assert_eq!(error.code, ErrorCode::Forbidden as i32);
}

#[tokio::test]
async fn session_is_issued_refreshed_and_expires_at_the_frozen_boundary() {
    let clock = Arc::new(TestClock::at(3_000));
    let service = service(
        Arc::clone(&clock),
        vec![identity("analyst-1", MAIN_TOKEN)],
        None,
        vec![],
    );

    let issued = service
        .get_current_session(request(GetCurrentSessionRequest {}, MAIN_TOKEN))
        .await
        .expect("transport succeeds")
        .into_inner();
    let Some(get_current_session_response::Result::Session(issued)) = issued.result else {
        panic!("session must be issued");
    };
    assert_eq!(
        issued.expires_at.as_ref().map(|value| value.seconds),
        Some(3_900)
    );

    clock.set(3_500);
    let refreshed = service
        .refresh_session(request(RefreshSessionRequest {}, MAIN_TOKEN))
        .await
        .expect("transport succeeds")
        .into_inner();
    let Some(refresh_session_response::Result::Session(refreshed)) = refreshed.result else {
        panic!("active session must refresh");
    };
    assert_ne!(refreshed.session_id, issued.session_id);
    assert_eq!(
        refreshed.expires_at.as_ref().map(|value| value.seconds),
        Some(4_400)
    );

    clock.set(4_401);
    let expired = service
        .get_current_session(request(GetCurrentSessionRequest {}, MAIN_TOKEN))
        .await
        .expect("transport succeeds")
        .into_inner();
    let Some(get_current_session_response::Result::Error(expired)) = expired.result else {
        panic!("expired session must be an explicit safe error");
    };
    assert_eq!(expired.code, ErrorCode::Expired as i32);
    assert!(!expired.retryable);
}

#[tokio::test]
async fn unauthenticated_error_has_stable_code_and_trace_without_credential_leakage() {
    let service = service(
        Arc::new(TestClock::at(4_000)),
        vec![identity("analyst-1", MAIN_TOKEN)],
        None,
        vec![],
    );

    let first = service
        .get_current_session(request(GetCurrentSessionRequest {}, "wrong-secret"))
        .await
        .expect("transport succeeds")
        .into_inner();
    let second = service
        .get_current_session(request(GetCurrentSessionRequest {}, "wrong-secret"))
        .await
        .expect("transport succeeds")
        .into_inner();
    let Some(get_current_session_response::Result::Error(first)) = first.result else {
        panic!("bad credential must be a safe error");
    };
    let Some(get_current_session_response::Result::Error(second)) = second.result else {
        panic!("bad credential must be a safe error");
    };
    assert_eq!(first.code, ErrorCode::Unauthenticated as i32);
    assert_eq!(first.trace_id, second.trace_id);
    assert!(!first.trace_id.is_empty());
    assert!(!first.trace_id.contains("wrong-secret"));
    assert!(!first.safe_message.contains("wrong-secret"));
}

#[tokio::test]
async fn app_and_session_revocation_close_the_frozen_authorization_lifecycle() {
    let service = service(
        Arc::new(TestClock::at(5_000)),
        vec![identity("analyst-1", MAIN_TOKEN)],
        None,
        vec![app(&["analyst-1"])],
    );
    service
        .get_current_session(request(GetCurrentSessionRequest {}, MAIN_TOKEN))
        .await
        .expect("session is issued");

    let authorized = service
        .authorize_app_launch(request(
            AuthorizeAppLaunchRequest {
                app_id: "fixture.rates".to_owned(),
            },
            MAIN_TOKEN,
        ))
        .await
        .expect("transport succeeds")
        .into_inner();
    let Some(app_launch_authorization_response::Result::Grant(authorized)) = authorized.result
    else {
        panic!("authorization must issue a grant");
    };

    let refreshed = service
        .refresh_app_launch(request(
            RefreshAppLaunchRequest {
                app_id: "fixture.rates".to_owned(),
            },
            MAIN_TOKEN,
        ))
        .await
        .expect("transport succeeds")
        .into_inner();
    let Some(app_launch_authorization_response::Result::Grant(refreshed)) = refreshed.result else {
        panic!("active grant must refresh");
    };
    assert_ne!(refreshed.launch_credential, authorized.launch_credential);

    let revoked_app = service
        .revoke_app_launch(request(
            RevokeAppLaunchRequest {
                app_id: "fixture.rates".to_owned(),
            },
            MAIN_TOKEN,
        ))
        .await
        .expect("transport succeeds")
        .into_inner();
    assert!(matches!(
        revoked_app.result,
        Some(revoke_app_launch_response::Result::Revocation(_))
    ));

    let refresh_after_revoke = service
        .refresh_app_launch(request(
            RefreshAppLaunchRequest {
                app_id: "fixture.rates".to_owned(),
            },
            MAIN_TOKEN,
        ))
        .await
        .expect("transport succeeds")
        .into_inner();
    let Some(app_launch_authorization_response::Result::Error(error)) = refresh_after_revoke.result
    else {
        panic!("revoked grant must not refresh");
    };
    assert_eq!(error.code, ErrorCode::Expired as i32);

    let revoked_session = service
        .revoke_session(request(RevokeSessionRequest {}, MAIN_TOKEN))
        .await
        .expect("transport succeeds")
        .into_inner();
    assert!(matches!(
        revoked_session.result,
        Some(revoke_session_response::Result::Revocation(_))
    ));

    let registry_after_revoke = service
        .get_app_registry(request(GetAppRegistryRequest {}, MAIN_TOKEN))
        .await
        .expect("transport succeeds")
        .into_inner();
    let Some(get_app_registry_response::Result::Error(error)) = registry_after_revoke.result else {
        panic!("revoked session must not reach the registry");
    };
    assert_eq!(error.code, ErrorCode::Unauthenticated as i32);
}

#[test]
fn application_port_error_maps_to_stable_safe_transport_error() {
    let mapper = SafeErrorMapper::new(TRACE_KEY).expect("valid trace key");
    let application = ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, true);

    let first = mapper.map_application("read-definition", &application);
    let second = mapper.map_application("read-definition", &application);

    assert_eq!(first.code, ErrorCode::Unavailable as i32);
    assert!(first.retryable);
    assert_eq!(first.trace_id, second.trace_id);
    assert!(!first.trace_id.is_empty());
}

#[test]
fn app_registration_rejects_loopback_prefix_lookalike_origin() {
    let registration = AppRegistration::try_new(
        "unsafe.app",
        "Unsafe app",
        "/index.html",
        "http://127.0.0.1.evil.example",
        ["rates:read"],
        ["analyst-1"],
        ["rates:read"],
        [CspPolicy::new("default-src", ["'none'"]).expect("valid CSP")],
        ["allow-scripts"],
    );

    assert!(registration.is_err());
}

#[test]
fn app_registration_uses_frozen_path_csp_and_sandbox_boundary() {
    let csp = [
        CspPolicy::new("default-src", ["'none'"]).expect("valid default CSP"),
        CspPolicy::new("connect-src", ["'self'"]).expect("valid connect CSP"),
    ];
    let full_url = AppRegistration::try_new(
        "unsafe.full-url",
        "Unsafe full URL",
        "https://apps.example.test/rates/index.html",
        "https://apps.example.test",
        ["rates:read"],
        ["analyst-1"],
        ["rates:read"],
        csp.clone(),
        ["allow-scripts"],
    );
    assert!(
        full_url.is_err(),
        "entrypoint is a frozen absolute path, not a URL"
    );

    let valid = AppRegistration::try_new(
        "safe.path",
        "Safe path",
        "/rates/index.html",
        "https://apps.example.test",
        ["rates:read"],
        ["analyst-1"],
        ["rates:read"],
        csp,
        ["allow-scripts"],
    );
    assert!(valid.is_ok());

    let unsafe_sandbox = AppRegistration::try_new(
        "unsafe.sandbox",
        "Unsafe sandbox",
        "/rates/index.html",
        "https://apps.example.test",
        ["rates:read"],
        ["analyst-1"],
        ["rates:read"],
        [CspPolicy::new("default-src", ["'none'"]).expect("valid default CSP")],
        ["allow-scripts", "allow-same-origin"],
    );
    assert!(unsafe_sandbox.is_err());
}
