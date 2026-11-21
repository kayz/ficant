// @vitest-environment node
import { create, fromBinary, toBinary } from '@bufbuild/protobuf';
import { createClient } from '@connectrpc/connect';
import { createGrpcWebTransport } from '@connectrpc/connect-web';
import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import { createServer as createNetServer } from 'node:net';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build as viteBuild } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import {
  DeskBlotterService,
  GetAccountSnapshotRequestSchema,
  GetAccountSnapshotResponseSchema,
  SubscribeEventsRequestSchema,
  SubscribeEventsResponseSchema,
} from '../generated/ficant/blotter/v1/service_pb';

const THIS_DIR = dirname(fileURLToPath(import.meta.url));
const DESK_WEB_ROOT = resolve(THIS_DIR, '..', '..');
const VITE_BIN = join(DESK_WEB_ROOT, 'node_modules', 'vite', 'bin', 'vite.js');

function encodeGrpcWebFrame(flag: number, payload: Uint8Array): Buffer {
  const header = Buffer.alloc(5);
  header.writeUInt8(flag, 0);
  header.writeUInt32BE(payload.byteLength, 1);
  return Buffer.concat([header, Buffer.from(payload)]);
}

function decodeFirstGrpcWebMessage(raw: Buffer): Uint8Array {
  if (raw.byteLength < 5) {
    throw new Error(`Truncated gRPC-Web frame: ${raw.byteLength} bytes.`);
  }
  const flag = raw.readUInt8(0);
  if (flag !== 0x00) {
    throw new Error(`Expected data frame flag 0x00, got 0x${flag.toString(16)}.`);
  }
  const length = raw.readUInt32BE(1);
  if (raw.byteLength < 5 + length) {
    throw new Error(`Incomplete gRPC-Web payload: expected ${length}, got ${raw.byteLength - 5}.`);
  }
  return raw.subarray(5, 5 + length);
}

function readBody(req: IncomingMessage): Promise<Buffer> {
  return new Promise((resolveBody, rejectBody) => {
    const chunks: Buffer[] = [];
    req.on('data', (chunk: Buffer) => chunks.push(chunk));
    req.on('end', () => resolveBody(Buffer.concat(chunks)));
    req.on('error', rejectBody);
  });
}

function getFreePort(): Promise<number> {
  return new Promise((resolvePort, rejectPort) => {
    const srv = createNetServer();
    srv.unref();
    srv.on('error', rejectPort);
    srv.listen(0, '127.0.0.1', () => {
      const addr = srv.address();
      if (!addr || typeof addr === 'string') {
        srv.close(() => rejectPort(new Error('Could not resolve ephemeral port.')));
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

async function waitForServer(url: string, proc: ChildProcess, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (proc.exitCode !== null) {
      throw new Error(`Vite process exited early with code ${String(proc.exitCode)}.`);
    }
    try {
      const res = await fetch(url);
      if (res.status < 500) {
        return;
      }
    } catch {
      // Waiting for socket listen.
    }
    await new Promise((r) => setTimeout(r, 150));
  }
  throw new Error(`Timed out waiting for Vite server at ${url}.`);
}

async function stopProcess(proc: ChildProcess | null): Promise<void> {
  if (!proc || proc.exitCode !== null) {
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
  await Promise.race([exited, new Promise((r) => setTimeout(r, 3000))]);
}

interface MockUpstreamController {
  readonly server: Server;
  readonly origin: string;
  readonly lastAuthHeaders: string[];
  isStreamStillOpen: () => boolean;
  waitForStreamCancel: () => Promise<void>;
  resetStreamState: () => void;
  closeAllStreams: () => void;
}

async function startMockGrpcWebUpstream(): Promise<MockUpstreamController> {
  const lastAuthHeaders: string[] = [];
  const activeStreams = new Set<ServerResponse>();
  let streamEnded = false;
  let resolveCancel: (() => void) | null = null;
  let cancelPromise = new Promise<void>((resolve) => {
    resolveCancel = resolve;
  });

  const server = createServer((req, res) => {
    void (async () => {
      const body = await readBody(req);
      const authHeader = req.headers['authorization'] ?? req.headers['x-ficant-credential'] ?? '';
      lastAuthHeaders.push(Array.isArray(authHeader) ? authHeader.join(',') : authHeader);

      if (req.url === '/ficant.blotter.v1.DeskBlotterService/GetAccountSnapshot') {
        const msgBytes = decodeFirstGrpcWebMessage(body);
        const decodedReq = fromBinary(GetAccountSnapshotRequestSchema, msgBytes);
        const resp = create(GetAccountSnapshotResponseSchema, {
          result: {
            case: 'value',
            value: {
              snapshot: {
                accountId: decodedReq.accountId,
                snapshotSeq: 77n,
              },
            },
          },
        });
        const dataFrame = encodeGrpcWebFrame(
          0x00,
          toBinary(GetAccountSnapshotResponseSchema, resp),
        );
        const trailerFrame = encodeGrpcWebFrame(0x80, Buffer.from('grpc-status:0\r\n', 'utf8'));
        res.writeHead(200, {
          'content-type': 'application/grpc-web+proto',
          'cache-control': 'no-store',
        });
        res.end(Buffer.concat([dataFrame, trailerFrame]));
        return;
      }

      if (req.url === '/ficant.blotter.v1.DeskBlotterService/SubscribeEvents') {
        const msgBytes = decodeFirstGrpcWebMessage(body);
        const decodedReq = fromBinary(SubscribeEventsRequestSchema, msgBytes);
        streamEnded = false;
        activeStreams.add(res);

        const firstFrameMsg = create(SubscribeEventsResponseSchema, {
          payload: {
            case: 'heartbeat',
            value: {
              headSeq: decodedReq.afterSeq + 32n,
              scannedThroughSeq: decodedReq.afterSeq + 32n,
            },
          },
        });
        const frame1 = encodeGrpcWebFrame(
          0x00,
          toBinary(SubscribeEventsResponseSchema, firstFrameMsg),
        );

        res.writeHead(200, {
          'content-type': 'application/grpc-web+proto',
          'cache-control': 'no-store',
        });
        res.flushHeaders();
        res.write(frame1);

        const markClosed = (): void => {
          streamEnded = true;
          activeStreams.delete(res);
          resolveCancel?.();
        };
        res.on('close', markClosed);
        req.on('close', () => {
          if (!res.writableEnded) {
            markClosed();
          }
        });
        return;
      }

      res.writeHead(404);
      res.end('not found');
    })();
  });

  await new Promise<void>((resolveListen, rejectListen) => {
    server.on('error', rejectListen);
    server.listen(0, '127.0.0.1', () => resolveListen());
  });

  const addr = server.address();
  if (!addr || typeof addr === 'string') {
    throw new Error('Failed to bind mock upstream server.');
  }

  return {
    server,
    origin: `http://127.0.0.1:${addr.port}`,
    lastAuthHeaders,
    isStreamStillOpen: () => !streamEnded,
    waitForStreamCancel: () => cancelPromise,
    resetStreamState: () => {
      streamEnded = false;
      cancelPromise = new Promise<void>((resolve) => {
        resolveCancel = resolve;
      });
    },
    closeAllStreams: () => {
      for (const s of activeStreams) {
        s.destroy();
      }
      activeStreams.clear();
    },
  };
}

describe('Vite dev & preview same-origin gRPC-Web binary unary and unbuffered streaming proxy', () => {
  let upstream: MockUpstreamController;
  let devProc: ChildProcess | null = null;
  let previewProc: ChildProcess | null = null;
  let devBaseUrl = '';
  let previewBaseUrl = '';
  let tempPreviewOutDir = '';

  beforeAll(async () => {
    upstream = await startMockGrpcWebUpstream();
    tempPreviewOutDir = mkdtempSync(join(tmpdir(), 'ficant-proxy-preview-dist-'));

    // Build an isolated production bundle into os.tmpdir() so preview proxy test works even before manual build.
    await viteBuild({
      root: DESK_WEB_ROOT,
      configFile: join(DESK_WEB_ROOT, 'vite.config.ts'),
      logLevel: 'silent',
      build: {
        outDir: tempPreviewOutDir,
        emptyOutDir: true,
      },
    });

    const devPort = await getFreePort();
    devBaseUrl = `http://127.0.0.1:${devPort}`;
    devProc = spawn(
      process.execPath,
      [VITE_BIN, '--host', '127.0.0.1', '--port', String(devPort), '--strictPort'],
      {
        cwd: DESK_WEB_ROOT,
        env: { ...process.env, FICANT_UPSTREAM_ORIGIN: upstream.origin },
        stdio: 'ignore',
        windowsHide: true,
      },
    );

    const previewPort = await getFreePort();
    previewBaseUrl = `http://127.0.0.1:${previewPort}`;
    previewProc = spawn(
      process.execPath,
      [
        VITE_BIN,
        'preview',
        '--host',
        '127.0.0.1',
        '--port',
        String(previewPort),
        '--strictPort',
        '--outDir',
        tempPreviewOutDir,
      ],
      {
        cwd: DESK_WEB_ROOT,
        env: { ...process.env, FICANT_UPSTREAM_ORIGIN: upstream.origin },
        stdio: 'ignore',
        windowsHide: true,
      },
    );

    await Promise.all([
      waitForServer(`${devBaseUrl}/`, devProc, 20000),
      waitForServer(`${previewBaseUrl}/`, previewProc, 20000),
    ]);
  }, 45000);

  afterAll(async () => {
    upstream?.closeAllStreams();
    await Promise.all([stopProcess(devProc), stopProcess(previewProc)]);
    if (upstream) {
      await new Promise<void>((resolveClose) => upstream.server.close(() => resolveClose()));
    }
    if (tempPreviewOutDir) {
      rmSync(tempPreviewOutDir, { recursive: true, force: true });
    }
  });

  for (const mode of ['dev', 'preview'] as const) {
    it(`proxies binary gRPC-Web unary and delivers first streaming frame before upstream closes (${mode})`, async () => {
      const baseUrl = mode === 'dev' ? devBaseUrl : previewBaseUrl;
      const transport = createGrpcWebTransport({
        baseUrl,
        useBinaryFormat: true,
      });
      const client = createClient(DeskBlotterService, transport);

      // 1. Binary unary RPC + credential header passthrough
      const unaryRes = await client.getAccountSnapshot(
        { accountId: `acct-${mode}` },
        { headers: { authorization: `Bearer token-${mode}` } },
      );
      expect(unaryRes.result.case).toBe('value');
      if (unaryRes.result.case === 'value') {
        expect(unaryRes.result.value.snapshot?.accountId).toBe(`acct-${mode}`);
        expect(unaryRes.result.value.snapshot?.snapshotSeq).toBe(77n);
      }
      expect(upstream.lastAuthHeaders).toContain(`Bearer token-${mode}`);

      // 2. Server-streaming first frame delivered while upstream keeps connection open
      upstream.resetStreamState();
      const abortController = new AbortController();
      const stream = client.subscribeEvents(
        { afterSeq: 10n },
        {
          signal: abortController.signal,
          headers: { 'x-ficant-credential': `cred-${mode}` },
        },
      );

      const iterator = stream[Symbol.asyncIterator]();
      const first = await iterator.next();
      expect(first.done).toBe(false);
      expect(first.value?.payload.case).toBe('heartbeat');
      if (first.value?.payload.case === 'heartbeat') {
        expect(first.value.payload.value.headSeq).toBe(42n);
        expect(first.value.payload.value.scannedThroughSeq).toBe(42n);
      }
      // Crucial assertion: upstream has NOT ended the HTTP response yet!
      expect(upstream.isStreamStillOpen()).toBe(true);

      // 3. Client cancellation propagates through Vite proxy to upstream server
      abortController.abort();
      await Promise.race([
        upstream.waitForStreamCancel(),
        new Promise<never>((_, reject) =>
          setTimeout(
            () => reject(new Error(`Upstream did not observe stream cancellation in ${mode} mode.`)),
            5000,
          ),
        ),
      ]);
      expect(upstream.isStreamStillOpen()).toBe(false);
    }, 20000);
  }
});
