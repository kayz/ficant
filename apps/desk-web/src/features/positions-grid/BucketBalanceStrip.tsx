import type { ReactElement } from 'react';
import {
  EarmarkState,
  FeedHealthState,
  type AccountBlotterSnapshot,
  type BucketBalanceView,
  type InstrumentPositionView,
} from '../../generated/ficant/blotter/v1/model_pb';
import { formatDecimalValue } from '../../codec/decimal';
import { formatSettlementBucket } from './formatters';

export interface BucketBalanceStripProps {
  readonly snapshot: AccountBlotterSnapshot;
  readonly selectedPosition: InstrumentPositionView | undefined;
}

function BucketCard({
  label,
  bucket,
  testId,
}: {
  readonly label: string;
  readonly bucket: BucketBalanceView | undefined;
  readonly testId: string;
}): ReactElement {
  if (!bucket) {
    return (
      <div data-testid={testId} style={{ border: '1px solid #d1d5db', padding: '8px', borderRadius: '4px' }}>
        <strong>{label}</strong>
        <div>未提供分桶余额</div>
      </div>
    );
  }

  return (
    <div
      data-testid={testId}
      style={{
        border: bucket.sellAmbiguityLocked ? '1px solid #dc2626' : '1px solid #d1d5db',
        padding: '8px',
        borderRadius: '4px',
        flex: '1 1 260px',
      }}
    >
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '4px' }}>
        <strong>
          {label} ({formatSettlementBucket(bucket.settlementBucket)} · {bucket.settlementDate || '—'})
        </strong>
        {bucket.sellAmbiguityLocked ? (
          <span
            data-testid={`${testId}-sell-lock`}
            style={{ color: '#b91c1c', fontWeight: 600 }}
          >
            sell_ambiguity_locked
          </span>
        ) : (
          <span style={{ color: '#15803d' }}>未锁卖单</span>
        )}
      </div>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(2, minmax(0, 1fr))', gap: '4px', fontSize: '12px' }}>
        <div>可用面额: {formatDecimalValue(bucket.availableNotional, { grouping: true })}</div>
        <div>
          HOLD 买/卖: {formatDecimalValue(bucket.holdBuyNotional, { grouping: true })} /{' '}
          {formatDecimalValue(bucket.holdSellNotional, { grouping: true })}
        </div>
        <div>
          未核销 COMMITTED 买/卖:{' '}
          {formatDecimalValue(bucket.committedBuyUnreconciledNotional, { grouping: true })} /{' '}
          {formatDecimalValue(bucket.committedSellUnreconciledNotional, { grouping: true })}
        </div>
        <div>
          已核销 买/卖: {formatDecimalValue(bucket.reconciledBuyNotional, { grouping: true })} /{' '}
          {formatDecimalValue(bucket.reconciledSellNotional, { grouping: true })}
        </div>
        <div>
          待核销 买/卖:{' '}
          {formatDecimalValue(bucket.pendingMatchBuyUnresolvedNotional, { grouping: true })} /{' '}
          {formatDecimalValue(bucket.pendingMatchSellUnresolvedNotional, { grouping: true })}
        </div>
      </div>
    </div>
  );
}

export function BucketBalanceStrip({
  snapshot,
  selectedPosition,
}: BucketBalanceStripProps): ReactElement {
  let activeHoldCount = 0;
  let activeCommittedCount = 0;
  for (const em of snapshot.activeEarmarks) {
    if (em.state === EarmarkState.HOLD) {
      activeHoldCount += 1;
    } else if (em.state === EarmarkState.COMMITTED) {
      activeCommittedCount += 1;
    }
  }

  let sellLockedPositions = 0;
  for (const pos of snapshot.positions) {
    if (pos.bucketT0?.sellAmbiguityLocked || pos.bucketT1?.sellAmbiguityLocked) {
      sellLockedPositions += 1;
    }
  }

  const pausedFeeds = snapshot.feedStatuses.filter(
    (f) => f.earmarksPaused || f.healthState !== FeedHealthState.HEALTHY,
  );

  const focusPosition = selectedPosition ?? snapshot.positions[0];

  return (
    <section
      aria-label="账户分桶余额条"
      data-testid="bucket-balance-strip"
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: '8px',
        padding: '8px',
        border: '1px solid #cbd5e1',
        borderRadius: '6px',
        backgroundColor: '#f8fafc',
      }}
    >
      <div
        data-testid="account-bucket-summary"
        style={{ display: 'flex', flexWrap: 'wrap', gap: '12px', fontSize: '13px', alignItems: 'center' }}
      >
        <span>
          账户: <strong>{snapshot.accountId}</strong>
        </span>
        <span>快照序号: {snapshot.snapshotSeq.toString()}</span>
        <span>账户版本: v{snapshot.accountVersion.toString()}</span>
        <span>持仓券种: {snapshot.positions.length}</span>
        <span>活动 HOLD: {activeHoldCount}</span>
        <span>活动 COMMITTED: {activeCommittedCount}</span>
        <span>卖单歧义锁定券数: {sellLockedPositions}</span>
        {pausedFeeds.length > 0 ? (
          <span data-testid="feed-warning-badge" style={{ color: '#b91c1c', fontWeight: 600 }}>
            Feed 异常/暂停: {pausedFeeds.map((f) => f.sourceId).join(', ')}
          </span>
        ) : null}
      </div>

      {focusPosition ? (
        <div>
          <div style={{ fontSize: '12px', marginBottom: '4px' }}>
            当前焦点券种分桶 (<strong>{focusPosition.instrumentCode}</strong> · 持仓版本 v
            {focusPosition.positionVersion.toString()} · 经济净面额:{' '}
            {formatDecimalValue(focusPosition.economicNetNotional, { grouping: true })})
          </div>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: '8px' }}>
            <BucketCard
              label="T+0 分桶"
              bucket={focusPosition.bucketT0}
              testId="bucket-balance-t0"
            />
            <BucketCard
              label="T+1 分桶"
              bucket={focusPosition.bucketT1}
              testId="bucket-balance-t1"
            />
          </div>
        </div>
      ) : (
        <div style={{ fontSize: '12px' }}>当前账户暂无持仓分桶记录。</div>
      )}
    </section>
  );
}
