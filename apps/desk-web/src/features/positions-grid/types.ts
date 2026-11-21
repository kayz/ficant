import type { MessageInitShape } from '@bufbuild/protobuf';
import type {
  EarmarkRecord,
  InstrumentPositionView,
  OpeningInstrumentBalance,
} from '../../generated/ficant/blotter/v1/model_pb';
import type {
  CancelCommittedRequestSchema,
  ConfirmHoldRequestSchema,
  CreateCommittedRequestSchema,
  CreateHoldRequestSchema,
  ExtendHoldRequestSchema,
  ReleaseHoldRequestSchema,
} from '../../generated/ficant/blotter/v1/service_pb';
import type { BlotterStoreApi } from '../../store/blotterStore';
import type { DeskBlotterRpcClient } from '../../transport/client';
import type { WriteIntentManager } from '../../transport/idempotency';

export type PositionsGridRpcClient = Pick<
  DeskBlotterRpcClient,
  | 'createHold'
  | 'extendHold'
  | 'confirmHold'
  | 'releaseHold'
  | 'createCommitted'
  | 'cancelCommitted'
>;

export interface PositionsGridPanelProps {
  readonly store: BlotterStoreApi;
  readonly client: PositionsGridRpcClient;
  /** Optional shared WriteIntentManager (e.g. `supervisor.writeIntents`). Defaults to a session-scoped manager. */
  readonly writeIntents?: WriteIntentManager | undefined;
  /** Optional explicit account override; defaults to `store.getState().selectedAccountId`. */
  readonly accountId?: string | null | undefined;
  /** Called after any write commits (including `confirm_hold_expired`) so supervisor can trigger catch-up. */
  readonly onWriteCommitted?: ((committedSeq: bigint) => void) | undefined;
  /** Called when the user clicks Re-sync after a `CONCURRENCY_CONFLICT` / `VERSION_CONFLICT`. */
  readonly onResyncRequested?: ((accountId: string) => void | Promise<void>) | undefined;
  /** Deterministic clock provider for expiry / TTL display (defaults to `Date.now`). */
  readonly nowMs?: (() => number) | undefined;
  /** Fixed viewport height (px) for the virtualized grid scroll container (used as fallback in jsdom). */
  readonly viewportHeightPx?: number | undefined;
  /** Fixed/estimated row height (px) for virtualization. */
  readonly rowHeightPx?: number | undefined;
  /** Number of overscan rows for `@tanstack/react-virtual`. */
  readonly overscan?: number | undefined;
}

export interface PositionRowModel {
  readonly position: InstrumentPositionView;
  readonly openingBalance: OpeningInstrumentBalance | undefined;
  readonly activeEarmarks: readonly EarmarkRecord[];
  readonly activeHoldCount: number;
  readonly activeCommittedCount: number;
  readonly hasLifecycleAlert: boolean;
  readonly sellAmbiguityLockedT0: boolean;
  readonly sellAmbiguityLockedT1: boolean;
  readonly feedStaleOrPaused: boolean;
}

export type HoldActionMode =
  | 'create_hold'
  | 'create_committed'
  | 'extend_hold'
  | 'confirm_hold'
  | 'release_hold'
  | 'cancel_committed';

export type QuoteInputMode = 'omit' | 'settlement_only' | 'clean_price' | 'yield';

export type ReplayableHoldRequest =
  | {
      readonly operation: 'ConfirmHold';
      readonly idempotencyKey: string;
      readonly request: MessageInitShape<typeof ConfirmHoldRequestSchema>;
    }
  | {
      readonly operation: 'CreateHold';
      readonly idempotencyKey: string;
      readonly request: MessageInitShape<typeof CreateHoldRequestSchema>;
    }
  | {
      readonly operation: 'ExtendHold';
      readonly idempotencyKey: string;
      readonly request: MessageInitShape<typeof ExtendHoldRequestSchema>;
    }
  | {
      readonly operation: 'ReleaseHold';
      readonly idempotencyKey: string;
      readonly request: MessageInitShape<typeof ReleaseHoldRequestSchema>;
    }
  | {
      readonly operation: 'CreateCommitted';
      readonly idempotencyKey: string;
      readonly request: MessageInitShape<typeof CreateCommittedRequestSchema>;
    }
  | {
      readonly operation: 'CancelCommitted';
      readonly idempotencyKey: string;
      readonly request: MessageInitShape<typeof CancelCommittedRequestSchema>;
    };

export interface ReceiptReplayHandle {
  readonly receiptId: string;
  readonly sessionGeneration: number;
  readonly accountId: string;
  readonly replay: ReplayableHoldRequest;
}
