import { fromBinary } from '@bufbuild/protobuf';
import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { existsSync } from 'node:fs';
import {
  createServer,
  request as httpRequest,
  type ClientRequest,
  type IncomingMessage,
  type Server,
  type ServerResponse,
} from 'node:http';
import { createServer as createNetServer } from 'node:net';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  GetAccountSnapshotRequestSchema,
  PollEventsRequestSchema,
  SubscribeEventsRequestSchema,
} from '../src/generated/ficant/blotter/v1/service_pb';

const THIS_DIR = dirname(fileURLToPath(import.meta.url));
const DESK_WEB_ROOT = resolve(THIS_DIR, '..');
const DIST_INDEX = join(DESK_WEB_ROOT, 'dist', 'index.html');
const VITE_BIN = join(DESK_WEB_ROOT, 'node_modules', 'vite', 'bin', 'vite.js');

const SUBSCRIBE_EVENTS_PATH = '/ficant.blotter.v1.DeskBlotterService/SubscribeEvents';
const POLL_EVENTS_PATH = '/ficant.blotter.v1.DeskBlotterService/PollEvents';
const GET_SNAPSHOT_PATH = '/ficant.blotter.v1.DeskBlotterService/GetAccountSnapshot';

export interface RecordedProxyRequest {
  readonly index: number;
  readonly path: string;
  readonly timestampMs: number;
  readonly hasAuthorization: boolean;
  readonly hasCustomCredential: boolean;
  readonly decodedAccountId?: string;
  readonly decodedAfterSeq?: bigint;
  readonly blockedByFault: boolean;
}

export interface SnapshotDelayGate {
  readonly arrived: Promise<void>;
  readonly release: () => void;
}

interface InternalDelayGate {
  readonly promise: Promise<void>;
  readonly markArrived: () => void;
  readonly release: () => void;
  consumed: boolean;
}

interface ActiveStreamEntry {
  readonly clientReq: IncomingMessage;
  readonly clientRes: ServerResponse;
  upstreamReq: ClientRequest | null;
  upstreamRes: IncomingMessage | null;
}

function decodeFirstGrpcWebPayload(body: Buffer): Uint8Array | null {
  if (body.byteLength < 5) {
    return null;
  }
  const flag = body.readUInt8(0);
  if (flag !== 0x00) {
    return null;
  }
  const len = body.readUInt32BE(1);
  if (body.byteLength < 5 + len) {
    return null;
  }
  return body.subarray(5, 5 + len);
}

function readRequestBody(req: IncomingMessage): Promise<Buffer> {
  return new Promise((resolveBody, rejectBody) => {
    const chunks: Buffer[] = [];
    req.on('data', (chunk: Buffer) => chunks.push(chunk));
    req.on('end', () => resolveBody(Buffer.concat(chunks)));
    req.on('error', rejectBody);
  });
}

export function getFreeLoopbackPort(): Promise<number> {
  return new Promise((resolvePort, rejectPort) => {
    const srv = createNetServer();
    srv.unref();
    srv.on('error', rejectPort);
    srv.listen(0, '127.0.0.1', () => {
      const addr = srv.address();
      if (!addr || typeof addr === 'string') {
        srv.close(() => rejectPort(new Error('Failed to bind ephemeral loopback port.')));
        return;
      }
      const { port } = addr;
      srv.close((err) => {
        if (err) {
          rejectPort(err);
        } else {
          resolvePort(port);
        }
      });
    });
  });
}

function writeGrpcWebUnavailable(res: ServerResponse, message: string): void {
  if (res.destroyed || res.writableEnded) {
    return;
  }
  const trailerText = `grpc-status:14\r\ngrpc-message:${encodeURIComponent(message)}\r\n`;
  const trailerBytes = Buffer.from(trailerText, 'utf8');
  const frame = Buffer.alloc(5 + trailerBytes.byteLength);
  frame.writeUInt8(0x80, 0);
  frame.writeUInt32BE(trailerBytes.byteLength, 1);
  trailerBytes.copy(frame, 5);
  if (res.headersSent) {
    res.end(frame);
    return;
  }
  res.writeHead(200, {
    'content-type': 'application/grpc-web+proto',
    'cache-control': 'no-store',
    'content-length': String(frame.byteLength),
  });
  res.end(frame);
}

export class ControllableProxy {
  readonly #server: Server;
  readonly #origin: string;
  #upstreamUrl: URL;
  #blockSubscribe = false;
  #blockPoll = false;
  readonly #snapshotDelayGates = new Map<string, InternalDelayGate>();
  readonly #activeStreams = new Set<ActiveStreamEntry>();
  readonly #streamWaiters = new Set<() => void>();
  readonly #recordedRequests: RecordedProxyRequest[] = [];
  #requestSeq = 0;

  private constructor(server: Server, origin: string, upstreamOrigin: string) {
    this.#server = server;
    this.#origin = origin;
    this.#upstreamUrl = new URL(upstreamOrigin);
  }

  static async start(upstreamOrigin: string): Promise<ControllableProxy> {
    let instance: ControllableProxy | null = null;
    const server = createServer((req, res) => {
      if (instance) {
        void instance.#handleRequest(req, res);
      } else {
        res.writeHead(503);
        res.end();
      }
    });

    await new Promise<void>((resolveListen, rejectListen) => {
      server.on('error', rejectListen);
      server.listen(0, '127.0.0.1', () => resolveListen());
    });

    const addr = server.address();
    if (!addr || typeof addr === 'string') {
      server.close();
      throw new Error('Failed to bind controllable proxy server.');
    }

    instance = new ControllableProxy(server, `http://127.0.0.1:${addr.port}`, upstreamOrigin);
    return instance;
  }

  get origin(): string {
    return this.#origin;
  }

  setUpstreamOrigin(upstreamOrigin: string): void {
    this.#upstreamUrl = new URL(upstreamOrigin);
  }

  blockSubscribeEvents(blocked: boolean): void {
    this.#blockSubscribe = blocked;
  }

  blockPollEvents(blocked: boolean): void {
    this.#blockPoll = blocked;
  }

  delaySnapshotForAccount(accountId: string): SnapshotDelayGate {
    let resolveArrived!: () => void;
    const arrived = new Promise<void>((resolvePromise) => {
      resolveArrived = resolvePromise;
    });
    let resolveRelease!: () => void;
    const releasePromise = new Promise<void>((resolvePromise) => {
      resolveRelease = resolvePromise;
    });

    const gate: InternalDelayGate = {
      promise: releasePromise,
      markArrived: resolveArrived,
      release: resolveRelease,
      consumed: false,
    };
    this.#snapshotDelayGates.set(accountId, gate);

    return {
      arrived,
      release: () => {
        this.#snapshotDelayGates.delete(accountId);
        gate.release();
      },
    };
  }

  resetFaults(): void {
    this.#blockSubscribe = false;
    this.#blockPoll = false;
    for (const gate of this.#snapshotDelayGates.values()) {
      gate.release();
    }
    this.#snapshotDelayGates.clear();
  }

  get activeStreamCount(): number {
    return this.#activeStreams.size;
  }

  async waitForActiveStream(minCount = 1, timeoutMs = 15_000): Promise<void> {
    if (this.#activeStreams.size >= minCount) {
      return;
    }
    await new Promise<void>((resolveWait, rejectWait) => {
      const timer = setTimeout(() => {
        this.#streamWaiters.delete(check);
        rejectWait(
          new Error(
            `Timed out waiting for >= ${minCount} active SubscribeEvents stream(s); current=${this.#activeStreams.size}`,
          ),
        );
      }, timeoutMs);

      const check = (): void => {
        if (this.#activeStreams.size >= minCount) {
          clearTimeout(timer);
          this.#streamWaiters.delete(check);
          resolveWait();
        }
      };
      this.#streamWaiters.add(check);
    });
  }

  destroyActiveStreams(): number {
    const entries = Array.from(this.#activeStreams);
    this.#activeStreams.clear();
    const trailerText = 'grpc-status:14\r\ngrpc-message:proxy%20forced%20stream%20cut\r\n';
    const trailerBytes = Buffer.from(trailerText, 'utf8');
    const trailerFrame = Buffer.alloc(5 + trailerBytes.byteLength);
    trailerFrame.writeUInt8(0x80, 0);
    trailerFrame.writeUInt32BE(trailerBytes.byteLength, 1);
    trailerBytes.copy(trailerFrame, 5);

    for (const entry of entries) {
      if (entry.upstreamRes && !entry.upstreamRes.destroyed) {
        entry.upstreamRes.destroy(new Error('proxy forced stream cut'));
      }
      if (entry.upstreamReq && !entry.upstreamReq.destroyed) {
        entry.upstreamReq.destroy(new Error('proxy forced stream cut'));
      }
      if (!entry.clientRes.destroyed && !entry.clientRes.writableEnded) {
        if (entry.clientRes.headersSent) {
          entry.clientRes.end(trailerFrame);
        } else {
          writeGrpcWebUnavailable(entry.clientRes, 'proxy forced stream cut');
        }
      }
    }
    return entries.length;
  }

  getRecordedRequests(): readonly RecordedProxyRequest[] {
    return [...this.#recordedRequests];
  }

  clearRecordedRequests(): void {
    this.#recordedRequests.length = 0;
  }

  async close(): Promise<void> {
    this.resetFaults();
    this.destroyActiveStreams();
    await new Promise<void>((resolveClose) => {
      this.#server.close(() => resolveClose());
    });
  }

  #notifyStreamWaiters(): void {
    for (const waiter of Array.from(this.#streamWaiters)) {
      waiter();
    }
  }

  async #handleRequest(req: IncomingMessage, res: ServerResponse): Promise<void> {
    const path = req.url ?? '/';
    const hasAuthorization = req.headers['authorization'] !== undefined;
    const hasCustomCredential = req.headers['x-ficant-credential'] !== undefined;

    let body: Buffer;
    try {
      body = await readRequestBody(req);
    } catch {
      if (!res.destroyed && !res.writableEnded) {
        res.destroy();
      }
      return;
    }

    let decodedAccountId: string | undefined;
    let decodedAfterSeq: bigint | undefined;
    const payload = decodeFirstGrpcWebPayload(body);
    if (payload !== null) {
      try {
        if (path === GET_SNAPSHOT_PATH) {
          decodedAccountId = fromBinary(GetAccountSnapshotRequestSchema, payload).accountId;
        } else if (path === SUBSCRIBE_EVENTS_PATH) {
          decodedAfterSeq = fromBinary(SubscribeEventsRequestSchema, payload).afterSeq;
        } else if (path === POLL_EVENTS_PATH) {
          decodedAfterSeq = fromBinary(PollEventsRequestSchema, payload).afterSeq;
        }
      } catch {
        // Leave decoded metadata undefined on malformed test payloads.
      }
    }

    const blockedByFault =
      (path === SUBSCRIBE_EVENTS_PATH && this.#blockSubscribe) ||
      (path === POLL_EVENTS_PATH && this.#blockPoll);

    this.#requestSeq += 1;
    this.#recordedRequests.push({
      index: this.#requestSeq,
      path,
      timestampMs: Date.now(),
      hasAuthorization,
      hasCustomCredential,
      ...(decodedAccountId !== undefined ? { decodedAccountId } : {}),
      ...(decodedAfterSeq !== undefined ? { decodedAfterSeq } : {}),
      blockedByFault,
    });

    if (blockedByFault) {
      writeGrpcWebUnavailable(res, `proxy fault-injected block for ${path}`);
      return;
    }

    let delayGate: InternalDelayGate | undefined;
    if (path === GET_SNAPSHOT_PATH && decodedAccountId !== undefined) {
      const candidate = this.#snapshotDelayGates.get(decodedAccountId);
      if (candidate && !candidate.consumed) {
        candidate.consumed = true;
        delayGate = candidate;
        delayGate.markArrived();
      }
    }

    const isSubscribeStream = path === SUBSCRIBE_EVENTS_PATH;
    const headers: Record<string, string | string[] | undefined> = { ...req.headers };
    headers['host'] = this.#upstreamUrl.host;
    headers['accept-encoding'] = 'identity';
    headers['content-length'] = String(body.byteLength);
    delete headers['connection'];
    delete headers['transfer-encoding'];

    if (delayGate) {
      await this.#forwardBufferedWithGate(req, res, path, headers, body, delayGate);
      return;
    }

    const streamEntry: ActiveStreamEntry | null = isSubscribeStream
      ? {
          clientReq: req,
          clientRes: res,
          upstreamReq: null,
          upstreamRes: null,
        }
      : null;

    const cleanupStream = (): void => {
      if (streamEntry) {
        this.#activeStreams.delete(streamEntry);
      }
    };

    const upstreamReq = httpRequest(
      {
        protocol: this.#upstreamUrl.protocol,
        hostname: this.#upstreamUrl.hostname,
        port: this.#upstreamUrl.port,
        method: req.method ?? 'POST',
        path,
        headers,
      },
      (upstreamRes) => {
        if (streamEntry) {
          streamEntry.upstreamRes = upstreamRes;
          this.#activeStreams.add(streamEntry);
          this.#notifyStreamWaiters();
        }

        const respHeaders: Record<string, string | string[] | undefined> = {
          ...upstreamRes.headers,
        };
        respHeaders['x-accel-buffering'] = 'no';
        if (!res.destroyed && !res.writableEnded) {
          res.writeHead(upstreamRes.statusCode ?? 200, respHeaders);
          if (res.socket) {
            res.socket.setNoDelay(true);
          }
          res.flushHeaders();
        }

        upstreamRes.on('data', (chunk: Buffer) => {
          if (!res.destroyed && !res.writableEnded) {
            res.write(chunk);
          }
        });
        upstreamRes.on('end', () => {
          cleanupStream();
          if (!res.destroyed && !res.writableEnded) {
            res.end();
          }
        });
        upstreamRes.on('error', () => {
          cleanupStream();
          if (!res.destroyed) {
            res.destroy();
          }
        });
      },
    );

    if (streamEntry) {
      streamEntry.upstreamReq = upstreamReq;
    }

    upstreamReq.on('error', (err) => {
      cleanupStream();
      if (!res.destroyed && !res.writableEnded) {
        writeGrpcWebUnavailable(res, `upstream error: ${err.message}`);
      }
    });

    const onClientClosed = (): void => {
      cleanupStream();
      if (!upstreamReq.destroyed) {
        upstreamReq.destroy();
      }
    };
    res.on('close', onClientClosed);
    req.on('aborted', onClientClosed);

    upstreamReq.end(body);
  }

  async #forwardBufferedWithGate(
    req: IncomingMessage,
    res: ServerResponse,
    path: string,
    headers: Record<string, string | string[] | undefined>,
    body: Buffer,
    delayGate: InternalDelayGate,
  ): Promise<void> {
    const upstreamResult = await new Promise<{
      readonly statusCode: number;
      readonly headers: Record<string, string | string[] | undefined>;
      readonly chunks: Buffer;
    } | null>((resolveUpstream) => {
      const upstreamReq = httpRequest(
        {
          protocol: this.#upstreamUrl.protocol,
          hostname: this.#upstreamUrl.hostname,
          port: this.#upstreamUrl.port,
          method: req.method ?? 'POST',
          path,
          headers,
        },
        (upstreamRes) => {
          const chunks: Buffer[] = [];
          upstreamRes.on('data', (chunk: Buffer) => chunks.push(chunk));
          upstreamRes.on('end', () => {
            resolveUpstream({
              statusCode: upstreamRes.statusCode ?? 200,
              headers: { ...upstreamRes.headers },
              chunks: Buffer.concat(chunks),
            });
          });
          upstreamRes.on('error', () => resolveUpstream(null));
        },
      );
      upstreamReq.on('error', () => resolveUpstream(null));
      upstreamReq.end(body);
    });

    await delayGate.promise;

    if (res.destroyed || res.writableEnded) {
      return;
    }
    if (!upstreamResult) {
      writeGrpcWebUnavailable(res, 'upstream request failed while delayed');
      return;
    }
    res.writeHead(upstreamResult.statusCode, upstreamResult.headers);
    res.end(upstreamResult.chunks);
  }
}

export interface PreviewWithProxyHandle {
  readonly baseUrl: string;
  readonly proxy: ControllableProxy;
  setUpstreamOrigin(origin: string): void;
  stop(): Promise<void>;
}

async function waitForHttpReady(url: string, proc: ChildProcess, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (proc.exitCode !== null) {
      throw new Error(`vite preview exited early with code ${String(proc.exitCode)}.`);
    }
    try {
      const res = await fetch(url);
      if (res.ok) {
        return;
      }
    } catch {
      // Waiting for preview listen.
    }
    await new Promise((r) => setTimeout(r, 150));
  }
  throw new Error(`Timed out waiting for vite preview at ${url}.`);
}

async function terminateChildProcess(proc: ChildProcess): Promise<void> {
  if (proc.exitCode !== null) {
    return;
  }
  const exited = new Promise<void>((resolveExit) => {
    proc.once('exit', () => resolveExit());
  });
  if (process.platform === 'win32' && proc.pid) {
    spawnSync('taskkill', ['/PID', String(proc.pid), '/T', '/F'], {
      stdio: 'ignore',
      windowsHide: true,
    });
  } else {
    proc.kill('SIGTERM');
  }
  await Promise.race([exited, new Promise((r) => setTimeout(r, 4000))]);
  if (proc.exitCode === null) {
    proc.kill('SIGKILL');
  }
}

export async function startPreviewWithProxy(
  upstreamOrigin: string,
): Promise<PreviewWithProxyHandle> {
  if (!existsSync(DIST_INDEX)) {
    throw new Error(
      `Missing built bundle at ${DIST_INDEX}. Run "pnpm --dir apps/desk-web run build" before E2E.`,
    );
  }
  if (!existsSync(VITE_BIN)) {
    throw new Error(`Missing local vite CLI at ${VITE_BIN}.`);
  }

  const proxy = await ControllableProxy.start(upstreamOrigin);
  const previewPort = await getFreeLoopbackPort();
  const baseUrl = `http://127.0.0.1:${previewPort}/`;

  let previewLogs = '';
  const previewProc = spawn(
    process.execPath,
    [VITE_BIN, 'preview', '--host', '127.0.0.1', '--port', String(previewPort), '--strictPort'],
    {
      cwd: DESK_WEB_ROOT,
      env: {
        ...process.env,
        FICANT_UPSTREAM_ORIGIN: proxy.origin,
      },
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    },
  );
  previewProc.stdout.on('data', (chunk: Buffer) => {
    previewLogs += chunk.toString('utf8');
  });
  previewProc.stderr.on('data', (chunk: Buffer) => {
    previewLogs += chunk.toString('utf8');
  });

  try {
    await waitForHttpReady(baseUrl, previewProc, 20_000);
  } catch (err) {
    await terminateChildProcess(previewProc);
    await proxy.close();
    throw new Error(
      `Failed to start vite preview: ${err instanceof Error ? err.message : String(err)}\nLogs:\n${previewLogs}`,
    );
  }

  return {
    baseUrl,
    proxy,
    setUpstreamOrigin(origin: string) {
      proxy.setUpstreamOrigin(origin);
    },
    async stop() {
      await terminateChildProcess(previewProc);
      await proxy.close();
    },
  };
}
