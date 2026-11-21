import {
  FeedHealthState,
  PrincipalRole,
  type AccountBlotterSnapshot,
  type SeqRange,
} from '../../generated/ficant/blotter/v1/model_pb';
import type { ConnectionHealthState } from '../../store/blotterStore';

export type SessionBadgeState =
  | 'auth_required'
  | 'idle'
  | 'bootstrapping'
  | 'stream_live'
  | 'poll_degraded'
  | 'recovering'
  | 'error';

export interface ConnectionBadgeDescriptor {
  readonly badgeState: SessionBadgeState;
  readonly rawState: ConnectionHealthState;
  readonly label: string;
  readonly failClosed: boolean;
  readonly recoverableWarning: boolean;
}

export function describeConnectionState(state: ConnectionHealthState): ConnectionBadgeDescriptor {
  switch (state) {
    case 'unauthenticated':
      return {
        badgeState: 'auth_required',
        rawState: state,
        label: '未认证 (auth_required)',
        failClosed: true,
        recoverableWarning: false,
      };
    case 'idle_no_accounts':
      return {
        badgeState: 'idle',
        rawState: state,
        label: '待输入账户 (idle)',
        failClosed: false,
        recoverableWarning: false,
      };
    case 'bootstrapping':
      return {
        badgeState: 'bootstrapping',
        rawState: state,
        label: '基线加载中 (bootstrapping)',
        failClosed: false,
        recoverableWarning: false,
      };
    case 'stream_live':
      return {
        badgeState: 'stream_live',
        rawState: state,
        label: '实时推流 (stream_live)',
        failClosed: false,
        recoverableWarning: false,
      };
    case 'poll_degraded':
      return {
        badgeState: 'poll_degraded',
        rawState: state,
        label: '轮询降级 (poll_degraded)',
        failClosed: false,
        recoverableWarning: true,
      };
    case 'recovering':
      return {
        badgeState: 'recovering',
        rawState: state,
        label: '多账户恢复中 (recovering)',
        failClosed: false,
        recoverableWarning: true,
      };
    case 'error':
      return {
        badgeState: 'error',
        rawState: state,
        label: '连接异常 (error)',
        failClosed: true,
        recoverableWarning: false,
      };
  }
}

/**
 * Parses comma-, semicolon-, or whitespace-separated explicit account IDs into a
 * deduplicated, order-preserving array of non-empty trimmed strings.
 */
export function parseExplicitAccountIds(rawInput: string): readonly string[] {
  const tokens = rawInput.split(/[\s,;，；]+/);
  const seen = new Set<string>();
  const result: string[] = [];
  for (const token of tokens) {
    const trimmed = token.trim();
    if (trimmed.length > 0 && !seen.has(trimmed)) {
      seen.add(trimmed);
      result.push(trimmed);
    }
  }
  return result;
}

export function formatPrincipalRoleLabel(role: PrincipalRole | null): string {
  switch (role) {
    case PrincipalRole.TRADER:
      return 'TRADER';
    case PrincipalRole.OPERATOR:
      return 'OPERATOR';
    case PrincipalRole.FEED_ADAPTER:
      return 'FEED_ADAPTER';
    case PrincipalRole.NLP_PARSER:
      return 'NLP_PARSER';
    case PrincipalRole.UNSPECIFIED:
    case null:
      return '未指定';
  }
}

export function canHintFetchOpeningBaseSpi(roleHint: PrincipalRole | null): boolean {
  if (roleHint === null || roleHint === PrincipalRole.UNSPECIFIED) {
    return true;
  }
  return roleHint === PrincipalRole.FEED_ADAPTER || roleHint === PrincipalRole.OPERATOR;
}

export function canHintImportOpeningBase(roleHint: PrincipalRole | null): boolean {
  if (roleHint === null || roleHint === PrincipalRole.UNSPECIFIED) {
    return true;
  }
  return roleHint === PrincipalRole.TRADER || roleHint === PrincipalRole.OPERATOR;
}

export function canHintPauseFeed(roleHint: PrincipalRole | null): boolean {
  if (roleHint === null || roleHint === PrincipalRole.UNSPECIFIED) {
    return true;
  }
  return roleHint === PrincipalRole.TRADER || roleHint === PrincipalRole.OPERATOR;
}

export function canHintForceClearGapPause(roleHint: PrincipalRole | null): boolean {
  if (roleHint === null || roleHint === PrincipalRole.UNSPECIFIED) {
    return true;
  }
  return roleHint === PrincipalRole.OPERATOR;
}

export function formatFeedHealthState(state: FeedHealthState): string {
  switch (state) {
    case FeedHealthState.HEALTHY:
      return '健康 (HEALTHY)';
    case FeedHealthState.STALE:
      return '陈旧 (STALE)';
    case FeedHealthState.GAP_DETECTED:
      return '缺口暂停 (GAP_DETECTED)';
    case FeedHealthState.PAUSED_BY_OPERATOR:
      return '人工暂停 (PAUSED_BY_OPERATOR)';
    case FeedHealthState.UNSPECIFIED:
      return '未指定 (UNSPECIFIED)';
  }
}

export function formatMissingFeedRanges(ranges: readonly SeqRange[]): string {
  if (ranges.length === 0) {
    return '无缺段';
  }
  return ranges.map((r) => `[${r.first.toString()}..${r.last.toString()}]`).join(', ');
}

export type BaselineProvenanceKind = 'spi' | 'import' | 'snapshot_base' | 'none';

export interface BaselineProvenanceInfo {
  readonly kind: BaselineProvenanceKind;
  readonly label: string;
  readonly sourceId: string | null;
  readonly tradingDate: string | null;
  readonly baseId: string | null;
}

export function describeBaselineProvenance(
  snapshot: AccountBlotterSnapshot | undefined,
  lastActionSource?: 'spi' | 'import',
): BaselineProvenanceInfo {
  const base = snapshot?.openingBase;
  if (!base) {
    return {
      kind: 'none',
      label: '无基线',
      sourceId: null,
      tradingDate: null,
      baseId: null,
    };
  }
  const sourceId = base.sourceId.trim().length > 0 ? base.sourceId : null;
  const tradingDate = base.tradingDate.trim().length > 0 ? base.tradingDate : null;
  const baseId = base.baseId?.value ?? null;
  if (lastActionSource === 'spi') {
    return {
      kind: 'spi',
      label: 'SPI 拉取基线 (PositionBaseProviderService)',
      sourceId,
      tradingDate,
      baseId,
    };
  }
  if (lastActionSource === 'import') {
    return {
      kind: 'import',
      label: '手工导入基线 (ImportOpeningBase)',
      sourceId,
      tradingDate,
      baseId,
    };
  }
  return {
    kind: 'snapshot_base',
    label: '已发布基线快照 (Import / SPI)',
    sourceId,
    tradingDate,
    baseId,
  };
}
