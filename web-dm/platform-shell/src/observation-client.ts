import { create } from "@bufbuild/protobuf";
import { createClient } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import { UlidSchema } from "../../packages/contracts-generated/src/ficant/core/v1/common_pb";
import {
  ExperimentService,
  GetGraphRunRequestSchema,
  ListNodeOutputManifestsRequestSchema,
  ReadNodeOutputRequestSchema,
  TraceGraphOutputRequestSchema,
  type GetGraphRunResponse,
  type ListNodeOutputManifestsResponse,
  type ReadNodeOutputResponse,
  type TraceGraphOutputResponse,
} from "../../packages/contracts-generated/src/ficant/research/v1/experiment_pb";

export interface Phase5AObservationClient {
  getGraphRun(runId: string, signal?: AbortSignal): Promise<GetGraphRunResponse>;
  listNodeOutputManifests(
    runId: string,
    signal?: AbortSignal,
  ): Promise<ListNodeOutputManifestsResponse>;
  traceGraphOutput(
    runId: string,
    nodeId: string,
    signal?: AbortSignal,
  ): Promise<TraceGraphOutputResponse>;
  readNodeOutput(
    runId: string,
    nodeId: string,
    signal?: AbortSignal,
  ): Promise<ReadNodeOutputResponse>;
}

export function createGrpcWebObservationClient(baseUrl: string): Phase5AObservationClient {
  const normalizedBaseUrl = validateBaseUrl(baseUrl);
  const transport = createGrpcWebTransport({
    baseUrl: normalizedBaseUrl,
    useBinaryFormat: true,
  });
  const client = createClient(ExperimentService, transport);
  const ulid = (value: string) => create(UlidSchema, { value });
  return {
    getGraphRun: (runId, signal) =>
      client.getGraphRun(create(GetGraphRunRequestSchema, { runId: ulid(runId) }), { signal }),
    listNodeOutputManifests: (runId, signal) =>
      client.listNodeOutputManifests(
        create(ListNodeOutputManifestsRequestSchema, { runId: ulid(runId) }),
        { signal },
      ),
    traceGraphOutput: (runId, nodeId, signal) =>
      client.traceGraphOutput(
        create(TraceGraphOutputRequestSchema, { runId: ulid(runId), nodeId: ulid(nodeId) }),
        { signal },
      ),
    readNodeOutput: (runId, nodeId, signal) =>
      client.readNodeOutput(
        create(ReadNodeOutputRequestSchema, { runId: ulid(runId), nodeId: ulid(nodeId) }),
        { signal },
      ),
  };
}

function validateBaseUrl(value: string): string {
  const url = new URL(value, window.location.origin);
  if (url.username || url.password || url.search || url.hash) {
    throw new Error("gRPC-Web base URL 不得包含凭据、查询参数或片段");
  }
  if (url.protocol !== "https:" && !isLoopbackHttp(url)) {
    throw new Error("gRPC-Web base URL 必须使用 HTTPS；本机回环开发环境除外");
  }
  return url.origin + url.pathname.replace(/\/$/, "");
}

function isLoopbackHttp(url: URL): boolean {
  return url.protocol === "http:" && ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname);
}
