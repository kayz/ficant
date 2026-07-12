use ring::hmac;
use std::time::{SystemTime, UNIX_EPOCH};

pub trait Clock: Send + Sync + 'static {
    fn now_unix_seconds(&self) -> i64;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_seconds(&self) -> i64 {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        i64::try_from(seconds).unwrap_or(i64::MAX)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionPolicy {
    pub(crate) session_ttl_seconds: i64,
    pub(crate) app_grant_ttl_seconds: i64,
}

impl SessionPolicy {
    /// Builds session and short-lived app grant lifetimes.
    ///
    /// # Errors
    ///
    /// Returns an error for non-positive durations or a grant lifetime that is not shorter.
    pub fn new(session_ttl_seconds: i64, app_grant_ttl_seconds: i64) -> Result<Self, &'static str> {
        if session_ttl_seconds <= 0 || app_grant_ttl_seconds <= 0 {
            return Err("session and app-grant TTLs must be positive");
        }
        if app_grant_ttl_seconds >= session_ttl_seconds {
            return Err("app-grant TTL must be shorter than session TTL");
        }
        Ok(Self {
            session_ttl_seconds,
            app_grant_ttl_seconds,
        })
    }
}

#[derive(Clone, Debug)]
pub struct TrustedIdentity {
    pub(crate) subject_id: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) bearer_digest: Option<[u8; 32]>,
}

impl TrustedIdentity {
    /// Builds a trusted identity authenticated by a primary bearer credential.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty credential, invalid subject, or invalid scope.
    pub fn bearer<I, S>(
        subject_id: impl Into<String>,
        bearer_credential: &[u8],
        scopes: I,
    ) -> Result<Self, &'static str>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        if bearer_credential.is_empty() {
            return Err("bearer credential must not be empty");
        }
        Self::build(
            subject_id,
            scopes,
            Some(credential_proof(bearer_credential)),
        )
    }

    /// Builds an identity available only through an explicitly loopback-bound server.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid subject or scope.
    pub fn implicit<I, S>(subject_id: impl Into<String>, scopes: I) -> Result<Self, &'static str>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::build(subject_id, scopes, None)
    }

    fn build<I, S>(
        subject_id: impl Into<String>,
        scopes: I,
        bearer_digest: Option<[u8; 32]>,
    ) -> Result<Self, &'static str>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let subject_id = subject_id.into();
        if subject_id.trim().is_empty() || subject_id.len() > 128 {
            return Err("subject ID must be 1..=128 non-blank bytes");
        }
        let mut scopes: Vec<String> = scopes.into_iter().map(Into::into).collect();
        if scopes.iter().any(|scope| !valid_token(scope)) {
            return Err("scope must be a compact ASCII token");
        }
        scopes.sort();
        scopes.dedup();
        Ok(Self {
            subject_id,
            scopes,
            bearer_digest,
        })
    }
}

pub(crate) fn credential_proof(credential: &[u8]) -> [u8; 32] {
    let key = hmac::Key::new(hmac::HMAC_SHA256, credential);
    let value = hmac::sign(&key, b"ficant-platform-primary-credential/v1");
    let mut output = [0_u8; 32];
    output.copy_from_slice(value.as_ref());
    output
}

pub(crate) fn credential_matches(credential: &[u8], expected: &[u8; 32]) -> bool {
    let key = hmac::Key::new(hmac::HMAC_SHA256, credential);
    hmac::verify(&key, b"ficant-platform-primary-credential/v1", expected).is_ok()
}

fn valid_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.'))
}
