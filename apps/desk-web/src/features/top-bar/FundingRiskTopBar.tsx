import { useStore } from 'zustand';
import {
  TRUSTED_RULEPACK_ANCHOR,
  type TrustedRulePackBuildAnchor,
} from 'virtual:ficant-trusted-rulepack';
import type { BlotterStoreApi } from '../../store/blotterStore';
import { selectFeedStatuses, selectSelectedSnapshot } from '../../store/selectors';
import {
  MISSING_MEASURE_TEXT,
  assessUncoveredRisk,
  describeFundingCompletenessFactors,
  evaluateRulePackTrustAnchor,
  formatDecimalWithUnit,
  formatSeqRanges,
  formatTaxBasisLabel,
  formatTopBarFeedState,
} from './topBarHelpers';

export interface FundingRiskTopBarProps {
  readonly store: BlotterStoreApi;
  readonly rulePackAnchor?: TrustedRulePackBuildAnchor | null | undefined;
  readonly valuationAt?: Date | undefined;
}

function MetricCell({
  label,
  valueText,
  unitText,
  testId,
}: {
  readonly label: string;
  readonly valueText: string;
  readonly unitText?: string | null;
  readonly testId: string;
}) {
  return (
    <div data-testid={testId}>
      <span>{label}：</span>
      <strong data-testid={`${testId}-value`}>{valueText}</strong>
      {unitText !== undefined && unitText !== null && (
        <span data-testid={`${testId}-unit`}> ({unitText})</span>
      )}
    </div>
  );
}

export function FundingRiskTopBar({
  store,
  rulePackAnchor,
  valuationAt,
}: FundingRiskTopBarProps) {
  const state = useStore(store);
  const snapshot = selectSelectedSnapshot(state);
  const resolvedAnchor = rulePackAnchor !== undefined ? rulePackAnchor : TRUSTED_RULEPACK_ANCHOR;
  const trustStatus = evaluateRulePackTrustAnchor(resolvedAnchor, valuationAt);

  if (!snapshot) {
    return (
      <header
        aria-label="资金与覆盖风险顶栏"
        data-testid="funding-risk-top-bar"
      >
        <div data-testid="top-bar-empty">尚未加载选中账户快照</div>
        <div
          data-testid="rulepack-trust-pill"
          data-trust-status={trustStatus.status}
        >
          <span>{trustStatus.label}</span> | <span>{trustStatus.rulePackId}</span> (
          <span>{trustStatus.version}</span>) | 有效期：<span>{trustStatus.effectiveWindow}</span>
        </div>
      </header>
    );
  }

  const risk = snapshot.riskSummary;
  const funding = snapshot.fundingSummary;
  const feeds = selectFeedStatuses(state, snapshot.accountId);

  const coveredDv01 = formatDecimalWithUnit(risk?.coveredNetPreTaxDv01);
  const coveredGross = formatDecimalWithUnit(risk?.coveredGrossNotional);
  const uncoveredGross = formatDecimalWithUnit(risk?.uncoveredGrossNotional);
  const uncoveredNet = formatDecimalWithUnit(risk?.uncoveredNetNotional);
  const uncoveredAssessment = assessUncoveredRisk(risk);

  const openingCashT0 = formatDecimalWithUnit(funding?.openingCashT0);
  const netFlowT0 = formatDecimalWithUnit(funding?.netSettlementFlowT0);
  const projectedCashT0 = formatDecimalWithUnit(funding?.projectedCashT0);
  const gapT0 = formatDecimalWithUnit(funding?.fundingGapT0);

  const openingDeltaT1 = formatDecimalWithUnit(funding?.openingNetCashDeltaT1);
  const netFlowT1 = formatDecimalWithUnit(funding?.netSettlementFlowT1);
  const projectedCashT1 = formatDecimalWithUnit(funding?.projectedCashT1);
  const gapT1 = formatDecimalWithUnit(funding?.fundingGapT1);

  const pendingCashT0 = formatDecimalWithUnit(funding?.pendingMatchCashT0);
  const pendingCashT1 = formatDecimalWithUnit(funding?.pendingMatchCashT1);

  const completeness = describeFundingCompletenessFactors(funding);

  return (
    <header
      aria-label="资金与覆盖风险顶栏"
      data-testid="funding-risk-top-bar"
      data-snapshot-seq={snapshot.snapshotSeq.toString()}
      data-account-id={snapshot.accountId}
    >
      {/* Header Metadata & RulePack Trust Pill */}
      <div data-testid="top-bar-meta">
        <span data-testid="top-bar-account">账户：{snapshot.accountId}</span>
        <span data-testid="top-bar-snapshot-seq">
          快照序号 seq={snapshot.snapshotSeq.toString()}
        </span>
        <span
          data-testid="rulepack-trust-pill"
          data-trust-status={trustStatus.status}
          title={trustStatus.detail}
        >
          <strong>{trustStatus.label}</strong> | id={trustStatus.rulePackId} |{' '}
          {trustStatus.version} | 有效期 {trustStatus.effectiveWindow}
        </span>
      </div>

      {/* 1. Covered PRE_TAX Risk */}
      <section aria-label="已覆盖税前风险" data-testid="covered-risk-section">
        <h3>已覆盖风险 ({risk ? formatTaxBasisLabel(risk.coveredTaxBasis) : MISSING_MEASURE_TEXT})</h3>
        <MetricCell
          label="已覆盖笔数"
          valueText={risk !== undefined ? String(risk.coveredInstrumentCount) : MISSING_MEASURE_TEXT}
          testId="covered-instrument-count"
        />
        <MetricCell
          label="已覆盖税前净 DV01"
          valueText={coveredDv01.text}
          unitText={coveredDv01.unitText}
          testId="covered-net-pre-tax-dv01"
        />
        <MetricCell
          label="已覆盖总面额"
          valueText={coveredGross.text}
          unitText={coveredGross.unitText}
          testId="covered-gross-notional"
        />
      </section>

      {/* 2. Uncovered Breakdown (Always rendered when count > 0 or gross > 0, plus summary) */}
      <section aria-label="未覆盖敞口明细" data-testid="uncovered-risk-section">
        <h3>未覆盖敞口</h3>
        <MetricCell
          label="未覆盖笔数"
          valueText={risk !== undefined ? String(risk.uncoveredInstrumentCount) : MISSING_MEASURE_TEXT}
          testId="uncovered-instrument-count"
        />
        {(uncoveredAssessment.shouldRenderBreakdown || risk !== undefined) && (
          <>
            <MetricCell
              label="未覆盖总面额 (Gross)"
              valueText={uncoveredGross.text}
              unitText={uncoveredGross.unitText}
              testId="uncovered-gross-notional"
            />
            <MetricCell
              label="未覆盖带符号净面额 (Signed Net)"
              valueText={uncoveredNet.text}
              unitText={uncoveredNet.unitText}
              testId="uncovered-net-notional"
            />
          </>
        )}

        {uncoveredAssessment.hasUncoveredWarning && (
          <div
            role="alert"
            data-testid="uncovered-risk-warning"
            data-whole-account-blocked={uncoveredAssessment.wholeAccountBlocked ? 'true' : 'false'}
            data-net-zero-gross-nonzero={
              uncoveredAssessment.isNetZeroWithGrossNonZero ? 'true' : 'false'
            }
            data-pure-short={uncoveredAssessment.isPureShortNetNegative ? 'true' : 'false'}
          >
            {uncoveredAssessment.warningMessages.map((msg, idx) => (
              <p key={idx}>{msg}</p>
            ))}
          </div>
        )}
      </section>

      {/* 3. T+0 / T+1 Cash Gaps & Independent Pending Match Cash */}
      <section aria-label="资金缺口与独立待核销现金" data-testid="funding-gaps-section">
        <h3>T+0 / T+1 资金缺口</h3>
        <div data-testid="funding-t0-group">
          <MetricCell
            label="T+0 期初现金"
            valueText={openingCashT0.text}
            unitText={openingCashT0.unitText}
            testId="funding-opening-cash-t0"
          />
          <MetricCell
            label="T+0 净结算流"
            valueText={netFlowT0.text}
            unitText={netFlowT0.unitText}
            testId="funding-net-flow-t0"
          />
          <MetricCell
            label="T+0 预计现金"
            valueText={projectedCashT0.text}
            unitText={projectedCashT0.unitText}
            testId="funding-projected-cash-t0"
          />
          <MetricCell
            label="T+0 资金缺口"
            valueText={gapT0.text}
            unitText={gapT0.unitText}
            testId="funding-gap-t0"
          />
        </div>

        <div data-testid="funding-t1-group">
          <MetricCell
            label="T+1 期初净变动"
            valueText={openingDeltaT1.text}
            unitText={openingDeltaT1.unitText}
            testId="funding-opening-delta-t1"
          />
          <MetricCell
            label="T+1 净结算流"
            valueText={netFlowT1.text}
            unitText={netFlowT1.unitText}
            testId="funding-net-flow-t1"
          />
          <MetricCell
            label="T+1 预计现金"
            valueText={projectedCashT1.text}
            unitText={projectedCashT1.unitText}
            testId="funding-projected-cash-t1"
          />
          <MetricCell
            label="T+1 资金缺口"
            valueText={gapT1.text}
            unitText={gapT1.unitText}
            testId="funding-gap-t1"
          />
        </div>

        <div data-testid="pending-match-cash-section">
          <h4>独立待核销现金（未并入已结算资金）</h4>
          <MetricCell
            label="待核销现金 T+0"
            valueText={pendingCashT0.text}
            unitText={pendingCashT0.unitText}
            testId="pending-match-cash-t0"
          />
          <MetricCell
            label="待核销现金 T+1"
            valueText={pendingCashT1.text}
            unitText={pendingCashT1.unitText}
            testId="pending-match-cash-t1"
          />
          <MetricCell
            label="待核销记录笔数"
            valueText={
              funding !== undefined ? String(funding.pendingMatchCount) : MISSING_MEASURE_TEXT
            }
            testId="pending-match-count"
          />
          {completeness.pendingZeroNetStillIncomplete && (
            <div role="status" data-testid="pending-zero-net-warning">
              待核销正负现金相抵为 0，但仍存在 {funding?.pendingMatchCount ?? 0}{' '}
              笔未核销记录，资金完整性保持不完整。
            </div>
          )}
        </div>
      </section>

      {/* 4. Five Funding Completeness Factors */}
      <section aria-label="五项资金完整性因子" data-testid="funding-completeness-section">
        <h3>
          资金完整性：
          <span
            data-testid="funding-completeness-badge"
            data-complete-exact={
              completeness.factors !== null
                ? completeness.factors.fundingIsCompleteExact
                  ? 'true'
                  : 'false'
                : 'unknown'
            }
          >
            {completeness.factors === null
              ? MISSING_MEASURE_TEXT
              : completeness.factors.fundingIsCompleteExact
                ? '完整精确 (EXACT)'
                : '不完整 / 含估算 (INCOMPLETE)'}
          </span>
        </h3>

        {completeness.items.length > 0 ? (
          <ul data-testid="funding-completeness-factors-list">
            {completeness.items.map((factor) => (
              <li
                key={factor.key}
                data-testid={`completeness-factor-${factor.key}`}
                data-triggered={factor.triggered ? 'true' : 'false'}
              >
                <strong>{factor.label}</strong>：<span>{factor.valueText}</span> [
                <span>{factor.triggered ? '触发/不完整' : '正常'}</span>] —{' '}
                <span>{factor.explanation}</span>
              </li>
            ))}
          </ul>
        ) : (
          <p data-testid="funding-completeness-missing">{MISSING_MEASURE_TEXT}</p>
        )}
      </section>

      {/* 5. Feed Health Overview */}
      {feeds.length > 0 && (
        <section aria-label="成交源健康概览" data-testid="top-bar-feed-health">
          <h3>成交源健康</h3>
          <ul>
            {feeds.map((feed) => (
              <li
                key={feed.sourceId}
                data-testid={`top-bar-feed-${feed.sourceId}`}
                data-earmarks-paused={feed.earmarksPaused ? 'true' : 'false'}
              >
                <strong>{feed.sourceId}</strong> | {formatTopBarFeedState(feed.healthState)} |{' '}
                earmarks_paused={feed.earmarksPaused ? 'true' : 'false'} | 缺段=
                {formatSeqRanges(feed.missingFeedRanges)} | 缺口暂停=
                {feed.pausedByGap ? '是' : '否'} | 陈旧暂停=
                {feed.pausedByStaleness ? '是' : '否'} | 人工暂停=
                {feed.pausedByOperator ? '是' : '否'}
                {feed.lastHeartbeatAt !== undefined
                  ? ` | 心跳=${feed.lastHeartbeatAt.localTradingDate}`
                  : ''}
              </li>
            ))}
          </ul>
        </section>
      )}
    </header>
  );
}
