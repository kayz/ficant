use std::collections::BTreeSet;

use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};

use super::{AccessScope, ApplicationResult, OperationFingerprint};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_domain::DomainErrorCode;

const TOKEN_VERSION: &str = "FCUR2";
const NONCE_LENGTH: usize = 12;
const TAG_LENGTH: usize = 16;

pub struct CursorKey {
    key_id: String,
    key: LessSafeKey,
}

impl CursorKey {
    /// Creates one configured AES-256-GCM cursor key.
    ///
    /// # Errors
    ///
    /// Returns validation failure unless the key ID is a compact ASCII token.
    pub fn new(key_id: impl Into<String>, mut key_material: [u8; 32]) -> ApplicationResult<Self> {
        let key_id = key_id.into();
        if key_id.is_empty()
            || key_id.len() > 64
            || !key_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            key_material.fill(0);
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        let key = UnboundKey::new(&aead::AES_256_GCM, &key_material);
        key_material.fill(0);
        let key = key
            .map(LessSafeKey::new)
            .map_err(|_| map_domain_error(DomainErrorCode::InvalidValue))?;
        Ok(Self { key_id, key })
    }

    #[must_use]
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
}

pub struct AeadCursorCodec {
    active: CursorKey,
    retired: Vec<CursorKey>,
    random: SystemRandom,
}

impl AeadCursorCodec {
    /// Configures the active encryption key and accepted retired decryption keys.
    ///
    /// # Errors
    ///
    /// Returns validation failure when a key ID appears more than once.
    pub fn new(active: CursorKey, retired: Vec<CursorKey>) -> ApplicationResult<Self> {
        let mut key_ids = BTreeSet::new();
        if !key_ids.insert(active.key_id.clone())
            || retired
                .iter()
                .any(|candidate| !key_ids.insert(candidate.key_id.clone()))
        {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        Ok(Self {
            active,
            retired,
            random: SystemRandom::new(),
        })
    }

    #[must_use]
    pub fn active_key_id(&self) -> &str {
        self.active.key_id()
    }

    fn seal(&self, scope: &AccessScope, plaintext: &[u8]) -> ApplicationResult<String> {
        let mut nonce_bytes = [0_u8; NONCE_LENGTH];
        self.random.fill(&mut nonce_bytes).map_err(|_| {
            ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, true)
        })?;
        let mut ciphertext = plaintext.to_vec();
        self.active
            .key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(aad(scope, self.active.key_id()).as_slice()),
                &mut ciphertext,
            )
            .map_err(|_| {
                ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, true)
            })?;
        Ok(format!(
            "{TOKEN_VERSION}.{}.{}.{}",
            self.active.key_id(),
            encode_hex(&nonce_bytes),
            encode_hex(&ciphertext)
        ))
    }

    fn open(&self, scope: &AccessScope, token: &str) -> ApplicationResult<Vec<u8>> {
        let mut fields = token.split('.');
        let (Some(version), Some(key_id), Some(nonce), Some(ciphertext)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(forbidden());
        };
        if fields.next().is_some() || version != TOKEN_VERSION {
            return Err(forbidden());
        }
        let key = self.find_key(key_id).ok_or_else(forbidden)?;
        let nonce_bytes = decode_hex(nonce).ok_or_else(forbidden)?;
        let nonce_bytes: [u8; NONCE_LENGTH] = nonce_bytes.try_into().map_err(|_| forbidden())?;
        let mut ciphertext = decode_hex(ciphertext).ok_or_else(forbidden)?;
        if ciphertext.len() <= TAG_LENGTH {
            return Err(forbidden());
        }
        let plaintext = key
            .key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(aad(scope, key_id).as_slice()),
                &mut ciphertext,
            )
            .map_err(|_| forbidden())?;
        Ok(plaintext.to_vec())
    }

    fn find_key(&self, key_id: &str) -> Option<&CursorKey> {
        if self.active.key_id() == key_id {
            return Some(&self.active);
        }
        self.retired
            .iter()
            .find(|candidate| candidate.key_id() == key_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    token: String,
    opaque_value: String,
    scope_fingerprint: OperationFingerprint,
}

impl PartialOrd for Cursor {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Cursor {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.token.cmp(&other.token)
    }
}

impl std::hash::Hash for Cursor {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&self.token, state);
    }
}

impl Cursor {
    /// Issues an authenticated encrypted cursor bound to its authorization scope.
    ///
    /// # Errors
    ///
    /// Returns validation failure for a blank/padded storage cursor or storage-unavailable when
    /// secure nonce generation or encryption fails.
    pub fn issue(
        codec: &AeadCursorCodec,
        scope: &AccessScope,
        opaque_value: impl Into<String>,
    ) -> ApplicationResult<Self> {
        let opaque_value = validated_plaintext(opaque_value.into())?;
        let token = codec.seal(scope, opaque_value.as_bytes())?;
        Ok(Self {
            token,
            opaque_value,
            scope_fingerprint: scope.fingerprint().clone(),
        })
    }

    /// Restores and authenticates an encrypted cursor under the supplied authorization scope.
    ///
    /// # Errors
    ///
    /// Returns one stable non-retryable forbidden error for malformed, modified, wrong-scope,
    /// unknown-version, unknown-key, invalid-tag, or invalid-plaintext tokens.
    pub fn resume(
        codec: &AeadCursorCodec,
        scope: &AccessScope,
        token: impl Into<String>,
    ) -> ApplicationResult<Self> {
        let token = token.into();
        let plaintext = codec.open(scope, &token)?;
        let opaque_value = String::from_utf8(plaintext).map_err(|_| forbidden())?;
        if opaque_value.trim().is_empty() || opaque_value != opaque_value.trim() {
            return Err(forbidden());
        }
        Ok(Self {
            token,
            opaque_value,
            scope_fingerprint: scope.fingerprint().clone(),
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.token
    }

    #[must_use]
    pub fn opaque_value(&self) -> &str {
        &self.opaque_value
    }

    #[must_use]
    pub fn scope_fingerprint(&self) -> &OperationFingerprint {
        &self.scope_fingerprint
    }

    /// Verifies that this cursor was issued or resumed under the supplied scope.
    ///
    /// # Errors
    ///
    /// Returns forbidden when tenant, actor, or canonical allowed-owner permissions differ.
    pub fn authorize_scope(&self, scope: &AccessScope) -> ApplicationResult<()> {
        if self.scope_fingerprint != *scope.fingerprint() {
            return Err(forbidden());
        }
        Ok(())
    }
}

fn validated_plaintext(value: String) -> ApplicationResult<String> {
    if value.trim().is_empty() || value != value.trim() {
        return Err(map_domain_error(DomainErrorCode::InvalidValue));
    }
    Ok(value)
}

fn aad(scope: &AccessScope, key_id: &str) -> Vec<u8> {
    let mut value = Vec::with_capacity(TOKEN_VERSION.len() + key_id.len() + 34);
    value.extend_from_slice(TOKEN_VERSION.as_bytes());
    value.push(0);
    value.extend_from_slice(key_id.as_bytes());
    value.push(0);
    value.extend_from_slice(scope.fingerprint().content_hash().as_bytes());
    value
}

fn encode_hex(value: &[u8]) -> String {
    value.iter().fold(
        String::with_capacity(value.len() * 2),
        |mut encoded, byte| {
            use std::fmt::Write as _;
            write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
            encoded
        },
    )
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = decode_nibble(pair[0])?;
            let low = decode_nibble(pair[1])?;
            Some((high << 4) | low)
        })
        .collect()
}

const fn decode_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}
