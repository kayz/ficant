//! External desk-blotter policy loader (embedded Policy B JSON + runtime override).
//!
//! Product allowlists, tolerances, and regulatory rule-pack bindings live strictly in external
//! JSON configuration rather than hardcoded in Rust source.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use chrono::{DateTime, NaiveTime, Utc};
use chrono_tz::Asia::Shanghai;
use ficant_api::decode_digest_hex;
use ficant_application::ports::{ApplicationResult, DeskBlotterPolicyProvider};
use ficant_application::use_cases::blotter::AccountDeskPolicy;
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_blotter_store::parse_fixed_decimal;
use ficant_domain::analytics::MARKET_TIMEZONE;
use ficant_domain::blotter::BlotterConfig;
use ficant_domain::market::{
    MarketRulePack, MarketRulePackInput, RulePackContent, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, FixedDecimal, MarketTime, OwnerRef, Ulid, Version,
};
use serde::{Deserialize, Serialize};

/// Embedded Policy B default configuration loaded strictly from external JSON.
pub const EMBEDDED_DEFAULT_POLICY_JSON: &str =
    include_str!("../policy/desk-blotter-policy-v1.json");

const DEFAULT_OWNER_ULID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

/// Serializable JSON schema for the top-level desk-blotter policy file.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeskBlotterPolicyFile {
    pub schema_version: u32,
    pub policy_version: u64,
    pub policy_profile: String,
    pub allowed_hedge_product_codes: Vec<String>,
    pub blotter_config: BlotterConfigJson,
    pub default_account_policy: AccountPolicyJson,
    #[serde(default)]
    pub accounts: BTreeMap<String, AccountPolicyOverrideJson>,
}

/// Serializable JSON schema for [`BlotterConfig`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlotterConfigJson {
    pub clean_price_tolerance: String,
    pub yield_tolerance: String,
    pub committed_overdue_seconds: u32,
    pub pre_close_alert_time: String,
    pub feed_staleness_seconds: Option<u32>,
}

/// Serializable JSON schema for default per-account desk policy.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountPolicyJson {
    pub default_quote_source_id: String,
    #[serde(default)]
    pub instrument_quote_sources: BTreeMap<String, String>,
    pub quote_staleness_seconds: u32,
    pub default_draft_hold_ttl_seconds: u32,
    #[serde(default)]
    pub limit_ceilings: BTreeMap<String, String>,
    #[serde(default)]
    pub regulatory_rule_pack: Option<RulePackBindingJson>,
}

/// Optional per-account override entry inside `accounts`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountPolicyOverrideJson {
    #[serde(default)]
    pub policy_version: Option<u64>,
    #[serde(default)]
    pub default_quote_source_id: Option<String>,
    #[serde(default)]
    pub instrument_quote_sources: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub quote_staleness_seconds: Option<u32>,
    #[serde(default)]
    pub default_draft_hold_ttl_seconds: Option<u32>,
    #[serde(default)]
    pub allowed_hedge_product_codes: Option<Vec<String>>,
    #[serde(default)]
    pub limit_ceilings: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub regulatory_rule_pack: Option<RulePackBindingJson>,
}

/// External trusted `MarketRulePack` binding referenced from policy JSON.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulePackBindingJson {
    pub rule_pack_id: String,
    pub version: u64,
    pub market: String,
    pub rule_type: String,
    pub source: String,
    pub effective_from_rfc3339: String,
    pub effective_to_rfc3339: String,
    pub verification_status: String,
    pub type_url: String,
    pub content_hash_sha256_hex: String,
    #[serde(default)]
    pub content_path: Option<String>,
    #[serde(default)]
    pub content_hex: Option<String>,
}

/// Validated runtime policy provider backing [`DeskBlotterPolicyProvider`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeskBlotterPolicyRuntime {
    pub policy_profile: String,
    pub blotter_config: BlotterConfig,
    pub default_account_policy: AccountDeskPolicy,
    pub account_overrides: BTreeMap<String, AccountDeskPolicy>,
}

impl DeskBlotterPolicyRuntime {
    /// Loads and validates the embedded default Policy B JSON.
    pub fn from_embedded_default() -> Result<Self, String> {
        Self::from_json_str(EMBEDDED_DEFAULT_POLICY_JSON, None)
    }

    /// Loads and validates a policy JSON file from disk (resolving relative rule-pack paths
    /// relative to the policy file's parent directory).
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, String> {
        let path_ref = path.as_ref();
        let raw = fs::read_to_string(path_ref)
            .map_err(|err| format!("failed to read policy file '{}': {err}", path_ref.display()))?;
        Self::from_json_str(&raw, path_ref.parent())
    }

    /// Parses and strictly validates a policy JSON string.
    pub fn from_json_str(json_text: &str, base_dir: Option<&Path>) -> Result<Self, String> {
        let file: DeskBlotterPolicyFile =
            serde_json::from_str(json_text).map_err(|err| format!("invalid policy json: {err}"))?;
        if file.schema_version != 1 {
            return Err(format!(
                "unsupported policy schema_version {}, expected 1",
                file.schema_version
            ));
        }
        if file.policy_version == 0 {
            return Err("policy_version must be >= 1".to_owned());
        }
        validate_non_empty(&file.policy_profile, "policy_profile")?;
        validate_product_codes(&file.allowed_hedge_product_codes)?;

        let blotter_config = parse_blotter_config(&file.blotter_config)?;
        let default_account_policy = parse_account_policy(
            file.policy_version,
            &file.allowed_hedge_product_codes,
            &file.default_account_policy,
            base_dir,
        )?;

        let mut account_overrides = BTreeMap::new();
        for (acct_id, ov) in file.accounts {
            validate_non_empty(&acct_id, "accounts.<key>")?;
            let codes = ov
                .allowed_hedge_product_codes
                .unwrap_or_else(|| file.allowed_hedge_product_codes.clone());
            validate_product_codes(&codes)?;

            let mut ceilings = default_account_policy.limit_ceilings.clone();
            if let Some(raw_ceilings) = ov.limit_ceilings {
                ceilings = parse_limit_ceilings(&raw_ceilings)?;
            }

            let reg_pack = match ov.regulatory_rule_pack {
                Some(binding) => Some(load_rule_pack_binding(&binding, base_dir)?),
                None => default_account_policy.regulatory_rule_pack.clone(),
            };

            let policy = AccountDeskPolicy {
                policy_version: ov.policy_version.unwrap_or(file.policy_version),
                default_quote_source_id: ov
                    .default_quote_source_id
                    .unwrap_or_else(|| default_account_policy.default_quote_source_id.clone()),
                instrument_quote_sources: ov
                    .instrument_quote_sources
                    .unwrap_or_else(|| default_account_policy.instrument_quote_sources.clone()),
                quote_staleness_seconds: ov
                    .quote_staleness_seconds
                    .unwrap_or(default_account_policy.quote_staleness_seconds),
                default_draft_hold_ttl_seconds: ov
                    .default_draft_hold_ttl_seconds
                    .unwrap_or(default_account_policy.default_draft_hold_ttl_seconds),
                allowed_hedge_product_codes: codes,
                regulatory_rule_pack: reg_pack,
                limit_ceilings: ceilings,
            };
            policy
                .validate()
                .map_err(|_| format!("invalid account policy override for '{acct_id}'"))?;
            account_overrides.insert(acct_id, policy);
        }

        Ok(Self {
            policy_profile: file.policy_profile,
            blotter_config,
            default_account_policy,
            account_overrides,
        })
    }

    /// Returns a cloned copy with an updated regulatory rule pack and limit ceilings applied to
    /// the default policy and/or a specific account (convenient for programmatic tests).
    #[must_use]
    pub fn with_account_policy(
        mut self,
        account_id: impl Into<String>,
        policy: AccountDeskPolicy,
    ) -> Self {
        self.account_overrides.insert(account_id.into(), policy);
        self
    }

    /// Builds the full map of desired policies for all persisted accounts on startup reconciliation.
    #[must_use]
    pub fn startup_policies_for_accounts<'a>(
        &self,
        account_ids: impl IntoIterator<Item = &'a str>,
    ) -> BTreeMap<String, AccountDeskPolicy> {
        let mut map = BTreeMap::new();
        for acct_id in account_ids {
            let policy = self
                .account_overrides
                .get(acct_id)
                .cloned()
                .unwrap_or_else(|| self.default_account_policy.clone());
            map.insert(acct_id.to_owned(), policy);
        }
        map
    }
}

impl DeskBlotterPolicyProvider for DeskBlotterPolicyRuntime {
    fn blotter_config(&self) -> BlotterConfig {
        self.blotter_config.clone()
    }

    fn account_policy(&self, account_id: &str) -> ApplicationResult<AccountDeskPolicy> {
        if account_id.trim().is_empty() || account_id != account_id.trim() {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::ValidationFailed,
                false,
            ));
        }
        Ok(self
            .account_overrides
            .get(account_id)
            .cloned()
            .unwrap_or_else(|| self.default_account_policy.clone()))
    }
}

fn parse_blotter_config(cfg: &BlotterConfigJson) -> Result<BlotterConfig, String> {
    let clean_price_tolerance = parse_fixed_decimal(&cfg.clean_price_tolerance).map_err(|_| {
        format!(
            "invalid clean_price_tolerance '{}'",
            cfg.clean_price_tolerance
        )
    })?;
    let yield_tolerance = parse_fixed_decimal(&cfg.yield_tolerance)
        .map_err(|_| format!("invalid yield_tolerance '{}'", cfg.yield_tolerance))?;
    let pre_close_alert_time = NaiveTime::parse_from_str(&cfg.pre_close_alert_time, "%H:%M:%S")
        .map_err(|err| format!("invalid pre_close_alert_time: {err}"))?;

    let out = BlotterConfig {
        clean_price_tolerance,
        yield_tolerance,
        committed_overdue_seconds: cfg.committed_overdue_seconds,
        pre_close_alert_time,
        feed_staleness_seconds: cfg.feed_staleness_seconds,
    };
    out.validate()
        .map_err(|err| format!("invalid blotter_config: {err}"))?;
    Ok(out)
}

fn parse_account_policy(
    policy_version: u64,
    allowed_codes: &[String],
    raw: &AccountPolicyJson,
    base_dir: Option<&Path>,
) -> Result<AccountDeskPolicy, String> {
    let limit_ceilings = parse_limit_ceilings(&raw.limit_ceilings)?;
    let regulatory_rule_pack = raw
        .regulatory_rule_pack
        .as_ref()
        .map(|b| load_rule_pack_binding(b, base_dir))
        .transpose()?;
    let policy = AccountDeskPolicy {
        policy_version,
        default_quote_source_id: raw.default_quote_source_id.clone(),
        instrument_quote_sources: raw.instrument_quote_sources.clone(),
        quote_staleness_seconds: raw.quote_staleness_seconds,
        default_draft_hold_ttl_seconds: raw.default_draft_hold_ttl_seconds,
        allowed_hedge_product_codes: allowed_codes.to_vec(),
        regulatory_rule_pack,
        limit_ceilings,
    };
    policy
        .validate()
        .map_err(|_| "invalid default_account_policy".to_owned())?;
    Ok(policy)
}

fn parse_limit_ceilings(
    raw: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, FixedDecimal>, String> {
    let mut out = BTreeMap::new();
    for (k, v) in raw {
        validate_non_empty(k, "limit_ceilings.<key>")?;
        let parsed = parse_fixed_decimal(v)
            .map_err(|_| format!("invalid limit ceiling for '{k}': '{v}'"))?;
        if !parsed.is_non_negative() {
            return Err(format!("limit ceiling for '{k}' must be non-negative"));
        }
        out.insert(k.clone(), parsed);
    }
    Ok(out)
}

fn load_rule_pack_binding(
    binding: &RulePackBindingJson,
    base_dir: Option<&Path>,
) -> Result<MarketRulePack, String> {
    let id = Ulid::new(binding.rule_pack_id.clone())
        .map_err(|_| "invalid regulatory_rule_pack.rule_pack_id".to_owned())?;
    let version = Version::new(binding.version)
        .map_err(|_| "invalid regulatory_rule_pack.version".to_owned())?;
    let from = parse_rfc3339_market_time(&binding.effective_from_rfc3339)?;
    let to = parse_rfc3339_market_time(&binding.effective_to_rfc3339)?;
    let effective = EffectivePeriod::new(from, to)
        .map_err(|_| "invalid regulatory_rule_pack effective period".to_owned())?;
    let status = match binding.verification_status.as_str() {
        "VERIFIED" => VerificationStatus::Verified,
        "UNVERIFIED" => VerificationStatus::Unverified,
        "REJECTED" => VerificationStatus::Rejected,
        other => {
            return Err(format!(
                "invalid regulatory_rule_pack.verification_status '{other}'"
            ));
        }
    };
    let digest_bytes = decode_digest_hex(&binding.content_hash_sha256_hex)?;
    let content_hash = ContentHash::from_bytes(&digest_bytes)
        .map_err(|_| "invalid regulatory_rule_pack content_hash".to_owned())?;

    let raw_bytes = match (&binding.content_path, &binding.content_hex) {
        (Some(rel), None) => {
            let candidate = Path::new(rel);
            let resolved = if candidate.is_absolute() {
                candidate.to_path_buf()
            } else if let Some(dir) = base_dir {
                dir.join(candidate)
            } else {
                candidate.to_path_buf()
            };
            fs::read(&resolved).map_err(|err| {
                format!(
                    "failed to read regulatory_rule_pack content '{}': {err}",
                    resolved.display()
                )
            })?
        }
        (None, Some(hex)) => decode_arbitrary_hex(hex)?,
        _ => {
            return Err(
                "regulatory_rule_pack must specify exactly one of content_path or content_hex"
                    .to_owned(),
            );
        }
    };

    let content = RulePackContent::new(binding.type_url.clone(), raw_bytes)
        .map_err(|_| "invalid regulatory_rule_pack content".to_owned())?;
    let owner_id = Ulid::new(DEFAULT_OWNER_ULID).expect("valid default owner ULID");
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id,
            version,
            owner: OwnerRef::new(owner_id.clone(), owner_id),
            market: binding.market.clone(),
            rule_type: binding.rule_type.clone(),
            source: binding.source.clone(),
            effective,
            verification_status: status,
            content_hash,
        },
        content,
    )
    .map_err(|_| "invalid regulatory_rule_pack".to_owned())
}

fn parse_rfc3339_market_time(raw: &str) -> Result<MarketTime, String> {
    let dt = DateTime::parse_from_rfc3339(raw)
        .map_err(|err| format!("invalid RFC3339 timestamp '{raw}': {err}"))?
        .with_timezone(&Utc);
    let local_date = dt.with_timezone(&Shanghai).date_naive();
    MarketTime::new(dt, MARKET_TIMEZONE, local_date)
        .map_err(|_| format!("invalid MarketTime '{raw}'"))
}

fn decode_arbitrary_hex(text: &str) -> Result<Vec<u8>, String> {
    let trimmed = text.trim();
    if trimmed != text || !trimmed.len().is_multiple_of(2) {
        return Err("invalid hex string in regulatory_rule_pack.content_hex".to_owned());
    }
    let mut out = Vec::with_capacity(trimmed.len() / 2);
    for pair in trimmed.as_bytes().chunks_exact(2) {
        let hi = hex_val(pair[0])?;
        let lo = hex_val(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_val(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err("invalid hex digit in content_hex".to_owned()),
    }
}

fn validate_product_codes(codes: &[String]) -> Result<(), String> {
    if codes.is_empty() {
        return Err("allowed_hedge_product_codes must be non-empty".to_owned());
    }
    let mut seen = BTreeSet::new();
    for code in codes {
        validate_non_empty(code, "allowed_hedge_product_codes[]")?;
        if !seen.insert(code.as_str()) {
            return Err(format!("duplicate allowed_hedge_product_code '{code}'"));
        }
    }
    Ok(())
}

fn validate_non_empty(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty() || value != value.trim() {
        return Err(format!(
            "{field} must be non-empty without leading/trailing whitespace"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::DeskBlotterPolicyRuntime;

    #[test]
    fn embedded_policy_default_loads_cleanly_and_rejects_invalid_json() {
        let policy = DeskBlotterPolicyRuntime::from_embedded_default().expect("embedded Policy B");
        assert_eq!(policy.policy_profile, "B");
        assert_eq!(
            policy
                .default_account_policy
                .allowed_hedge_product_codes
                .len(),
            4
        );
        assert!(DeskBlotterPolicyRuntime::from_json_str(r#"{"schema_version": 2}"#, None).is_err());
        assert!(
            DeskBlotterPolicyRuntime::from_json_str(
                r#"{"schema_version":1,"policy_version":1,"policy_profile":"X","allowed_hedge_product_codes":[],"blotter_config":{"clean_price_tolerance":"0.05","yield_tolerance":"0.0005","committed_overdue_seconds":3600,"pre_close_alert_time":"16:30:00","feed_staleness_seconds":300},"default_account_policy":{"default_quote_source_id":"q","quote_staleness_seconds":300,"default_draft_hold_ttl_seconds":1800}}"#,
                None,
            )
            .is_err()
        );
    }
}
