/// <reference types="vite/client" />

interface TrustedMarketTimeRecord {
  readonly instant: string;
  readonly market_timezone: string;
  readonly local_trading_date: string;
}

interface TrustedRulePackManifest {
  readonly schema: 'ficant.desk-web.rulepack-manifest.v1';
  readonly approval_ref: string;
  readonly rule_pack_id: string;
  readonly version: number;
  readonly owner: {
    readonly tenant_id: string;
    readonly owner_id: string;
  };
  readonly market: string;
  readonly rule_type: string;
  readonly source: string;
  readonly verification_status: 'VERIFIED';
  readonly effective_from: TrustedMarketTimeRecord;
  readonly effective_to: TrustedMarketTimeRecord;
  readonly type_url: string;
  readonly payload_asset_path: string;
  readonly payload_byte_length: number;
  readonly payload_sha256: string;
}

interface TrustedRulePackBuildAnchor {
  readonly manifestSha256: string;
  readonly payloadSha256: string;
  readonly payloadByteLength: number;
  readonly payloadAssetPath: string;
  readonly manifest: TrustedRulePackManifest;
}

declare const __FICANT_TRUSTED_RULEPACK__: TrustedRulePackBuildAnchor;

declare module 'virtual:ficant-trusted-rulepack' {
  export type {
    TrustedMarketTimeRecord,
    TrustedRulePackBuildAnchor,
    TrustedRulePackManifest,
  };
  export const TRUSTED_RULEPACK_ANCHOR: TrustedRulePackBuildAnchor;
  export default TRUSTED_RULEPACK_ANCHOR;
}
