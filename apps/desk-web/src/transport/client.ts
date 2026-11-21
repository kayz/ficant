import { type MessageInitShape } from '@bufbuild/protobuf';
import {
  createClient,
  type CallOptions,
  type Interceptor,
  type Transport,
} from '@connectrpc/connect';
import { createGrpcWebTransport } from '@connectrpc/connect-web';
import type { OpeningPositionBase } from '../generated/ficant/blotter/v1/model_pb';
import {
  DeskBlotterService,
  type CancelCommittedRequestSchema,
  type CancelCommittedResult,
  type ConfirmDraftEarmarkRequestSchema,
  type ConfirmDraftEarmarkResult,
  type ConfirmHoldRequestSchema,
  type ConfirmHoldResult,
  type CreateCommittedRequestSchema,
  type CreateCommittedResult,
  type CreateHoldRequestSchema,
  type CreateHoldResult,
  type ExtendHoldRequestSchema,
  type ExtendHoldResult,
  type GetAccountSnapshotRequestSchema,
  type GetAccountSnapshotResult,
  type ImportOpeningBaseRequestSchema,
  type ImportOpeningBaseResult,
  type IngestMarketQuotesRequestSchema,
  type IngestMarketQuotesResult,
  type IngestTradeFeedRequestSchema,
  type IngestTradeFeedResult,
  type PollEventsRequestSchema,
  type PollEventsResult,
  type PreviewCoveredHedgeRequestSchema,
  type PreviewCoveredHedgeResult,
  type ReleaseHoldRequestSchema,
  type ReleaseHoldResult,
  type ResolvePendingMatchRequestSchema,
  type ResolvePendingMatchResult,
  type SetFeedControlRequestSchema,
  type SetFeedControlResult,
  type SubmitDraftEarmarkRequestSchema,
  type SubmitDraftEarmarkResult,
  type SubscribeEventsRequestSchema,
  type SubscribeEventsResponse,
  type SweepLifecycleRequestSchema,
  type SweepLifecycleResult,
} from '../generated/ficant/blotter/v1/service_pb';
import {
  IntranetNlpIntentParserService,
  PositionBaseProviderService,
  type FetchOpeningPositionBaseRequestSchema,
  type NlpEarmarkProposal,
  type ParseNlpIntentRequestSchema,
} from '../generated/ficant/blotter/v1/spi_pb';
import type { ErrorDetail } from '../generated/ficant/core/v1/error_pb';
import {
  assertExclusiveCredentialHeaders,
  assertSecureCredentialContext,
  type InMemoryCredentialStore,
  type LocationLike,
} from './auth';
import { BlotterClientError } from './error';

export interface RpcCallOptions {
  readonly signal?: AbortSignal | undefined;
  readonly timeoutMs?: number | undefined;
}

export interface DeskBlotterRpcClient {
  getAccountSnapshot(
    request: MessageInitShape<typeof GetAccountSnapshotRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<GetAccountSnapshotResult>;

  pollEvents(
    request: MessageInitShape<typeof PollEventsRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<PollEventsResult>;

  subscribeEvents(
    request: MessageInitShape<typeof SubscribeEventsRequestSchema>,
    options?: RpcCallOptions,
  ): AsyncIterable<SubscribeEventsResponse>;

  previewCoveredHedge(
    request: MessageInitShape<typeof PreviewCoveredHedgeRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<PreviewCoveredHedgeResult>;

  importOpeningBase(
    request: MessageInitShape<typeof ImportOpeningBaseRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<ImportOpeningBaseResult>;

  ingestMarketQuotes(
    request: MessageInitShape<typeof IngestMarketQuotesRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<IngestMarketQuotesResult>;

  createHold(
    request: MessageInitShape<typeof CreateHoldRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<CreateHoldResult>;

  extendHold(
    request: MessageInitShape<typeof ExtendHoldRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<ExtendHoldResult>;

  confirmHold(
    request: MessageInitShape<typeof ConfirmHoldRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<ConfirmHoldResult>;

  releaseHold(
    request: MessageInitShape<typeof ReleaseHoldRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<ReleaseHoldResult>;

  createCommitted(
    request: MessageInitShape<typeof CreateCommittedRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<CreateCommittedResult>;

  cancelCommitted(
    request: MessageInitShape<typeof CancelCommittedRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<CancelCommittedResult>;

  submitDraftEarmark(
    request: MessageInitShape<typeof SubmitDraftEarmarkRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<SubmitDraftEarmarkResult>;

  confirmDraftEarmark(
    request: MessageInitShape<typeof ConfirmDraftEarmarkRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<ConfirmDraftEarmarkResult>;

  ingestTradeFeed(
    request: MessageInitShape<typeof IngestTradeFeedRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<IngestTradeFeedResult>;

  resolvePendingMatch(
    request: MessageInitShape<typeof ResolvePendingMatchRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<ResolvePendingMatchResult>;

  sweepLifecycle(
    request: MessageInitShape<typeof SweepLifecycleRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<SweepLifecycleResult>;

  setFeedControl(
    request: MessageInitShape<typeof SetFeedControlRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<SetFeedControlResult>;

  fetchOpeningPositionBase(
    request: MessageInitShape<typeof FetchOpeningPositionBaseRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<OpeningPositionBase>;

  parseNlpIntent(
    request: MessageInitShape<typeof ParseNlpIntentRequestSchema>,
    options?: RpcCallOptions,
  ): Promise<NlpEarmarkProposal>;
}

export interface CreateDeskBlotterClientOptions {
  readonly credentialStore: InMemoryCredentialStore;
  /** Defaults to same-origin (`globalThis.location.origin` or `""`). */
  readonly baseUrl?: string | undefined;
  /** Optional custom fetch implementation (useful for unit tests and proxied environments). */
  readonly fetch?: typeof globalThis.fetch | undefined;
  /** Optional location override for HTTPS / loopback check. */
  readonly location?: LocationLike | undefined;
  /** Optional pre-built transport override. */
  readonly transport?: Transport | undefined;
}

export function resolveSameOriginBaseUrl(locationOverride?: LocationLike): string {
  if (locationOverride) {
    if (
      typeof locationOverride.origin === 'string' &&
      locationOverride.origin.length > 0 &&
      locationOverride.origin !== 'null'
    ) {
      return locationOverride.origin;
    }
    const portSuffix =
      typeof locationOverride.port === 'string' && locationOverride.port.length > 0
        ? `:${locationOverride.port}`
        : '';
    return `${locationOverride.protocol}//${locationOverride.hostname}${portSuffix}`;
  }
  if (
    typeof globalThis.location === 'object' &&
    globalThis.location !== null &&
    typeof globalThis.location.origin === 'string' &&
    globalThis.location.origin.length > 0 &&
    globalThis.location.origin !== 'null'
  ) {
    return globalThis.location.origin;
  }
  return '';
}

export function createCredentialAuthInterceptor(
  credentialStore: InMemoryCredentialStore,
  locationOverride?: LocationLike,
): Interceptor {
  return (next) => async (req) => {
    assertSecureCredentialContext(locationOverride);
    credentialStore.applyToHeaders(req.header);
    assertExclusiveCredentialHeaders(req.header);
    return next(req);
  };
}

export function createSameOriginGrpcWebTransport(
  options: CreateDeskBlotterClientOptions,
): Transport {
  const baseUrl = options.baseUrl ?? resolveSameOriginBaseUrl(options.location);
  const authInterceptor = createCredentialAuthInterceptor(
    options.credentialStore,
    options.location,
  );
  return createGrpcWebTransport({
    baseUrl,
    useBinaryFormat: true,
    interceptors: [authInterceptor],
    ...(options.fetch !== undefined ? { fetch: options.fetch } : {}),
  });
}

type ResultOneof<T> =
  | { readonly case: 'value'; readonly value: T }
  | { readonly case: 'error'; readonly value: ErrorDetail }
  | { readonly case: undefined; readonly value?: undefined };

function unwrapResultOneof<T>(
  rpcName: string,
  result: ResultOneof<T>,
  secrets: readonly string[],
): NonNullable<T> {
  if (result.case === 'value' && result.value !== undefined && result.value !== null) {
    return result.value as NonNullable<T>;
  }
  if (result.case === 'error') {
    throw BlotterClientError.fromErrorDetail(result.value, secrets);
  }
  throw BlotterClientError.protocol(`${rpcName} returned an empty result oneof.`);
}

export function decodeConfirmHoldResult(
  result: ConfirmHoldResult,
  secrets: readonly string[] = [],
): ConfirmHoldResult {
  if (result.reconfirmationError !== undefined) {
    throw BlotterClientError.fromConfirmHoldExpired(result, secrets);
  }
  return result;
}

function toConnectCallOptions(options?: RpcCallOptions): CallOptions | undefined {
  if (!options) {
    return undefined;
  }
  const out: CallOptions = {};
  if (options.signal !== undefined) {
    out.signal = options.signal;
  }
  if (options.timeoutMs !== undefined) {
    out.timeoutMs = options.timeoutMs;
  }
  return out;
}

export function createDeskBlotterClient(
  options: CreateDeskBlotterClientOptions,
): DeskBlotterRpcClient {
  const transport = options.transport ?? createSameOriginGrpcWebTransport(options);
  const blotter = createClient(DeskBlotterService, transport);
  const baseProvider = createClient(PositionBaseProviderService, transport);
  const nlpParser = createClient(IntranetNlpIntentParserService, transport);

  const secrets = (): readonly string[] => options.credentialStore.getRedactedSecrets();

  async function callUnary<T>(
    rpcName: string,
    invoke: () => Promise<{ readonly result: ResultOneof<T> }>,
  ): Promise<NonNullable<T>> {
    try {
      const response = await invoke();
      return unwrapResultOneof(rpcName, response.result, secrets());
    } catch (err) {
      throw BlotterClientError.fromUnknown(err, secrets());
    }
  }

  return {
    getAccountSnapshot(request, callOpts) {
      return callUnary('GetAccountSnapshot', () =>
        blotter.getAccountSnapshot(request, toConnectCallOptions(callOpts)),
      );
    },

    pollEvents(request, callOpts) {
      return callUnary('PollEvents', () =>
        blotter.pollEvents(request, toConnectCallOptions(callOpts)),
      );
    },

    subscribeEvents(request, callOpts) {
      const rawStream = blotter.subscribeEvents(request, toConnectCallOptions(callOpts));
      return {
        async *[Symbol.asyncIterator](): AsyncIterator<SubscribeEventsResponse> {
          try {
            for await (const frame of rawStream) {
              if (frame.payload.case === 'error') {
                throw BlotterClientError.fromErrorDetail(frame.payload.value, secrets());
              }
              if (frame.payload.case === undefined) {
                throw BlotterClientError.protocol(
                  'SubscribeEvents frame has empty payload oneof.',
                );
              }
              yield frame;
            }
          } catch (err) {
            throw BlotterClientError.fromUnknown(err, secrets());
          }
        },
      };
    },

    previewCoveredHedge(request, callOpts) {
      return callUnary('PreviewCoveredHedge', () =>
        blotter.previewCoveredHedge(request, toConnectCallOptions(callOpts)),
      );
    },

    importOpeningBase(request, callOpts) {
      return callUnary('ImportOpeningBase', () =>
        blotter.importOpeningBase(request, toConnectCallOptions(callOpts)),
      );
    },

    ingestMarketQuotes(request, callOpts) {
      return callUnary('IngestMarketQuotes', () =>
        blotter.ingestMarketQuotes(request, toConnectCallOptions(callOpts)),
      );
    },

    createHold(request, callOpts) {
      return callUnary('CreateHold', () =>
        blotter.createHold(request, toConnectCallOptions(callOpts)),
      );
    },

    extendHold(request, callOpts) {
      return callUnary('ExtendHold', () =>
        blotter.extendHold(request, toConnectCallOptions(callOpts)),
      );
    },

    async confirmHold(request, callOpts) {
      const value = await callUnary('ConfirmHold', () =>
        blotter.confirmHold(request, toConnectCallOptions(callOpts)),
      );
      return decodeConfirmHoldResult(value, secrets());
    },

    releaseHold(request, callOpts) {
      return callUnary('ReleaseHold', () =>
        blotter.releaseHold(request, toConnectCallOptions(callOpts)),
      );
    },

    createCommitted(request, callOpts) {
      return callUnary('CreateCommitted', () =>
        blotter.createCommitted(request, toConnectCallOptions(callOpts)),
      );
    },

    cancelCommitted(request, callOpts) {
      return callUnary('CancelCommitted', () =>
        blotter.cancelCommitted(request, toConnectCallOptions(callOpts)),
      );
    },

    submitDraftEarmark(request, callOpts) {
      return callUnary('SubmitDraftEarmark', () =>
        blotter.submitDraftEarmark(request, toConnectCallOptions(callOpts)),
      );
    },

    confirmDraftEarmark(request, callOpts) {
      return callUnary('ConfirmDraftEarmark', () =>
        blotter.confirmDraftEarmark(request, toConnectCallOptions(callOpts)),
      );
    },

    ingestTradeFeed(request, callOpts) {
      return callUnary('IngestTradeFeed', () =>
        blotter.ingestTradeFeed(request, toConnectCallOptions(callOpts)),
      );
    },

    resolvePendingMatch(request, callOpts) {
      return callUnary('ResolvePendingMatch', () =>
        blotter.resolvePendingMatch(request, toConnectCallOptions(callOpts)),
      );
    },

    sweepLifecycle(request, callOpts) {
      return callUnary('SweepLifecycle', () =>
        blotter.sweepLifecycle(request, toConnectCallOptions(callOpts)),
      );
    },

    setFeedControl(request, callOpts) {
      return callUnary('SetFeedControl', () =>
        blotter.setFeedControl(request, toConnectCallOptions(callOpts)),
      );
    },

    fetchOpeningPositionBase(request, callOpts) {
      return callUnary('FetchOpeningPositionBase', () =>
        baseProvider.fetchOpeningPositionBase(request, toConnectCallOptions(callOpts)),
      );
    },

    parseNlpIntent(request, callOpts) {
      return callUnary('ParseNlpIntent', () =>
        nlpParser.parseNlpIntent(request, toConnectCallOptions(callOpts)),
      );
    },
  };
}
