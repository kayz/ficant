//! Dual-protocol bearer credential extraction and constant-time SHA-256 digest verification.
//!
//! Supports both native HTTP/2 gRPC and HTTP/1.1 `gRPC-Web` requests via `tonic::metadata::MetadataMap`.
//! Accepts a unique `authorization: Bearer <token>` header (primary contract) or a unique
//! `x-ficant-credential` header, rejecting duplicate or ambiguous headers with `UNAUTHENTICATED`.

use std::collections::{BTreeSet, HashSet};
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use ficant_application::ports::{
    ApplicationResult, CredentialVerifier, PrincipalRole, VerifiedPrincipal,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::primitives::MarketTime;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tonic::metadata::MetadataMap;

pub const AUTHORIZATION_HEADER: &str = "authorization";
pub const FICANT_CREDENTIAL_HEADER: &str = "x-ficant-credential";

/// Extracts a single, unambiguous credential token from gRPC / gRPC-Web request metadata.
///
/// Rules:
/// - Rejects requests carrying both `authorization` and `x-ficant-credential`.
/// - Rejects requests carrying multiple values for either header.
/// - `authorization` MUST start with exact prefix `"Bearer "` followed by a non-empty token
///   with no whitespace.
/// - `x-ficant-credential` accepts either `"Bearer <token>"` or a raw non-empty token with
///   no whitespace.
pub fn extract_bearer_token(metadata: &MetadataMap) -> ApplicationResult<String> {
    let auth_values: Vec<_> = metadata.get_all(AUTHORIZATION_HEADER).iter().collect();
    let custom_values: Vec<_> = metadata.get_all(FICANT_CREDENTIAL_HEADER).iter().collect();

    if !auth_values.is_empty() && !custom_values.is_empty() {
        return Err(unauthenticated());
    }
    if auth_values.len() > 1 || custom_values.len() > 1 {
        return Err(unauthenticated());
    }

    if let Some(raw) = auth_values.first() {
        let text = raw.to_str().map_err(|_| unauthenticated())?;
        let token = text.strip_prefix("Bearer ").ok_or_else(unauthenticated)?;
        return validate_raw_token(token);
    }

    if let Some(raw) = custom_values.first() {
        let text = raw.to_str().map_err(|_| unauthenticated())?;
        let token = text.strip_prefix("Bearer ").unwrap_or(text);
        return validate_raw_token(token);
    }

    Err(unauthenticated())
}

fn validate_raw_token(token: &str) -> ApplicationResult<String> {
    if token.is_empty()
        || token
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        return Err(unauthenticated());
    }
    Ok(token.to_owned())
}

/// Computes the fixed 32-byte SHA-256 digest of a raw credential token.
#[must_use]
pub fn sha256_token_digest(raw_token: &str) -> [u8; 32] {
    let digest = Sha256::digest(raw_token.as_bytes());
    let mut out = [0_u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Formats a 32-byte digest as lowercase 64-character hex.
#[must_use]
pub fn encode_digest_hex(digest: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for &b in digest {
        out.push(HEX[usize::from(b >> 4)] as char);
        out.push(HEX[usize::from(b & 0x0f)] as char);
    }
    out
}

/// Parses a 64-character hex string into a fixed 32-byte digest.
pub fn decode_digest_hex(hex: &str) -> Result<[u8; 32], String> {
    let trimmed = hex.trim();
    if trimmed.len() != 64 || trimmed != hex {
        return Err("credential digest must be exactly 64 hex characters".to_owned());
    }
    let bytes = trimmed.as_bytes();
    let mut out = [0_u8; 32];
    for (idx, pair) in bytes.chunks_exact(2).enumerate() {
        let hi = hex_nibble(pair[0])?;
        let lo = hex_nibble(pair[1])?;
        out[idx] = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err("invalid hex character in credential digest".to_owned()),
    }
}

/// Constant-time 32-byte equality comparison (no early exit, zero `unsafe`).
#[must_use]
pub fn constant_time_eq_32(lhs: &[u8; 32], rhs: &[u8; 32]) -> bool {
    let mut diff = 0_u8;
    for i in 0..32 {
        diff |= lhs[i] ^ rhs[i];
    }
    diff == 0
}

/// Serializable JSON schema for a server startup credential configuration file.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialConfigFile {
    pub schema_version: u32,
    pub credentials: Vec<CredentialEntryConfig>,
}

/// Serializable JSON schema for one credential digest entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialEntryConfig {
    pub actor_id: String,
    pub role: String,
    pub token_sha256_hex: String,
    #[serde(default)]
    pub allowed_accounts: Vec<String>,
    #[serde(default)]
    pub allowed_sources: Vec<String>,
    #[serde(default)]
    pub expires_at_rfc3339: Option<String>,
    #[serde(default)]
    pub revoked: bool,
}

/// Validated in-memory credential record holding ONLY the fixed 32-byte digest (never plaintext).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialEntry {
    pub actor_id: String,
    pub role: PrincipalRole,
    pub token_digest: [u8; 32],
    pub allowed_accounts: BTreeSet<String>,
    pub allowed_sources: BTreeSet<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked: bool,
}

impl CredentialEntry {
    /// Builds a credential entry from a plaintext token by immediately hashing it to SHA-256.
    #[must_use]
    pub fn from_plaintext_token(
        actor_id: impl Into<String>,
        role: PrincipalRole,
        raw_token: &str,
        allowed_accounts: impl IntoIterator<Item = String>,
        allowed_sources: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            actor_id: actor_id.into(),
            role,
            token_digest: sha256_token_digest(raw_token),
            allowed_accounts: allowed_accounts.into_iter().collect(),
            allowed_sources: allowed_sources.into_iter().collect(),
            expires_at: None,
            revoked: false,
        }
    }

    #[must_use]
    pub const fn with_expires_at(mut self, expires_at: DateTime<Utc>) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    #[must_use]
    pub const fn with_revoked(mut self, revoked: bool) -> Self {
        self.revoked = revoked;
        self
    }
}

/// Read-only startup-configured credential digest verifier with constant-time comparison and
/// optional runtime revocation (for mid-stream revocation enforcement).
pub struct StaticCredentialVerifier {
    entries: RwLock<Vec<CredentialEntry>>,
}

impl std::fmt::Debug for StaticCredentialVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticCredentialVerifier")
            .finish_non_exhaustive()
    }
}

impl StaticCredentialVerifier {
    /// Validates and constructs a credential verifier from in-memory entries.
    ///
    /// Fails closed on empty entries, duplicate digests, pre-revoked entries, empty/untrimmed
    /// actor IDs, or empty/untrimmed account/source scope entries.
    pub fn new(entries: Vec<CredentialEntry>) -> Result<Self, String> {
        if entries.is_empty() {
            return Err("credential table must contain at least one entry".to_owned());
        }
        let mut seen_digests: HashSet<[u8; 32]> = HashSet::with_capacity(entries.len());
        for entry in &entries {
            validate_non_empty_id(&entry.actor_id, "actor_id")?;
            if entry.revoked {
                return Err(format!(
                    "startup credential entry for actor '{}' is already revoked",
                    entry.actor_id
                ));
            }
            for acct in &entry.allowed_accounts {
                validate_non_empty_id(acct, "allowed_accounts[]")?;
            }
            for src in &entry.allowed_sources {
                validate_non_empty_id(src, "allowed_sources[]")?;
            }
            if !seen_digests.insert(entry.token_digest) {
                return Err("duplicate credential digest mapping detected".to_owned());
            }
        }
        Ok(Self {
            entries: RwLock::new(entries),
        })
    }

    /// Validates and constructs a credential verifier against a startup clock timestamp,
    /// rejecting pre-revoked and already-expired (`expires_at <= now`) entries.
    pub fn new_for_startup(
        entries: Vec<CredentialEntry>,
        now: &MarketTime,
    ) -> Result<Self, String> {
        let verifier = Self::new(entries)?;
        verifier.validate_startup(now)?;
        Ok(verifier)
    }

    /// Validates that no configured credential entry is pre-revoked or already expired
    /// (`expires_at <= now`) when enabling Desk Blotter storage at startup.
    pub fn validate_startup(&self, now: &MarketTime) -> Result<(), String> {
        let guard = self
            .entries
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.is_empty() {
            return Err("credential table must contain at least one entry".to_owned());
        }
        for entry in guard.iter() {
            if entry.revoked {
                return Err(format!(
                    "startup credential entry for actor '{}' is already revoked",
                    entry.actor_id
                ));
            }
            if let Some(expires_at) = entry.expires_at
                && expires_at <= now.instant()
            {
                return Err(format!(
                    "startup credential entry for actor '{}' is already expired at startup",
                    entry.actor_id
                ));
            }
        }
        Ok(())
    }

    /// Parses and validates a startup credential JSON configuration file.
    pub fn from_json_str(json_text: &str) -> Result<Self, String> {
        let parsed: CredentialConfigFile = serde_json::from_str(json_text)
            .map_err(|err| format!("invalid credentials json: {err}"))?;
        if parsed.schema_version != 1 {
            return Err(format!(
                "unsupported credentials schema_version {}, expected 1",
                parsed.schema_version
            ));
        }

        let mut entries = Vec::with_capacity(parsed.credentials.len());
        for raw in parsed.credentials {
            let role = parse_role(&raw.role)?;
            let token_digest = decode_digest_hex(&raw.token_sha256_hex)?;
            let expires_at = raw
                .expires_at_rfc3339
                .as_deref()
                .map(|ts| {
                    DateTime::parse_from_rfc3339(ts)
                        .map(|dt| dt.with_timezone(&Utc))
                        .map_err(|err| format!("invalid expires_at_rfc3339: {err}"))
                })
                .transpose()?;
            let mut allowed_accounts = BTreeSet::new();
            for acct in raw.allowed_accounts {
                validate_non_empty_id(&acct, "allowed_accounts[]")?;
                if !allowed_accounts.insert(acct) {
                    return Err("duplicate account in allowed_accounts".to_owned());
                }
            }
            let mut allowed_sources = BTreeSet::new();
            for src in raw.allowed_sources {
                validate_non_empty_id(&src, "allowed_sources[]")?;
                if !allowed_sources.insert(src) {
                    return Err("duplicate source in allowed_sources".to_owned());
                }
            }
            entries.push(CredentialEntry {
                actor_id: raw.actor_id,
                role,
                token_digest,
                allowed_accounts,
                allowed_sources,
                expires_at,
                revoked: raw.revoked,
            });
        }

        Self::new(entries)
    }

    /// Parses and validates a startup credential JSON configuration file against `now`,
    /// rejecting pre-revoked and already-expired (`expires_at <= now`) entries.
    pub fn from_json_str_at(json_text: &str, now: &MarketTime) -> Result<Self, String> {
        let verifier = Self::from_json_str(json_text)?;
        verifier.validate_startup(now)?;
        Ok(verifier)
    }

    /// Revokes all credentials belonging to `actor_id` immediately (used to verify mid-stream
    /// subscription termination upon revocation).
    pub fn revoke_actor(&self, actor_id: &str) {
        let mut guard = self
            .entries
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in guard.iter_mut() {
            if entry.actor_id == actor_id {
                entry.revoked = true;
            }
        }
    }

    /// Revokes a specific credential by its 32-byte SHA-256 digest.
    pub fn revoke_digest(&self, digest: &[u8; 32]) {
        let mut guard = self
            .entries
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in guard.iter_mut() {
            if constant_time_eq_32(&entry.token_digest, digest) {
                entry.revoked = true;
            }
        }
    }
}

impl CredentialVerifier for StaticCredentialVerifier {
    fn verify_bearer(
        &self,
        bearer_token: &str,
        now: &MarketTime,
    ) -> ApplicationResult<VerifiedPrincipal> {
        let _ = validate_raw_token(bearer_token)?;
        let candidate_digest = sha256_token_digest(bearer_token);

        let guard = self
            .entries
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Full linear scan using constant-time 32-byte digest comparison (no early exit).
        let mut matched_idx: Option<usize> = None;
        for (idx, entry) in guard.iter().enumerate() {
            if constant_time_eq_32(&entry.token_digest, &candidate_digest) {
                matched_idx = Some(idx);
            }
        }

        let entry = matched_idx
            .and_then(|idx| guard.get(idx))
            .ok_or_else(unauthenticated)?;

        if entry.revoked {
            return Err(unauthenticated());
        }
        if let Some(expires_at) = entry.expires_at
            && now.instant() >= expires_at
        {
            return Err(unauthenticated());
        }

        Ok(VerifiedPrincipal {
            actor_id: entry.actor_id.clone(),
            role: entry.role,
            allowed_accounts: entry.allowed_accounts.clone(),
            allowed_sources: entry.allowed_sources.clone(),
        })
    }
}

fn parse_role(raw: &str) -> Result<PrincipalRole, String> {
    match raw {
        "TRADER" => Ok(PrincipalRole::Trader),
        "NLP_PARSER" => Ok(PrincipalRole::NlpParser),
        "FEED_ADAPTER" => Ok(PrincipalRole::FeedAdapter),
        "OPERATOR" => Ok(PrincipalRole::Operator),
        other => Err(format!("unknown principal role '{other}'")),
    }
}

fn validate_non_empty_id(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty() || value != value.trim() {
        return Err(format!(
            "{field} must be non-empty without leading/trailing whitespace"
        ));
    }
    Ok(())
}

const fn unauthenticated() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Unauthenticated, false)
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, TimeZone, Utc};
    use ficant_application::ApplicationErrorCategory;
    use ficant_application::ports::{CredentialVerifier, PrincipalRole};
    use ficant_domain::analytics::MARKET_TIMEZONE;
    use ficant_domain::primitives::MarketTime;
    use tonic::metadata::{MetadataMap, MetadataValue};

    use super::{
        AUTHORIZATION_HEADER, CredentialEntry, FICANT_CREDENTIAL_HEADER, StaticCredentialVerifier,
        constant_time_eq_32, encode_digest_hex, extract_bearer_token, sha256_token_digest,
    };

    fn now_time() -> MarketTime {
        let instant = Utc.with_ymd_and_hms(2026, 6, 15, 2, 0, 0).unwrap();
        MarketTime::new(
            instant,
            MARKET_TIMEZONE,
            NaiveDate::from_ymd_opt(2026, 6, 15).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn extracts_authorization_bearer_and_x_ficant_credential_uniquely() {
        let mut m1 = MetadataMap::new();
        m1.insert(
            AUTHORIZATION_HEADER,
            MetadataValue::try_from("Bearer secret-tok-1").unwrap(),
        );
        assert_eq!(extract_bearer_token(&m1).unwrap(), "secret-tok-1");

        let mut m2 = MetadataMap::new();
        m2.insert(
            FICANT_CREDENTIAL_HEADER,
            MetadataValue::try_from("secret-tok-2").unwrap(),
        );
        assert_eq!(extract_bearer_token(&m2).unwrap(), "secret-tok-2");

        let mut m3 = MetadataMap::new();
        m3.insert(
            FICANT_CREDENTIAL_HEADER,
            MetadataValue::try_from("Bearer secret-tok-3").unwrap(),
        );
        assert_eq!(extract_bearer_token(&m3).unwrap(), "secret-tok-3");

        // Missing header -> Unauthenticated
        assert_eq!(
            extract_bearer_token(&MetadataMap::new())
                .unwrap_err()
                .category(),
            ApplicationErrorCategory::Unauthenticated
        );

        // Both headers present -> Unauthenticated
        let mut both = MetadataMap::new();
        both.insert(
            AUTHORIZATION_HEADER,
            MetadataValue::try_from("Bearer tok-a").unwrap(),
        );
        both.insert(
            FICANT_CREDENTIAL_HEADER,
            MetadataValue::try_from("tok-a").unwrap(),
        );
        assert_eq!(
            extract_bearer_token(&both).unwrap_err().category(),
            ApplicationErrorCategory::Unauthenticated
        );

        // Duplicate authorization header -> Unauthenticated
        let mut dup = MetadataMap::new();
        dup.append(
            AUTHORIZATION_HEADER,
            MetadataValue::try_from("Bearer tok-a").unwrap(),
        );
        dup.append(
            AUTHORIZATION_HEADER,
            MetadataValue::try_from("Bearer tok-b").unwrap(),
        );
        assert_eq!(
            extract_bearer_token(&dup).unwrap_err().category(),
            ApplicationErrorCategory::Unauthenticated
        );

        // Missing Bearer prefix on authorization -> Unauthenticated
        let mut bad_prefix = MetadataMap::new();
        bad_prefix.insert(
            AUTHORIZATION_HEADER,
            MetadataValue::try_from("Basic abcdef").unwrap(),
        );
        assert_eq!(
            extract_bearer_token(&bad_prefix).unwrap_err().category(),
            ApplicationErrorCategory::Unauthenticated
        );
    }

    #[test]
    fn rejects_duplicate_digests_revoked_and_expired_credentials() {
        let tok = "high-entropy-trader-token-001";
        let e1 = CredentialEntry::from_plaintext_token(
            "trader-1",
            PrincipalRole::Trader,
            tok,
            ["acct-a".to_owned()],
            ["src-1".to_owned()],
        );
        let e2 = CredentialEntry::from_plaintext_token(
            "trader-2",
            PrincipalRole::Trader,
            tok,
            ["acct-a".to_owned()],
            ["src-1".to_owned()],
        );
        assert!(StaticCredentialVerifier::new(vec![e1.clone(), e2]).is_err());

        let expired = CredentialEntry::from_plaintext_token(
            "expired-trader",
            PrincipalRole::Trader,
            "expired-tok",
            ["acct-a".to_owned()],
            ["src-1".to_owned()],
        )
        .with_expires_at(Utc.with_ymd_and_hms(2026, 6, 15, 2, 0, 0).unwrap());

        // Pre-revoked entry rejected at startup construction and JSON load.
        let pre_revoked = e1.clone().with_revoked(true);
        assert!(StaticCredentialVerifier::new(vec![pre_revoked]).is_err());
        let revoked_json = format!(
            r#"{{"schema_version":1,"credentials":[{{"actor_id":"rev-1","role":"TRADER","token_sha256_hex":"{}","revoked":true}}]}}"#,
            encode_digest_hex(&sha256_token_digest("rev-tok"))
        );
        assert!(StaticCredentialVerifier::from_json_str(&revoked_json).is_err());

        let before_expiry = MarketTime::new(
            Utc.with_ymd_and_hms(2026, 6, 15, 1, 59, 59).unwrap(),
            MARKET_TIMEZONE,
            NaiveDate::from_ymd_opt(2026, 6, 15).unwrap(),
        )
        .unwrap();
        let now = now_time();

        // Already-expired entry (expires_at <= now) rejected at startup validation, while
        // future expires_at (> startup time) is accepted and expires cleanly at runtime.
        assert!(
            StaticCredentialVerifier::new_for_startup(vec![e1.clone(), expired.clone()], &now)
                .is_err()
        );
        let expired_json = format!(
            r#"{{"schema_version":1,"credentials":[{{"actor_id":"exp-1","role":"TRADER","token_sha256_hex":"{}","expires_at_rfc3339":"2026-06-15T02:00:00Z"}}]}}"#,
            encode_digest_hex(&sha256_token_digest("expired-tok"))
        );
        assert!(StaticCredentialVerifier::from_json_str_at(&expired_json, &now).is_err());
        assert!(StaticCredentialVerifier::from_json_str_at(&expired_json, &before_expiry).is_ok());

        let verifier =
            StaticCredentialVerifier::new_for_startup(vec![e1, expired], &before_expiry).unwrap();
        assert!(verifier.validate_startup(&now).is_err());

        // Before expiry -> valid
        assert_eq!(
            verifier
                .verify_bearer("expired-tok", &before_expiry)
                .unwrap()
                .actor_id,
            "expired-trader"
        );
        let principal = verifier.verify_bearer(tok, &now).unwrap();
        assert_eq!(principal.actor_id, "trader-1");
        assert_eq!(principal.role, PrincipalRole::Trader);

        // Runtime clock reaches exact expiry boundary -> Unauthenticated
        assert_eq!(
            verifier
                .verify_bearer("expired-tok", &now)
                .unwrap_err()
                .category(),
            ApplicationErrorCategory::Unauthenticated
        );

        // Runtime revocation takes effect immediately
        verifier.revoke_actor("trader-1");
        assert_eq!(
            verifier.verify_bearer(tok, &now).unwrap_err().category(),
            ApplicationErrorCategory::Unauthenticated
        );
        assert!(verifier.validate_startup(&before_expiry).is_err());

        let d1 = sha256_token_digest("abc");
        let d2 = sha256_token_digest("abd");
        assert!(constant_time_eq_32(&d1, &d1));
        assert!(!constant_time_eq_32(&d1, &d2));
        assert_eq!(encode_digest_hex(&d1).len(), 64);
    }
}
