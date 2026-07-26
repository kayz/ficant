import { create, toBinary } from "@bufbuild/protobuf";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import {
  AnalyzeBondResultSchema,
} from "../../packages/contracts-generated/src/ficant/rates/v1/analytics_pb";
import {
  GetGraphRunResponseSchema,
  ListNodeOutputManifestsResponseSchema,
  ReadNodeOutputResponseSchema,
  RunState,
  TraceGraphOutputResponseSchema,
} from "../../packages/contracts-generated/src/ficant/research/v1/experiment_pb";
import type { Phase5AObservationClient } from "../src/observation-client";
import { PlatformShell } from "../src/app";
import { Phase5AObserver } from "../src/phase5a-observer";
import { fixtureClient, registry } from "./fixtures/platform";

const RUN_ID = "01H00000000000000000000000";
const NODE_ID = "01H00000000000000000000001";
const HASH = Uint8Array.from({ length: 32 }, (_, index) => index);

function decimal(coefficient: string, scale: number) {
  return { coefficient, scale };
}

function observationClient(
  overrides: Partial<Phase5AObservationClient> = {},
): Phase5AObservationClient {
  const manifest = {
    attempt: 1,
    content: {
      nodeId: { value: NODE_ID },
      outputs: [{
        portName: "analysis",
        valueType: { typeId: "ficant.rates.v1.analyze-bond-result", typeVersion: 1n },
        artifact: { objectId: { value: "01H00000000000000000000005" } },
        contentHash: { value: HASH },
      }],
    },
  };
  const analysis = create(AnalyzeBondResultSchema, {
    cashflows: [{
      sequence: 1,
      nominalDate: "2026-08-01",
      paymentDate: "2026-08-03",
      coupon: decimal("175", 2),
      principal: decimal("10000", 2),
      total: decimal("10175", 2),
    }],
    measures: {
      accruedInterest: decimal("42", 2),
      cleanPrice: decimal("99875", 3),
      dirtyPrice: decimal("100295", 3),
      yieldToMaturity: decimal("21875", 6),
      macaulayDuration: decimal("4875", 3),
      modifiedDuration: decimal("4760", 3),
      convexity: decimal("2850", 2),
      dv01: decimal("476", 3),
    },
    metadata: {
      engineId: "ficant-rates",
      engineVersion: "0.1.0",
      algorithm: { algorithmId: "bond-analytics", algorithmVersion: 1 },
    },
  });
  return {
    getGraphRun: async () => create(GetGraphRunResponseSchema, {
      graphRun: {
        run: {
          experimentRunId: { value: RUN_ID },
          dataSnapshot: { objectId: { value: "01H00000000000000000000002" } },
          universeSnapshot: { objectId: { value: "01H00000000000000000000006" } },
          state: RunState.SUCCEEDED,
        },
        graph: { graphId: { value: "01H00000000000000000000003" } },
        execution: {
          reproducibility: {
            dataSnapshotHash: { value: HASH },
            universeSnapshotHash: { value: HASH },
            rulePacks: [{
              rulePackId: { value: "01H00000000000000000000004" },
              version: 1n,
              contentHash: { value: HASH },
            }],
            externalInputs: [{
              inputId: "bond-request",
              valueType: {
                typeId: "ficant.rates.v1.analyze-bond-request",
                typeVersion: 1n,
              },
              resolvedArtifact: { objectId: { value: "01H00000000000000000000007" } },
              contentHash: { value: HASH },
            }],
            seed: 42n,
            runtimeImageDigest: { value: HASH },
            environmentDigest: { value: HASH },
          },
        },
      },
    }),
    listNodeOutputManifests: async () => create(ListNodeOutputManifestsResponseSchema, {
      manifests: [{ manifest, checkpoint: { journalSequence: 7n } }],
    }),
    traceGraphOutput: async () => create(TraceGraphOutputResponseSchema, {
      trace: { manifests: [{ manifest }], externalInputs: [] },
    }),
    readNodeOutput: async () => create(ReadNodeOutputResponseSchema, {
      manifest,
      outputs: [{
        portName: "analysis",
        valueType: { typeId: "ficant.rates.v1.analyze-bond-result", typeVersion: 1n },
        contentHash: { value: HASH },
        payload: toBinary(AnalyzeBondResultSchema, analysis),
      }],
    }),
    ...overrides,
  };
}

async function loadNode(client: Phase5AObservationClient) {
  render(<Phase5AObserver client={client} />);
  fireEvent.change(screen.getByLabelText("Experiment Run ID"), { target: { value: RUN_ID } });
  fireEvent.click(screen.getByRole("button", { name: "读取运行" }));
  const nodeButton = await screen.findByRole("button", { name: new RegExp(NODE_ID) });
  fireEvent.click(nodeButton);
  return screen.findByRole("heading", { name: "节点输出" });
}

describe("Phase 5A 临时运行观测面板", () => {
  it("仅在 Platform Shell 完成会话与目录边界后装配", async () => {
    render(
      <PlatformShell
        client={fixtureClient({ getAppRegistry: async () => registry([]) })}
        observationClient={observationClient()}
      />,
    );
    expect(await screen.findByRole("heading", { name: "当前没有可用应用" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "固收运行观测面板" })).toBeInTheDocument();
  });

  it("明确标识非业务用途且不呈现交易动作", () => {
    render(<Phase5AObserver client={observationClient()} />);
    expect(screen.getByText("非业务界面")).toBeInTheDocument();
    expect(screen.getByText(/不提供筛选、推荐、买卖信号、目标仓位/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /买入|卖出|发布|推荐/ })).not.toBeInTheDocument();
  });

  it("在本地拒绝非规范 ULID，不向后端发送请求", async () => {
    const getGraphRun = vi.fn(observationClient().getGraphRun);
    render(<Phase5AObserver client={observationClient({ getGraphRun })} />);
    fireEvent.change(screen.getByLabelText("Experiment Run ID"), { target: { value: "not-a-run" } });
    fireEvent.click(screen.getByRole("button", { name: "读取运行" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("规范的 26 位 ULID");
    expect(getGraphRun).not.toHaveBeenCalled();
  });

  it("只通过真实观测客户端展示运行绑定、血缘和冻结数值 payload", async () => {
    await loadNode(observationClient());
    const bindings = screen.getByRole("group", { name: "运行绑定" });
    expect(within(bindings).getByText("SUCCEEDED")).toBeInTheDocument();
    expect(within(bindings).getByText(RUN_ID)).toBeInTheDocument();
    expect(within(bindings).getByText("42")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "RulePack 绑定" })).toBeInTheDocument();
    expect(screen.getByText("01H00000000000000000000004")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "外部输入绑定" })).toBeInTheDocument();
    expect(screen.getByText("01H00000000000000000000007")).toBeInTheDocument();
    expect(screen.getByText(/Artifact 01H00000000000000000000005/)).toBeInTheDocument();
    expect(screen.getByText("99.875")).toBeInTheDocument();
    expect(screen.getByText("0.476")).toBeInTheDocument();
    expect(screen.getByText("2026-08-03")).toBeInTheDocument();
    expect(screen.getByText(/engine ficant-rates/)).toBeInTheDocument();
  });

  it("已知类型 payload 解码失败时失败关闭，不伪造数值", async () => {
    const base = observationClient();
    await loadNode(observationClient({
      readNodeOutput: async (...args) => {
        const response = await base.readNodeOutput(...args);
        response.outputs[0]!.payload = Uint8Array.of(0xff, 0xff);
        return response;
      },
    }));
    expect(await screen.findByRole("alert")).toHaveTextContent("无法按冻结 Protobuf 解码");
    expect(screen.queryByText("99.875")).not.toBeInTheDocument();
  });

  it("后端返回不完整清单时不显示节点输出", async () => {
    const client = observationClient({
      readNodeOutput: async () => create(ReadNodeOutputResponseSchema, { outputs: [] }),
    });
    render(<Phase5AObserver client={client} />);
    fireEvent.change(screen.getByLabelText("Experiment Run ID"), { target: { value: RUN_ID } });
    fireEvent.click(screen.getByRole("button", { name: "读取运行" }));
    fireEvent.click(await screen.findByRole("button", { name: new RegExp(NODE_ID) }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("节点观测响应不完整"));
    expect(screen.queryByRole("heading", { name: "节点输出" })).not.toBeInTheDocument();
  });
});
