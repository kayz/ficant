use crate::error::{PlatformFailure, PlatformFailureCode};
use crate::session::{Clock, SessionPolicy, TrustedIdentity, credential_matches};
use ring::hmac;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tonic::codegen::http::Uri;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CspPolicy {
    pub(crate) name: String,
    pub(crate) values: Vec<String>,
}

impl CspPolicy {
    /// Builds a header-safe CSP directive.
    ///
    /// # Errors
    ///
    /// Returns an error for empty or newline-bearing names and values.
    pub fn new<I, S>(name: impl Into<String>, values: I) -> Result<Self, &'static str>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let name = name.into();
        let values: Vec<String> = values.into_iter().map(Into::into).collect();
        if !matches!(
            name.as_str(),
            "default-src"
                | "connect-src"
                | "font-src"
                | "frame-src"
                | "img-src"
                | "script-src"
                | "style-src"
        ) || values.is_empty()
            || name.contains(['\r', '\n'])
            || values.iter().any(|value| !valid_csp_source(value))
        {
            return Err("CSP directive must be allowlisted, non-empty, and source-safe");
        }
        Ok(Self { name, values })
    }
}

#[derive(Clone, Debug)]
pub struct AppRegistration {
    pub(crate) app_id: String,
    pub(crate) display_name: String,
    pub(crate) entrypoint: String,
    pub(crate) allowed_origin: String,
    pub(crate) capabilities: Vec<String>,
    pub(crate) allowed_subjects: BTreeSet<String>,
    pub(crate) grant_scopes: Vec<String>,
    pub(crate) csp: Vec<CspPolicy>,
    pub(crate) sandbox_tokens: Vec<String>,
}

impl AppRegistration {
    /// Builds one validated application registration.
    ///
    /// # Errors
    ///
    /// Returns an error when identity, origin, entrypoint, or sandbox policy is unsafe.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new<CI, CS, AI, AS, GI, GS, PI, SI, SS>(
        app_id: impl Into<String>,
        display_name: impl Into<String>,
        entrypoint: impl Into<String>,
        allowed_origin: impl Into<String>,
        capabilities: CI,
        allowed_subjects: AI,
        grant_scopes: GI,
        csp: PI,
        sandbox_tokens: SI,
    ) -> Result<Self, &'static str>
    where
        CI: IntoIterator<Item = CS>,
        CS: Into<String>,
        AI: IntoIterator<Item = AS>,
        AS: Into<String>,
        GI: IntoIterator<Item = GS>,
        GS: Into<String>,
        PI: IntoIterator<Item = CspPolicy>,
        SI: IntoIterator<Item = SS>,
        SS: Into<String>,
    {
        let app_id = app_id.into();
        let display_name = display_name.into();
        let entrypoint = entrypoint.into();
        let allowed_origin = allowed_origin.into();
        if app_id.is_empty() || display_name.trim().is_empty() {
            return Err("app ID and display name must not be empty");
        }
        if !valid_app_origin(&allowed_origin) || !valid_entrypoint(&entrypoint) {
            return Err(
                "entrypoint must be a credential-free absolute path under the exact origin",
            );
        }
        let mut capabilities: Vec<String> = capabilities.into_iter().map(Into::into).collect();
        capabilities.sort();
        capabilities.dedup();
        let allowed_subjects = allowed_subjects.into_iter().map(Into::into).collect();
        let mut grant_scopes: Vec<String> = grant_scopes.into_iter().map(Into::into).collect();
        grant_scopes.sort();
        grant_scopes.dedup();
        if grant_scopes
            .iter()
            .any(|scope| capabilities.binary_search(scope).is_err())
        {
            return Err("grant scopes must be declared registry capabilities");
        }
        let csp: Vec<CspPolicy> = csp.into_iter().collect();
        let mut csp_names = BTreeSet::new();
        if csp
            .iter()
            .any(|policy| !csp_names.insert(policy.name.clone()))
            || !csp.iter().any(|policy| {
                policy.name == "default-src" && policy.values.as_slice() == [String::from("'none'")]
            })
        {
            return Err("CSP must have unique directives and default-src 'none'");
        }
        let mut sandbox_tokens: Vec<String> = sandbox_tokens.into_iter().map(Into::into).collect();
        sandbox_tokens.sort();
        sandbox_tokens.dedup();
        if sandbox_tokens.is_empty()
            || sandbox_tokens.iter().any(|token| {
                !matches!(
                    token.as_str(),
                    "allow-downloads"
                        | "allow-forms"
                        | "allow-modals"
                        | "allow-same-origin"
                        | "allow-scripts"
                )
            })
            || (sandbox_tokens
                .binary_search(&String::from("allow-scripts"))
                .is_ok()
                && sandbox_tokens
                    .binary_search(&String::from("allow-same-origin"))
                    .is_ok())
        {
            return Err("unsupported iframe sandbox token");
        }
        Ok(Self {
            app_id,
            display_name,
            entrypoint,
            allowed_origin,
            capabilities,
            allowed_subjects,
            grant_scopes,
            csp,
            sandbox_tokens,
        })
    }
}

#[derive(Clone, Debug)]
pub enum RequestCredential {
    Bearer(Vec<u8>),
    Implicit,
    Invalid,
}

#[derive(Clone, Debug)]
pub struct SessionView {
    pub(crate) session_id: String,
    pub(crate) subject_id: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) issued_at: i64,
    pub(crate) expires_at: i64,
}

#[derive(Clone, Debug)]
pub struct AppGrantView {
    pub(crate) app: AppRegistration,
    pub(crate) scopes: Vec<String>,
    pub(crate) issued_at: i64,
    pub(crate) expires_at: i64,
    pub(crate) launch_credential: Vec<u8>,
}

pub trait PlatformPort: Send + Sync + 'static {
    /// # Errors
    ///
    /// Returns a stable platform failure when credentials or session state are invalid.
    fn current_session(
        &self,
        credential: &RequestCredential,
    ) -> Result<SessionView, PlatformFailure>;
    /// # Errors
    ///
    /// Returns a stable platform failure when credentials or session state are invalid.
    fn refresh_session(
        &self,
        credential: &RequestCredential,
    ) -> Result<SessionView, PlatformFailure>;
    /// # Errors
    ///
    /// Returns a stable platform failure when credentials or session state are invalid.
    fn revoke_session(&self, credential: &RequestCredential) -> Result<i64, PlatformFailure>;
    /// # Errors
    ///
    /// Returns a stable platform failure when credentials or session state are invalid.
    fn registry(
        &self,
        credential: &RequestCredential,
    ) -> Result<Vec<AppRegistration>, PlatformFailure>;
    /// # Errors
    ///
    /// Returns a stable platform failure when credentials, session, or app access are invalid.
    fn authorize_app(
        &self,
        credential: &RequestCredential,
        app_id: &str,
    ) -> Result<AppGrantView, PlatformFailure>;
    /// # Errors
    ///
    /// Returns a stable platform failure when credentials, session, or app access are invalid.
    fn refresh_app(
        &self,
        credential: &RequestCredential,
        app_id: &str,
    ) -> Result<AppGrantView, PlatformFailure>;
    /// # Errors
    ///
    /// Returns a stable platform failure when credentials, session, or app access are invalid.
    fn revoke_app(
        &self,
        credential: &RequestCredential,
        app_id: &str,
    ) -> Result<i64, PlatformFailure>;
}

pub struct PlatformApplication {
    clock: Arc<dyn Clock>,
    policy: SessionPolicy,
    signing_key: hmac::Key,
    identities: Vec<TrustedIdentity>,
    implicit: Option<TrustedIdentity>,
    apps: Vec<AppRegistration>,
    sessions: Mutex<BTreeMap<String, ActiveSession>>,
    grant_sequence: AtomicU64,
}

#[derive(Clone, Debug)]
struct ActiveSession {
    view: SessionView,
    generation: u64,
    revoked_apps: BTreeSet<String>,
}

impl PlatformApplication {
    /// Builds the application-facing registry and session port.
    ///
    /// # Errors
    ///
    /// Returns an error for weak keys or duplicate trusted identities and app IDs.
    pub fn try_new<C: Clock>(
        clock: Arc<C>,
        policy: SessionPolicy,
        signing_key: &[u8],
        identities: Vec<TrustedIdentity>,
        implicit: Option<TrustedIdentity>,
        apps: Vec<AppRegistration>,
    ) -> Result<Self, &'static str> {
        if signing_key.len() < 32 {
            return Err("platform signing key must contain at least 32 bytes");
        }
        if implicit
            .as_ref()
            .is_some_and(|identity| identity.bearer_digest.is_some())
        {
            return Err("implicit identity must not carry a bearer credential");
        }
        let mut subjects = BTreeSet::new();
        let mut digests = BTreeSet::new();
        for identity in identities.iter().chain(implicit.iter()) {
            if !subjects.insert(identity.subject_id.clone()) {
                return Err("trusted subject IDs must be unique");
            }
            if let Some(digest) = identity.bearer_digest
                && !digests.insert(digest)
            {
                return Err("trusted bearer credentials must be unique");
            }
        }
        let mut app_ids = BTreeSet::new();
        if apps.iter().any(|app| !app_ids.insert(app.app_id.clone())) {
            return Err("registered app IDs must be unique");
        }
        Ok(Self {
            clock,
            policy,
            signing_key: hmac::Key::new(hmac::HMAC_SHA256, signing_key),
            identities,
            implicit,
            apps,
            sessions: Mutex::new(BTreeMap::new()),
            grant_sequence: AtomicU64::new(0),
        })
    }

    fn identity(
        &self,
        credential: &RequestCredential,
    ) -> Result<&TrustedIdentity, PlatformFailure> {
        match credential {
            RequestCredential::Bearer(value) => self
                .identities
                .iter()
                .find(|identity| {
                    identity
                        .bearer_digest
                        .is_some_and(|expected| credential_matches(value, &expected))
                })
                .ok_or_else(unauthenticated),
            RequestCredential::Implicit => self.implicit.as_ref().ok_or_else(unauthenticated),
            RequestCredential::Invalid => Err(unauthenticated()),
        }
    }

    fn active_session(&self, identity: &TrustedIdentity) -> Result<SessionView, PlatformFailure> {
        let now = self.clock.now_unix_seconds();
        let sessions = self.sessions.lock().map_err(|_| internal_failure())?;
        let Some(session) = sessions.get(&identity.subject_id) else {
            return Err(unauthenticated());
        };
        if now >= session.view.expires_at {
            return Err(expired());
        }
        Ok(session.view.clone())
    }

    fn issue_session(
        &self,
        identity: &TrustedIdentity,
        generation: u64,
    ) -> Result<ActiveSession, PlatformFailure> {
        let issued_at = self.clock.now_unix_seconds();
        let expires_at = issued_at
            .checked_add(self.policy.session_ttl_seconds)
            .ok_or_else(internal_failure)?;
        let session_id = self.signed_identifier(
            "session/v1",
            &identity.subject_id,
            issued_at,
            expires_at,
            generation,
        );
        Ok(ActiveSession {
            view: SessionView {
                session_id,
                subject_id: identity.subject_id.clone(),
                scopes: identity.scopes.clone(),
                issued_at,
                expires_at,
            },
            generation,
            revoked_apps: BTreeSet::new(),
        })
    }

    fn signed_identifier(
        &self,
        purpose: &str,
        subject_id: &str,
        issued_at: i64,
        expires_at: i64,
        sequence: u64,
    ) -> String {
        let input = format!("{purpose}\0{subject_id}\0{issued_at}\0{expires_at}\0{sequence}");
        let tag = hmac::sign(&self.signing_key, input.as_bytes());
        hex(tag.as_ref())
    }

    fn visible_apps(&self, identity: &TrustedIdentity) -> Vec<AppRegistration> {
        self.apps
            .iter()
            .filter(|app| app.allowed_subjects.contains(&identity.subject_id))
            .cloned()
            .collect()
    }

    fn app_for_subject(
        &self,
        identity: &TrustedIdentity,
        app_id: &str,
    ) -> Result<AppRegistration, PlatformFailure> {
        if app_id.is_empty() {
            return Err(PlatformFailure::new(
                PlatformFailureCode::InvalidRequest,
                false,
                "empty-app-id",
            ));
        }
        let app = self
            .apps
            .iter()
            .find(|app| app.app_id == app_id)
            .ok_or_else(|| {
                PlatformFailure::new(PlatformFailureCode::NotFound, false, "unknown-app")
            })?;
        if !app.allowed_subjects.contains(&identity.subject_id) {
            return Err(PlatformFailure::new(
                PlatformFailureCode::Forbidden,
                false,
                "app-subject-denied",
            ));
        }
        Ok(app.clone())
    }

    fn issue_grant(
        &self,
        identity: &TrustedIdentity,
        session: &SessionView,
        app: AppRegistration,
    ) -> Result<AppGrantView, PlatformFailure> {
        let issued_at = self.clock.now_unix_seconds();
        let policy_expiry = issued_at
            .checked_add(self.policy.app_grant_ttl_seconds)
            .ok_or_else(internal_failure)?;
        let expires_at = policy_expiry.min(session.expires_at);
        if expires_at <= issued_at {
            return Err(expired());
        }
        let sequence = self.grant_sequence.fetch_add(1, Ordering::SeqCst);
        let input = format!(
            "app-grant/v1\0{}\0{}\0{}\0{}\0{}",
            session.session_id, app.app_id, issued_at, expires_at, sequence
        );
        let credential = hmac::sign(&self.signing_key, input.as_bytes());
        let session_scopes: BTreeSet<&str> = identity.scopes.iter().map(String::as_str).collect();
        let scopes = app
            .grant_scopes
            .iter()
            .filter(|scope| session_scopes.contains(scope.as_str()))
            .cloned()
            .collect();
        Ok(AppGrantView {
            app,
            scopes,
            issued_at,
            expires_at,
            launch_credential: credential.as_ref().to_vec(),
        })
    }
}

impl PlatformPort for PlatformApplication {
    fn current_session(
        &self,
        credential: &RequestCredential,
    ) -> Result<SessionView, PlatformFailure> {
        let identity = self.identity(credential)?;
        let now = self.clock.now_unix_seconds();
        let mut sessions = self.sessions.lock().map_err(|_| internal_failure())?;
        if let Some(session) = sessions.get(&identity.subject_id) {
            if now >= session.view.expires_at {
                return Err(expired());
            }
            return Ok(session.view.clone());
        }
        let session = self.issue_session(identity, 0)?;
        let view = session.view.clone();
        sessions.insert(identity.subject_id.clone(), session);
        Ok(view)
    }

    fn refresh_session(
        &self,
        credential: &RequestCredential,
    ) -> Result<SessionView, PlatformFailure> {
        let identity = self.identity(credential)?;
        let now = self.clock.now_unix_seconds();
        let mut sessions = self.sessions.lock().map_err(|_| internal_failure())?;
        let current = sessions
            .get(&identity.subject_id)
            .ok_or_else(unauthenticated)?;
        if now >= current.view.expires_at {
            return Err(expired());
        }
        let generation = current
            .generation
            .checked_add(1)
            .ok_or_else(internal_failure)?;
        let refreshed = self.issue_session(identity, generation)?;
        let view = refreshed.view.clone();
        sessions.insert(identity.subject_id.clone(), refreshed);
        Ok(view)
    }

    fn revoke_session(&self, credential: &RequestCredential) -> Result<i64, PlatformFailure> {
        let identity = self.identity(credential)?;
        let mut sessions = self.sessions.lock().map_err(|_| internal_failure())?;
        sessions
            .remove(&identity.subject_id)
            .ok_or_else(unauthenticated)?;
        Ok(self.clock.now_unix_seconds())
    }

    fn registry(
        &self,
        credential: &RequestCredential,
    ) -> Result<Vec<AppRegistration>, PlatformFailure> {
        let identity = self.identity(credential)?;
        self.active_session(identity)?;
        Ok(self.visible_apps(identity))
    }

    fn authorize_app(
        &self,
        credential: &RequestCredential,
        app_id: &str,
    ) -> Result<AppGrantView, PlatformFailure> {
        let identity = self.identity(credential)?;
        let session = self.active_session(identity)?;
        let app = self.app_for_subject(identity, app_id)?;
        let mut sessions = self.sessions.lock().map_err(|_| internal_failure())?;
        let state = sessions
            .get_mut(&identity.subject_id)
            .ok_or_else(unauthenticated)?;
        state.revoked_apps.remove(app_id);
        drop(sessions);
        self.issue_grant(identity, &session, app)
    }

    fn refresh_app(
        &self,
        credential: &RequestCredential,
        app_id: &str,
    ) -> Result<AppGrantView, PlatformFailure> {
        let identity = self.identity(credential)?;
        let session = self.active_session(identity)?;
        let app = self.app_for_subject(identity, app_id)?;
        let sessions = self.sessions.lock().map_err(|_| internal_failure())?;
        let state = sessions
            .get(&identity.subject_id)
            .ok_or_else(unauthenticated)?;
        if state.revoked_apps.contains(app_id) {
            return Err(expired());
        }
        drop(sessions);
        self.issue_grant(identity, &session, app)
    }

    fn revoke_app(
        &self,
        credential: &RequestCredential,
        app_id: &str,
    ) -> Result<i64, PlatformFailure> {
        let identity = self.identity(credential)?;
        self.active_session(identity)?;
        self.app_for_subject(identity, app_id)?;
        let mut sessions = self.sessions.lock().map_err(|_| internal_failure())?;
        sessions
            .get_mut(&identity.subject_id)
            .ok_or_else(unauthenticated)?
            .revoked_apps
            .insert(app_id.to_owned());
        Ok(self.clock.now_unix_seconds())
    }
}

fn unauthenticated() -> PlatformFailure {
    PlatformFailure::new(
        PlatformFailureCode::Unauthenticated,
        false,
        "credential-rejected",
    )
}

fn expired() -> PlatformFailure {
    PlatformFailure::new(PlatformFailureCode::Expired, false, "session-expired")
}

fn internal_failure() -> PlatformFailure {
    PlatformFailure::new(PlatformFailureCode::Internal, false, "platform-state")
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn valid_app_origin(origin: &str) -> bool {
    let Ok(uri) = origin.parse::<Uri>() else {
        return false;
    };
    let Some(scheme) = uri.scheme_str() else {
        return false;
    };
    let Some(host) = uri.host() else {
        return false;
    };
    let secure = scheme == "https";
    let loopback_http = scheme == "http" && matches!(host, "127.0.0.1" | "localhost" | "::1");
    (secure || loopback_http)
        && uri.path() == "/"
        && uri.query().is_none()
        && !origin.ends_with('/')
}

fn valid_entrypoint(entrypoint: &str) -> bool {
    entrypoint.starts_with('/') && !entrypoint.starts_with("//") && !entrypoint.contains(['?', '#'])
}

fn valid_csp_source(value: &str) -> bool {
    if matches!(value, "'none'" | "'self'") {
        return true;
    }
    if value.is_empty()
        || value.bytes().any(|byte| byte.is_ascii_whitespace())
        || value.contains([';', '*', '@'])
    {
        return false;
    }
    valid_app_origin(value)
}
