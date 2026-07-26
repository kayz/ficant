import { fromBinary } from "@bufbuild/protobuf";
import { ConnectError } from "@connectrpc/connect";
import { useEffect, useMemo, useRef, useState } from "react";
import type { DecimalValue } from "../../packages/contracts-generated/src/ficant/core/v1/common_pb";
import {
  AnalyzeBondResultSchema,
  RiskSummarySchema,
  type AnalyzeBondResult,
  type RiskSummary,
} from "../../packages/contracts-generated/src/ficant/rates/v1/analytics_pb";
import {
  RunState,
  type GetGraphRunResponse,
  type ListNodeOutputManifestsResponse,
  type ObservedNodeOutput,
  type ReadNodeOutputResponse,
  type TraceGraphOutputResponse,
} from "../../packages/contracts-generated/src/ficant/research/v1/experiment_pb";
import type { Phase5AObservationClient } from "./observation-client";

const ULID = /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/;
const ANALYSIS_TYPE = "ficant.rates.v1.analyze-bond-result";
const RISK_TYPE = "ficant.rates.v1.risk-summary";

interface RunObservation {
  graph: GetGraphRunResponse;
  manifests: ListNodeOutputManifestsResponse;
}

interface NodeObservation {
  trace: TraceGraphOutputResponse;
  output: ReadNodeOutputResponse;
}

export function Phase5AObserver({ client }: { client: Phase5AObservationClient }) {
  const [runId, setRunId] = useState("");
  const [run, setRun] = useState<RunObservation>();
  const [node, setNode] = useState<NodeObservation>();
  const [selectedNodeId, setSelectedNodeId] = useState("");
  const [status, setStatus] = useState<"idle" | "loading-run" | "loading-node">("idle");
  const [error, setError] = useState("");
  const request = useRef<AbortController | undefined>(undefined);

  useEffect(() => () => request.current?.abort(), []);

  async function loadRun() {
    const candidate = runId.trim().toUpperCase();
    if (!ULID.test(candidate)) {
      setError("Run ID 必须是规范的 26 位 ULID。");
      setRun(undefined);
      setNode(undefined);
      return;
    }
    request.current?.abort();
    const controller = new AbortController();
    request.current = controller;
    setStatus("loading-run");
    setError("");
    setNode(undefined);
    setSelectedNodeId("");
    try {
      const [graph, manifests] = await Promise.all([
        client.getGraphRun(candidate, controller.signal),
        client.listNodeOutputManifests(candidate, controller.signal),
      ]);
      if (!graph.graphRun) throw new Error("运行响应缺少 graph_run");
      setRun({ graph, manifests });
      setRunId(candidate);
    } catch (failure) {
      if (!controller.signal.aborted) {
        setRun(undefined);
        setError(safeError(failure));
      }
    } finally {
      if (!controller.signal.aborted) setStatus("idle");
    }
  }

  async function loadNode(nodeId: string) {
    const candidateRun = runId.trim().toUpperCase();
    if (!ULID.test(candidateRun) || !ULID.test(nodeId)) {
      setError("运行或节点标识不完整。");
      return;
    }
    request.current?.abort();
    const controller = new AbortController();
    request.current = controller;
    setStatus("loading-node");
    setError("");
    setSelectedNodeId(nodeId);
    try {
      const [trace, output] = await Promise.all([
        client.traceGraphOutput(candidateRun, nodeId, controller.signal),
        client.readNodeOutput(candidateRun, nodeId, controller.signal),
      ]);
      if (!trace.trace || !output.manifest || output.outputs.length === 0) {
        throw new Error("节点观测响应不完整");
      }
      setNode({ trace, output });
    } catch (failure) {
      if (!controller.signal.aborted) {
        setNode(undefined);
        setError(safeError(failure));
      }
    } finally {
      if (!controller.signal.aborted) setStatus("idle");
    }
  }

  const graphRun = run?.graph.graphRun;
  const identity = graphRun?.execution?.reproducibility;
  const nodeRows = useMemo(
    () => run?.manifests.manifests.flatMap((stored) => {
      const content = stored.manifest?.content;
      return content?.nodeId?.value
        ? [{
            nodeId: content.nodeId.value,
            attempt: stored.manifest?.attempt ?? 0,
            outputs: content.outputs.length,
            checkpoint: stored.checkpoint?.journalSequence ?? 0n,
          }]
        : [];
    }) ?? [],
    [run],
  );

  return (
    <section className="phase5a-observer" aria-labelledby="phase5a-observer-title">
      <div className="observer-warning">
        <div>
          <p className="eyebrow">PHASE 5A / TEMPORARY OBSERVABILITY</p>
          <h2 id="phase5a-observer-title">固收运行观测面板</h2>
        </div>
        <strong>非业务界面</strong>
      </div>
      <p className="observer-disclaimer">
        该页面只读取已持久化的运行、清单、血缘和经过完整性校验的节点输出。
        它不提供筛选、推荐、买卖信号、目标仓位或正式研究结论。
      </p>

      <form className="observer-query" onSubmit={(event) => { event.preventDefault(); void loadRun(); }}>
        <label htmlFor="phase5a-run-id">Experiment Run ID</label>
        <div>
          <input
            id="phase5a-run-id"
            value={runId}
            onChange={(event) => setRunId(event.target.value)}
            placeholder="01H00000000000000000000000"
            autoComplete="off"
            spellCheck={false}
          />
          <button className="primary-action" type="submit" disabled={status !== "idle"}>
            {status === "loading-run" ? "读取中" : "读取运行"}
          </button>
        </div>
      </form>

      {error ? <p className="observer-error" role="alert">{error}</p> : null}

      {graphRun ? (
        <>
          <dl className="observer-facts" role="group" aria-label="运行绑定">
            <Fact label="状态" value={runState(graphRun.run?.state)} />
            <Fact label="Run" value={graphRun.run?.experimentRunId?.value} mono />
            <Fact label="Graph" value={graphRun.graph?.graphId?.value} mono />
            <Fact label="DataSnapshot" value={graphRun.run?.dataSnapshot?.objectId?.value} mono />
            <Fact label="UniverseSnapshot" value={graphRun.run?.universeSnapshot?.objectId?.value} mono />
            <Fact label="Data hash" value={hex(identity?.dataSnapshotHash?.value)} mono />
            <Fact label="Universe hash" value={hex(identity?.universeSnapshotHash?.value)} mono />
            <Fact label="RulePack 数" value={String(identity?.rulePacks.length ?? 0)} />
            <Fact label="Seed" value={identity?.seed.toString()} mono />
            <Fact label="Runtime" value={hex(identity?.runtimeImageDigest?.value)} mono />
            <Fact label="Environment" value={hex(identity?.environmentDigest?.value)} mono />
          </dl>

          <div className="observer-bindings-grid">
            <section aria-labelledby="observer-rule-packs">
              <h3 id="observer-rule-packs">RulePack 绑定</h3>
              {identity?.rulePacks.length ? (
                <table>
                  <thead><tr><th>ID</th><th>版本</th><th>内容 hash</th></tr></thead>
                  <tbody>
                    {identity.rulePacks.map((binding) => (
                      <tr key={`${binding.rulePackId?.value}/${binding.version.toString()}`}>
                        <td><code>{binding.rulePackId?.value || "—"}</code></td>
                        <td>{binding.version.toString()}</td>
                        <td><code>{hex(binding.contentHash?.value)}</code></td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              ) : <p className="observer-empty">未持久化 RulePack 绑定。</p>}
            </section>
            <section aria-labelledby="observer-external-inputs">
              <h3 id="observer-external-inputs">外部输入绑定</h3>
              {identity?.externalInputs.length ? (
                <table>
                  <thead><tr><th>输入</th><th>类型</th><th>Artifact</th><th>内容 hash</th></tr></thead>
                  <tbody>
                    {identity.externalInputs.map((binding) => (
                      <tr key={binding.inputId}>
                        <td>{binding.inputId}</td>
                        <td><code>{binding.valueType?.typeId || "—"}</code></td>
                        <td><code>{binding.resolvedArtifact?.objectId?.value || "—"}</code></td>
                        <td><code>{hex(binding.contentHash?.value)}</code></td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              ) : <p className="observer-empty">该运行未声明外部输入。</p>}
            </section>
          </div>

          <div className="observer-nodes">
            <h3>持久化节点</h3>
            {nodeRows.length === 0 ? (
              <p className="observer-empty">当前运行尚无已 checkpoint 的节点输出。</p>
            ) : (
              <ul>
                {nodeRows.map((row) => (
                  <li key={row.nodeId}>
                    <button
                      type="button"
                      className="observer-node-button"
                      aria-pressed={selectedNodeId === row.nodeId}
                      disabled={status !== "idle"}
                      onClick={() => void loadNode(row.nodeId)}
                    >
                      <span>{row.nodeId}</span>
                      <small>attempt {row.attempt} · {row.outputs} output · journal {row.checkpoint.toString()}</small>
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </>
      ) : null}

      {node ? <NodeOutput observation={node} /> : null}
    </section>
  );
}

function NodeOutput({ observation }: { observation: NodeObservation }) {
  const trace = observation.trace.trace;
  return (
    <section className="observer-output" aria-labelledby="observer-output-title">
      <div className="observer-output-heading">
        <div>
          <p className="eyebrow">VERIFIED ARTIFACT PAYLOAD</p>
          <h3 id="observer-output-title">节点输出</h3>
        </div>
        <span>{trace?.manifests.length ?? 0} 级 manifest · {trace?.externalInputs.length ?? 0} 个外部输入</span>
      </div>
      {observation.output.outputs.map((output) => {
        const binding = observation.output.manifest?.content?.outputs
          .find((candidate) => candidate.portName === output.portName);
        return (
          <ObservedPayload
            key={output.portName}
            output={output}
            artifactId={binding?.artifact?.objectId?.value}
          />
        );
      })}
    </section>
  );
}

function ObservedPayload({
  output,
  artifactId,
}: {
  output: ObservedNodeOutput;
  artifactId?: string;
}) {
  const typeId = output.valueType?.typeId ?? "";
  let decoded: AnalyzeBondResult | RiskSummary | undefined;
  let decodeError = false;
  try {
    if (typeId === ANALYSIS_TYPE) decoded = fromBinary(AnalyzeBondResultSchema, output.payload);
    if (typeId === RISK_TYPE) decoded = fromBinary(RiskSummarySchema, output.payload);
  } catch {
    decodeError = true;
  }
  return (
    <article className="observer-payload">
      <header>
        <div>
          <strong>{output.portName}</strong>
          <small>{typeId || "未知类型"}</small>
          <small>Artifact {artifactId || "—"}</small>
        </div>
        <code>{hex(output.contentHash?.value)}</code>
      </header>
      {decodeError ? <p className="observer-error" role="alert">已知类型的 payload 无法按冻结 Protobuf 解码。</p> : null}
      {decoded?.$typeName === "ficant.rates.v1.AnalyzeBondResult"
        ? <BondResult value={decoded as AnalyzeBondResult} />
        : null}
      {decoded?.$typeName === "ficant.rates.v1.RiskSummary"
        ? <RiskResult value={decoded as RiskSummary} />
        : null}
      {!decoded && !decodeError ? (
        <p className="observer-empty">未知输出类型，仅展示经过验证的类型、hash 和 {output.payload.byteLength} 字节长度。</p>
      ) : null}
    </article>
  );
}

function BondResult({ value }: { value: AnalyzeBondResult }) {
  const measures = value.measures;
  return (
    <>
      <dl className="observer-measures">
        <Fact label="净价" value={decimal(measures?.cleanPrice)} />
        <Fact label="全价" value={decimal(measures?.dirtyPrice)} />
        <Fact label="应计利息" value={decimal(measures?.accruedInterest)} />
        <Fact label="YTM" value={decimal(measures?.yieldToMaturity)} />
        <Fact label="Macaulay 久期" value={decimal(measures?.macaulayDuration)} />
        <Fact label="修正久期" value={decimal(measures?.modifiedDuration)} />
        <Fact label="凸性" value={decimal(measures?.convexity)} />
        <Fact label="DV01" value={decimal(measures?.dv01)} />
      </dl>
      <div className="observer-cashflows">
        <h4>派生现金流 · {value.cashflows.length}</h4>
        <table>
          <thead><tr><th>序号</th><th>名义日</th><th>支付日</th><th>票息</th><th>本金</th><th>合计</th></tr></thead>
          <tbody>
            {value.cashflows.map((cashflow) => (
              <tr key={cashflow.sequence}>
                <td>{cashflow.sequence}</td>
                <td>{cashflow.nominalDate}</td>
                <td>{cashflow.paymentDate}</td>
                <td>{decimal(cashflow.coupon)}</td>
                <td>{decimal(cashflow.principal)}</td>
                <td>{decimal(cashflow.total)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="observer-engine">
        engine {value.metadata?.engineId || "—"} / {value.metadata?.engineVersion || "—"} ·
        algorithm {value.metadata?.algorithm?.algorithmId || "—"} v{value.metadata?.algorithm?.algorithmVersion ?? 0}
      </p>
    </>
  );
}

function RiskResult({ value }: { value: RiskSummary }) {
  return (
    <dl className="observer-measures">
      <Fact label="修正久期" value={decimal(value.modifiedDuration)} />
      <Fact label="凸性" value={decimal(value.convexity)} />
      <Fact label="DV01" value={decimal(value.dv01)} />
      <Fact label="来源算法" value={value.sourceMetadata?.algorithm?.algorithmId} />
    </dl>
  );
}

function Fact({ label, value, mono = false }: { label: string; value?: string; mono?: boolean }) {
  return <div><dt>{label}</dt><dd className={mono ? "mono" : undefined}>{value || "—"}</dd></div>;
}

function decimal(value?: DecimalValue): string {
  if (!value) return "—";
  const negative = value.coefficient.startsWith("-");
  const digits = negative ? value.coefficient.slice(1) : value.coefficient;
  if (!/^\d+$/.test(digits)) return "INVALID";
  const scale = Number(value.scale);
  const padded = digits.padStart(scale + 1, "0");
  const point = scale === 0
    ? padded
    : `${padded.slice(0, -scale)}.${padded.slice(-scale)}`;
  return `${negative ? "-" : ""}${point}`;
}

function hex(value?: Uint8Array): string {
  if (!value || value.byteLength === 0) return "—";
  return [...value].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

function runState(value?: RunState): string {
  switch (value) {
    case RunState.CREATED: return "CREATED";
    case RunState.RUNNING: return "RUNNING";
    case RunState.SUCCEEDED: return "SUCCEEDED";
    case RunState.FAILED: return "FAILED";
    case RunState.CANCELLED: return "CANCELLED";
    default: return "UNSPECIFIED";
  }
}

function safeError(failure: unknown): string {
  if (failure instanceof Error && failure.name === "AbortError") return "";
  const connect = ConnectError.from(failure);
  if (connect.rawMessage) return `读取失败（${connect.code}）：${connect.rawMessage}`;
  return "读取失败：服务未返回可安全展示的错误信息。";
}
