use ficant_application::ports::AuthorizedPrincipal;
use ficant_domain::governance::PlatformRole;
use ficant_domain::primitives::{ContentHash, Ulid};
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
    pub(crate) principal: AuthorizedPrincipal,
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
        actor_id: Ulid,
        tenant_id: Ulid,
        allowed_owner_ids: Vec<Ulid>,
        active_role: PlatformRole,
        scopes: I,
    ) -> Result<Self, &'static str>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        if bearer_credential.is_empty() {
            return Err("bearer credential must not be empty");
        }
        let proof = credential_proof(bearer_credential);
        Self::build(
            subject_id,
            actor_id,
            tenant_id,
            allowed_owner_ids,
            active_role,
            scopes,
            ContentHash::from_bytes(&proof)
                .map_err(|_| "credential fingerprint must be SHA-256")?,
            Some(proof),
        )
    }

    /// Builds a trusted bearer identity from one already validated principal.
    ///
    /// # Errors
    ///
    /// Returns an error when the bearer credential is empty or its fingerprint does not match
    /// the principal credential binding.
    pub fn bearer_principal(
        principal: AuthorizedPrincipal,
        bearer_credential: &[u8],
    ) -> Result<Self, &'static str> {
        if bearer_credential.is_empty() {
            return Err("bearer credential must not be empty");
        }
        let proof = credential_proof(bearer_credential);
        if principal.credential_fingerprint().as_bytes() != &proof {
            return Err("principal credential fingerprint must match bearer credential");
        }
        Ok(Self {
            principal,
            bearer_digest: Some(proof),
        })
    }

    /// Builds a loopback-only trusted identity from one already validated principal.
    #[must_use]
    pub const fn implicit_principal(principal: AuthorizedPrincipal) -> Self {
        Self {
            principal,
            bearer_digest: None,
        }
    }

    /// Builds an identity available only through an explicitly loopback-bound server.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid subject or scope.
    pub fn implicit<I, S>(
        subject_id: impl Into<String>,
        actor_id: Ulid,
        tenant_id: Ulid,
        allowed_owner_ids: Vec<Ulid>,
        active_role: PlatformRole,
        scopes: I,
    ) -> Result<Self, &'static str>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let subject_id = subject_id.into();
        let fingerprint = implicit_credential_fingerprint(
            &subject_id,
            &actor_id,
            &tenant_id,
            &allowed_owner_ids,
            active_role,
        );
        Self::build(
            subject_id,
            actor_id,
            tenant_id,
            allowed_owner_ids,
            active_role,
            scopes,
            fingerprint,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build<I, S>(
        subject_id: impl Into<String>,
        actor_id: Ulid,
        tenant_id: Ulid,
        allowed_owner_ids: Vec<Ulid>,
        active_role: PlatformRole,
        scopes: I,
        credential_fingerprint: ContentHash,
        bearer_digest: Option<[u8; 32]>,
    ) -> Result<Self, &'static str>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let scopes = scopes.into_iter().map(Into::into).collect();
        let principal = AuthorizedPrincipal::new(
            subject_id.into(),
            actor_id,
            tenant_id,
            allowed_owner_ids,
            active_role,
            scopes,
            credential_fingerprint,
        )
        .map_err(|_| "trusted principal fields must be valid")?;
        Ok(Self {
            principal,
            bearer_digest,
        })
    }
}

fn implicit_credential_fingerprint(
    subject_id: &str,
    actor_id: &Ulid,
    tenant_id: &Ulid,
    allowed_owner_ids: &[Ulid],
    active_role: PlatformRole,
) -> ContentHash {
    let mut owners = allowed_owner_ids.to_vec();
    owners.sort();
    owners.dedup();
    let mut bytes = b"ficant-platform-implicit-credential/v1\0".to_vec();
    append_fingerprint_field(&mut bytes, subject_id.as_bytes());
    append_fingerprint_field(&mut bytes, actor_id.as_str().as_bytes());
    append_fingerprint_field(&mut bytes, tenant_id.as_str().as_bytes());
    for owner in owners {
        append_fingerprint_field(&mut bytes, owner.as_str().as_bytes());
    }
    append_fingerprint_field(
        &mut bytes,
        &[match active_role {
            PlatformRole::PlatformAdmin => 1,
            PlatformRole::Researcher => 2,
        }],
    );
    ContentHash::digest(&bytes)
}

fn append_fingerprint_field(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(
        &u64::try_from(value.len())
            .expect("trusted identity field length fits u64")
            .to_be_bytes(),
    );
    bytes.extend_from_slice(value);
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
