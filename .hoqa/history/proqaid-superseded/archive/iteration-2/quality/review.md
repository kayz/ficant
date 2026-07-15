# Quality 评审：iteration-2 round-2a contract stage

## Verdict

**pass — 仅允许 Task 3 domain TDD dispatch。**

该 verdict 只覆盖 integration commit `591dfcaf46eb9fdc8a68d879edbc542dd9ded448` 的 Task 2 契约/生成阶段。它不代表 Migration、TypeScript consumer、W4、PostgreSQL/MinIO 业务闭环、Phase 0/1、发布或整体 readiness。

## 输入与边界

- `.proqaid/quality/inbox.iteration-2.round-2a.md`
- W2 报告 `.planning/iteration-2-workers/w2-contract-domain.md`
- 集成候选 `591dfcaf46eb9fdc8a68d879edbc542dd9ded448`
- main `42f570f309e20c867f65cffbce76e7f6d64d65d5`
- W2 Task review：Critical/Important/Minor 0，quality approved
- Interface round-2：contract pass

按 inbox 未运行 Compose。候选 descriptor/tree/provenance 无具体 mismatch，因此未重跑 BSR。

## 独立 fresh 验证

| 验证 | 环境/命令 | 观察 |
|---|---|---|
| 候选身份 | 宿主 Git `rev-parse HEAD`、前后 `status --porcelain` | SHA 精确为 `591dfcaf46eb9fdc8a68d879edbc542dd9ded448`；前后 clean |
| descriptor suite | `ficant-ubuntu-24.04` root；`CARGO_TARGET_DIR=/tmp/ficant-quality-round2a-target cargo test -p ficant-contract-tests --test descriptor_inventory` | 11 passed，0 failed |
| Buf | direct integration worktree `buf format --diff --exit-code interface`；`buf lint interface` | 均 exit 0 |
| Python consumer | Python 3.12.11、uv 0.7.13、WSL 内部 venv/cache；`uv run --locked pytest python/tests/test_contract_import.py` | 1 passed，0 failed |
| descriptor binding | `buf build interface --as-file-descriptor-set` + `sha256sum` | `d1832ff40a3057d9ae11c7e7dcc8c847efbf13c76f4e18a14f8d905be3fdf1d0`，与候选证据一致 |
| 禁止项扫描 | WSL 原生 `grep/find` | proto float/double 0；非生成 production parallel DTO 0；唯一 Phase 2 关键词只是 `rule.proto` 中明确禁止 duration/DV01 的注释，不是数值行为 |
| 生成路径 | commit blob proof tree | Rust 7、Python 13、TypeScript 13；只位于三个冻结生成根 |

首轮 WSL Git 身份检查因 linked worktree `.git` 使用 Windows 绝对路径而停止；第二次 WSL Git clean 检查又因跨平台 EOL 过滤把物理 checkout 误报为 modified。两次都在合同测试前停止。候选 SHA/clean 最终由宿主 Git前后绑定；WSL 直接读取同一 worktree 执行 Cargo/Buf/Python。该环境差异不计作产品 RED 或合同 finding。

## 合同覆盖审计

- 17 个精确对象：Instrument、Bond、FuturesContract、Cashflow、Calendar、Unit、Quote、Trade、Valuation、CurveSnapshot、MarketRulePack、DataSnapshot、UniverseSnapshot、ExperimentRun、Artifact、SignalSet、RunJournal。
- 六服务精确集合：MarketDefinitionService、MarketFactService、SnapshotService、ExperimentService、ArtifactService、PlatformService；测试拒绝额外第七个 `ficant.*` service。
- 三个查询服务精确 method/input/output，所有冻结服务保持 unary。
- PlatformService 精确七 RPC：GetAppRegistry、GetCurrentSession、RefreshSession、RevokeSession、AuthorizeAppLaunch、RefreshAppLaunch、RevokeAppLaunch；SafeError/ErrorCode、CSP、session、短期 AppLaunchGrant、撤销和 oneof/enum 数值均由 descriptor 精确断言。
- descriptor 递归禁止 float/double 和平行 canonical representation；非生成生产源码未发现 17 对象平行 DTO；未发现 Phase 2 数值 RPC/行为。

## exact-pin A/B、provenance 与路径

`buf.gen.yaml` 固定四个 remote plugin 的 version+revision：prost v0.5.0/r2、tonic v0.5.0/r4、Python v31.1/r2、ES v2.5.2/r1。W2 报告提供对应 upstream tag、版本和 Apache-2.0/BSD-3-Clause provenance；候选 pins 与记录逐项一致。

Quality 从 commit blob 重建 proof tree，使用记录中的规范化 SHA-256 算法复核：

| tree | files | SHA-256 |
|---|---:|---|
| Rust | 7 | `b5fc2cf200628d8f5c07e7fd7ca7e097333cc17d5ee416bde501ca6180b5998e` |
| Python | 13 | `4c3171300e12ce22d940051ef0a25d9f32b73119fa18135d57b799296b842d25` |
| TypeScript | 13 | `6c454fca55e90f6348b81ba471e0246dba40ce522404827ca5361a158522034e` |
| overall | 33 | `a74c23a823dfc8f20e9784bc11c8b1a2004a62749b3bed791153e1cd1feba146` |
| generation input | 15 | `987a8ee73d4a781e17e0cc83d2c39e77e0d15011a1980fd40bb3b39ec50e7fdb` |

这些值与 W2 fresh exact-pin A/B 记录一致；无 mismatch，因此没有再次访问 BSR。

## 精确 Q2 状态

| ID | 状态 | round-2a 判定 |
|---|---|---|
| `Q2-CTR-01` | `passed` | fresh Buf format/lint 均 exit 0 |
| `Q2-CTR-02` | `not-applicable-initial-contract-baseline` | main `42f570f…` 无 `interface/`，不得写 breaking PASS；未来 breaking 必须绑定 `.git#ref=591dfcaf46eb9fdc8a68d879edbc542dd9ded448,subdir=interface` |
| `Q2-CTR-03` | `collected/incomplete` | fresh exact-pin A/B/tree binding、Rust consumer 11/11、Python consumer 1/1 已证明；TypeScript consumer compilation 等 W4/Task10 |
| `Q2-CTR-04` | `passed` | 17 对象、六服务/签名、Platform security、禁止 float/parallel DTO/数值行为、额外服务回归均通过 |
| `Q2-P0-03` | `collected/incomplete` | locked Python 3.12 generated import 本轮通过；完整 base-image/reproducibility 仍待 Task10，不作 Phase 0 PASS |

## Findings

### Blocking

- 无 contract-stage blocking。

### Important

- `Q2-CTR-03` 必须保持 `collected/incomplete`，不得把生成 TypeScript tree 等同 consumer compilation；由 W4/Task10 补齐。
- 后续 breaking 必须使用精确 `591dfcaf46eb9fdc8a68d879edbc542dd9ded448` contract baseline，不得浮动到 branch/main。

### Note

- WSL Git 的 Windows linked-worktree/EOL 视图限制已隔离；宿主 Git提供候选身份/clean 证据。
- 目标 runtime 为 GPT-5.6 Terra/high；实际模型/reasoning 无 attestation，记 `unverified`。

## Dispatch 结论

Task 3 domain TDD 可以按已批准的互斥范围派发。该结论不授权扩大实现范围，也不关闭任何 Migration、业务闭环、Product P 项或 Phase 0/1 exit gate。

## 有效期

Valid: iteration-2 round-2a only
