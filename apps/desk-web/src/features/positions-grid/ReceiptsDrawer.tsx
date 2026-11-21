import type { ReactElement } from 'react';
import type { WriteReceiptRecord } from '../../store/blotterStore';
import { formatEarmarkState, formatErrorCode } from './formatters';
import type { ReceiptReplayHandle } from './types';

export interface ReceiptsDrawerProps {
  readonly accountId: string;
  readonly sessionGeneration: number;
  readonly receipts: readonly WriteReceiptRecord[];
  readonly replayHandlesByReceiptId: Readonly<Record<string, ReceiptReplayHandle>>;
  readonly replayingReceiptId: string | null;
  readonly onReplayReceipt: (handle: ReceiptReplayHandle) => Promise<void>;
  readonly onResyncRequested?: ((accountId: string) => void | Promise<void>) | undefined;
}

function formatReceiptStatusLabel(status: WriteReceiptRecord['status']): string {
  switch (status) {
    case 'succeeded':
      return '成功 (SUCCEEDED)';
    case 'confirm_hold_expired':
      return '已过期提交 (EXPIRED)';
    case 'failed':
      return '失败 (FAILED)';
  }
}

export function ReceiptsDrawer({
  accountId,
  sessionGeneration,
  receipts,
  replayHandlesByReceiptId,
  replayingReceiptId,
  onReplayReceipt,
  onResyncRequested,
}: ReceiptsDrawerProps): ReactElement {
  const scopedReceipts = receipts.filter((r) => r.accountId === accountId);

  return (
    <section
      aria-label="占仓操作回执"
      data-testid="hold-receipts-drawer"
      data-session-generation={sessionGeneration.toString()}
      data-account-id={accountId}
      style={{
        border: '1px solid #cbd5e1',
        borderRadius: '6px',
        padding: '10px',
        backgroundColor: '#ffffff',
      }}
    >
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '8px' }}>
        <h3 style={{ margin: 0, fontSize: '14px' }}>
          持久操作回执 ({scopedReceipts.length}) · 账户 {accountId} · 会话代次 #{sessionGeneration}
        </h3>
      </div>

      {scopedReceipts.length === 0 ? (
        <div data-testid="receipts-empty" style={{ fontSize: '12px', color: '#64748b' }}>
          暂无占仓写操作回执。执行 CreateHold / ExtendHold / ReleaseHold / ConfirmHold 后，即使活动行移出表格，回执仍将持久保留。
        </div>
      ) : (
        <ul
          data-testid="receipts-list"
          style={{ listStyle: 'none', padding: 0, margin: 0, display: 'flex', flexDirection: 'column', gap: '8px' }}
        >
          {[...scopedReceipts].reverse().map((receipt) => {
            const handle = replayHandlesByReceiptId[receipt.id];
            const isReplaying = replayingReceiptId === receipt.id;
            const err = receipt.error;
            const detail = err?.detail;
            const isConflict = err?.isConcurrencyConflict() ?? false;

            return (
              <li
                key={receipt.id}
                data-testid={`receipt-item-${receipt.id}`}
                data-receipt-operation={receipt.operation}
                data-receipt-status={receipt.status}
                data-receipt-committed-seq={receipt.committedSeq.toString()}
                data-receipt-replayed={receipt.replayed ? 'true' : 'false'}
                style={{
                  border:
                    receipt.status === 'confirm_hold_expired'
                      ? '1px solid #d97706'
                      : receipt.status === 'failed'
                        ? '1px solid #dc2626'
                        : '1px solid #16a34a',
                  borderRadius: '4px',
                  padding: '8px',
                  fontSize: '12px',
                  backgroundColor:
                    receipt.status === 'confirm_hold_expired'
                      ? '#fffbeb'
                      : receipt.status === 'failed'
                        ? '#fef2f2'
                        : '#f0fdf4',
                }}
              >
                <div style={{ display: 'flex', flexWrap: 'wrap', gap: '8px', alignItems: 'center' }}>
                  <strong>{receipt.operation}</strong>
                  <span>状态: {formatReceiptStatusLabel(receipt.status)}</span>
                  <span data-testid={`receipt-seq-${receipt.id}`}>
                    committed_seq: {receipt.committedSeq > 0n ? receipt.committedSeq.toString() : '—'}
                  </span>
                  <span data-testid={`receipt-replayed-${receipt.id}`}>
                    {receipt.replayed ? 'replayed=true (幂等重放)' : 'replayed=false (首次提交)'}
                  </span>
                  {receipt.earmarkId ? <span>earmark_id: {receipt.earmarkId}</span> : null}
                  {receipt.earmark ? (
                    <span data-testid={`receipt-earmark-state-${receipt.id}`}>
                      占仓终态: {formatEarmarkState(receipt.earmark.state)} (rev{' '}
                      {receipt.earmark.earmarkRevision.toString()})
                    </span>
                  ) : null}
                </div>

                {receipt.status === 'confirm_hold_expired' ? (
                  <div
                    role="alert"
                    data-testid={`receipt-reconfirmation-error-${receipt.id}`}
                    style={{ marginTop: '6px', color: '#92400e' }}
                  >
                    <div>
                      原 HOLD 已到期释放并提交为 EXPIRED（committed_seq=
                      {receipt.committedSeq.toString()}），重确认被拒：
                    </div>
                    <div>
                      错误码: {formatErrorCode(detail?.code ?? err?.errorCode ?? 0)} ·{' '}
                      {detail?.message || err?.message || '重确认失败'}
                    </div>
                    {detail?.traceId || err?.traceId ? (
                      <div>trace_id: {detail?.traceId || err?.traceId}</div>
                    ) : null}
                    {detail?.resourceRef || err?.resourceRef ? (
                      <div>resource_ref: {detail?.resourceRef || err?.resourceRef}</div>
                    ) : null}
                    {err && err.fieldViolations.length > 0 ? (
                      <div>
                        字段错误:{' '}
                        {err.fieldViolations
                          .map((fv) => `${fv.field}: ${fv.description}`)
                          .join('; ')}
                      </div>
                    ) : null}
                  </div>
                ) : err ? (
                  <div role="alert" style={{ marginTop: '6px', color: '#b91c1c' }}>
                    错误码: {formatErrorCode(err.errorCode)} · {err.message}
                    {err.traceId ? ` (trace_id: ${err.traceId})` : ''}
                    {err.fieldViolations.length > 0
                      ? ` [${err.fieldViolations.map((fv) => `${fv.field}: ${fv.description}`).join('; ')}]`
                      : ''}
                  </div>
                ) : null}

                <div style={{ display: 'flex', gap: '8px', marginTop: '6px' }}>
                  {handle ? (
                    <button
                      type="button"
                      data-testid={`receipt-replay-btn-${receipt.id}`}
                      disabled={isReplaying}
                      onClick={() => {
                        void onReplayReceipt(handle);
                      }}
                    >
                      {isReplaying ? '重放中...' : '按原幂等键重放 (Replay)'}
                    </button>
                  ) : null}
                  {isConflict && onResyncRequested ? (
                    <button
                      type="button"
                      data-testid={`receipt-resync-btn-${receipt.id}`}
                      onClick={() => {
                        void onResyncRequested(accountId);
                      }}
                    >
                      重新同步快照 (Re-sync)
                    </button>
                  ) : null}
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
