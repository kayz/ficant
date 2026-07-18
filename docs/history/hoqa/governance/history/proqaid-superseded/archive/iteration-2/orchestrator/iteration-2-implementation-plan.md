# ficant iteration-2 Phase 0 + Phase 1 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: 使用 `subagent-driven-development` 按任务执行；所有行为遵守 TDD，步骤使用 `- [ ]` 跟踪。生产 worker 只能写分配的 worktree/文件范围。

**Goal：** 建立可复现的 Phase 0 技术/契约基线，并实现 Phase 1 领域内核，使真实 PostgreSQL/MinIO 业务闭环、版本/不可变/血缘和重放通过自动化验收。

**Architecture：** 分层模块化单体。根 `interface/` 是唯一 Protobuf 源；纯 Rust domain/application/runtime 与 PostgreSQL/MinIO storage 分离；`web-dm` 只消费生成 gRPC-Web client。四个 worker 按 W1 → W2 →（W3 ∥ W4）→ W1 集成。

**Tech Stack：** Ubuntu 24.04 x86_64、Rust Edition 2024、Protobuf/Buf/tonic、PostgreSQL 16/SQLx、MinIO、Docker Compose、Python 3.12/uv、Clang 18/CMake/Ninja、Node 22/pnpm/React/Vite/Playwright。

## 全局约束

- 当前 checkout 的旧 `master` 只读不推送；所有开发从验证后的 `main` 建 integration worktree。
- Phase 2 数值算法、完整 DMQuant、Phase 9 SignalSet 发布审批和 `TargetExposure` 不在本轮。
- `Valuation`、`CurveSnapshot`、来源 `Cashflow` 只保存输入事实；C++ 不得提供常数/fake/stub 数值结果。
- 根 `interface/` 是唯一后台/Protobuf 契约源；Web/Python/Rust 不手写平行跨边界 DTO。
- `docs/` 自然语言输出中文；页面设计在 `web-dm/webapps/<app-id>/design.md`，后台契约说明在 `interface/README.md`。
- Quality 的 `Q2-*` ID、真实容器禁令、红灯/同命令绿灯和证据字段属于所有 worker 的隐含要求。
- 工具版本是冻结提案而非事实：W1 必须在 Ubuntu 验证可获得性、校验和、OCI digest、许可证和锁兼容；失败时停止并路由，不静默升级。
- 验证采用混合方式：Rust、Python、C++、Node、单元测试及大部分业务测试优先在本地 WSL 高频执行；Docker/Compose 只在阶段验收执行一次容器专属检查（非 root、只读根、端口绑定、权限/资源限制、固定 project label 与零残留清理）。本机 Ubuntu 26.04 WSL 仅作快速开发环境，不得替代 Ubuntu 24.04 最终兼容性证据；在专用 Ubuntu 24.04 WSL 可用前，最终证据继续使用已固定 digest 的 Ubuntu 24.04 容器。
- 唯一开发环境启动命令是 `docker compose -f deploy/dev/docker-compose.yml --project-name ficant-iteration-2 --profile dev up --build --wait`；配置、exec、ps、测试与清理命令必须使用同一 Compose 文件和 project name，不得另立根 Compose 入口或省略 profile。
- 不访问测试机或 `C:\git\key`；不推镜像、不打 release tag、不执行远端 Migration。
- 不残留测试专用生产入口、假实现、硬编码成功数据、一次性脚本、未使用 mock 或未解决占位。

## 文件与 worker 所有权

| Worker | 独占写入 | 明确禁止 |
|---|---|---|
| W1 | 根 Workspace/lock/toolchain、根 `Cargo.toml` 的 `[workspace.dependencies]` 与 `Cargo.lock`、`.github/`、`deploy/`、`binaries/`、`crates/ficant-api/`、`cpp/`、`python/`（但排除 W2 的精确生成目录）、`.gitignore`、最终共享集成 | `interface/`、W2/W3 自有 crate 的成员 manifest/业务源文件、W2 生成目录、`web-dm/` 其余内容 |
| W2 | `interface/`、`crates/ficant-contracts/`、`crates/ficant-contract-tests/`、`crates/ficant-domain/`、`ficant-application/`、`ficant-runtime/`、对应 contract/domain tests、`python/node-contracts/src/ficant_contracts/generated/`、`python/tests/test_contract_import.py`、`web-dm/packages/contracts-generated/` | storage/migrations、上述精确生成目录/文件之外的 `python/`/`web-dm/`、共享 lock/Compose/CI |
| W3 | `crates/ficant-storage/`、`ficant-acceptance/`、`migrations/postgresql/`、`tests/golden-cases/` | 契约源、domain 语义、`web-dm/`、共享 lock/CI |
| W4 | `web-dm/`，但排除 `web-dm/packages/contracts-generated/` | `interface/`、全部 Rust crates、Migration、根 lock/CI、W2 生成契约目录 |
| Orchestrator | checklist、工具约束、角色 inbox、当前中文 docs 合并、deviations、cleanup | 直接编写生产行为 |

## Worker worktree、分支、报告与清理合同

integration worktree 固定为 `.worktrees/iteration-2`，分支固定为 `iteration-2/phase0-phase1`；它只由 Orchestrator 用于合并已经复核的 worker commit，任何 worker 不直接写 integration worktree。

| 派发 | 创建时点/基线 | 独立 branch | 独立 worktree | 临时报告路径 |
|---|---|---|---|---|
| W1 bootstrap | integration 建立后 | `iteration-2/w1-bootstrap` | `.worktrees/iteration-2-w1-bootstrap` | `.planning/iteration-2-workers/w1-bootstrap.md` |
| W1 lock for W2 | W2 scaffold commit 合入 integration 后 | `iteration-2/w1-lock-w2` | `.worktrees/iteration-2-w1-lock-w2` | `.planning/iteration-2-workers/w1-lock-w2.md` |
| W2 contract/domain | W1 commit 合入 integration 后 | `iteration-2/w2-contract-domain` | `.worktrees/iteration-2-w2-contract-domain` | `.planning/iteration-2-workers/w2-contract-domain.md` |
| W1 lock for W3 | W3 scaffold commit 合入 integration 后 | `iteration-2/w1-lock-w3` | `.worktrees/iteration-2-w1-lock-w3` | `.planning/iteration-2-workers/w1-lock-w3.md` |
| W3 storage/acceptance | W2 commit 合入 integration 后 | `iteration-2/w3-storage-acceptance` | `.worktrees/iteration-2-w3-storage-acceptance` | `.planning/iteration-2-workers/w3-storage-acceptance.md` |
| W4 web | W2 commit 合入 integration 后，可与 W3 并行 | `iteration-2/w4-web` | `.worktrees/iteration-2-w4-web` | `.planning/iteration-2-workers/w4-web.md` |
| W1 integration | W3/W4 commit 均合入 integration 后 | `iteration-2/w1-integration` | `.worktrees/iteration-2-w1-integration` | `.planning/iteration-2-workers/w1-integration.md` |

每个 worker prompt 必须逐项列出 checklist/Q2 ID、上述写入/禁止范围、首个有效红灯命令、同命令绿灯、回归命令和报告路径。报告必须记录 branch/worktree、base/HEAD SHA、变更路径、测试 ID、red/green argv/cwd/exit code/输出摘要、fixture/hash、未解决偏差与自清理状态；worker 不得只给口头成功结论。

Orchestrator 仅在 standing role/Quality 复核后把明确 commit 合入 integration。每次合入后由 W1 处理唯一 shared lock/CI/Compose 变化；不得让其他 worker 越界解决冲突。临时报告中的持久事实合并进现有 Quality/Delivery 文档后删除报告。清理时先用 `git worktree list --porcelain` 核对上述绝对路径均位于仓库的 `.worktrees/` 下，再对逐个字面路径运行 `git worktree remove`；仅对 `git branch --merged iteration-2/phase0-phase1` 证明已合入的上述精确分支执行删除。禁止全局 prune、通配递归删除或清理未知 worktree/branch。

Cargo 采用串行 manifest/lock handoff：W1 初始根 Workspace 只注册当前真实存在的 `binaries/*`，因为 Cargo 1.96 会把尚无匹配成员的 `crates/*` 判为 manifest error；禁止用空 crate/占位目录规避。W2 scaffold 合入后，首个 W1 lock checkpoint 才把 `crates/*` 加入根 Workspace，并在同一 W1 commit 中同步更新 `deploy/dev/RustService.Dockerfile` 复制真实 `crates/`，随后用唯一 Compose 命令验证容器构建。W1 独占根 `Cargo.toml` 的依赖版本与 `Cargo.lock`；W2/W3 可在各自 crate 内写成员 manifest，但第三方依赖只能写成 `{ workspace = true }`，并在报告中提交精确版本申请，不得改 root/lock。Orchestrator 先合入 scaffold，随后派 W1 在专用 lock checkpoint worktree固定根 `[workspace.dependencies]` 与 lock，执行 `cargo metadata --locked` 后合入；原 worker 只可 `--ff-only` 同步更新后的 integration，再运行有效行为红灯和绿灯。lock 尚未刷新产生的解析/下载失败不计入 `Q2-TDD-*`。后续新增依赖必须重复 checkpoint，禁止 worker 用未提交 lock、`--offline` 偶然缓存或越界修改制造绿色。

## Phase 0 精确验收 ID

| ID | 关闭条件 | 首要 owner/计划位置 |
|---|---|---|
| `Q2-P0-01` | 唯一 Compose 命令完成 PostgreSQL/MinIO/bucket-init/Migration/Rust readiness 启动 DAG | W1 Task 1 建合同，W3 Task 6 接通，Task 10 最终重建验证 |
| `Q2-P0-02` | 固定 Rust toolchain/lock 下 Workspace、binaries、tests 可重放 | W1 Task 1/lock checkpoints/Task 10 |
| `Q2-P0-03` | Python 3.12/uv lock/base image 可重放，生成契约真实导入 | W1 Task 1 + W2 Task 2 + Task 10 |
| `Q2-P0-04` | Clang 18/CMake/Ninja 固定，C ABI build/CTest 可重放且无伪业务 | W1 Task 1/Task 10 |
| `Q2-P0-05` | Node/pnpm lock 下 Platform Shell typecheck/build/Vitest/Playwright 可重放 | W4 Task 8 + W1 Task 10 |
| `Q2-P0-06` | 工具 SHA-256、OCI digest、许可证、action/image pin 与锁兼容有 Ubuntu 证据 | W1 Task 1/Task 10，Delivery 复核 |
| `Q2-P0-07` | MinIO bucket-init 幂等、命名/content-addressing、重启持久性和隔离清理通过 | W3 Task 6/7 + Task 10 |
| `Q2-P0-08` | baseline/final repo-policy、十项 CI、allowlist/deny、SBOM/漏洞/secret/reproducibility gates 通过 | W1 Task 1/Task 10 |

任何报告不得只写 `Q2-P0-*`；必须列出上述精确 ID。Task 1 只能关闭其真实执行到的子项，依赖 W2/W3/W4 的 ID 保持 `collected/incomplete`，直到 Task 10 和 Quality round-4 在干净环境复核。

## Product P-01..P-07 验收映射

| Product ID | 必须联动的 Quality/计划证据 | 首要关闭位置 |
|---|---|---|
| `P-01` | `Q2-BIZ-01`、`Q2-OBJ-01..17`；真实事实→Snapshot→Run→Journal→Artifact/最小 SignalSet | W3 Task 7，Quality round-3/4 |
| `P-02` | `Q2-BIZ-01/02`、`Q2-INV-06/07`；相同 fixture/version 的规范化查询、完整反向血缘与摘要一致 | W3 Task 7，Quality round-4 |
| `P-03` | `Q2-BIZ-01/02`、`Q2-INV-08/09`、`Q2-P0-01/07`；服务重启后 DB/对象引用/Journal/重放一致 | W3 Task 6/7，W1 Task 10，Quality round-4 |
| `P-04` | `Q2-INV-03/04/11`；历史 version/Snapshot/Artifact/SignalSet/Journal 禁止原位覆盖或篡改 | W2 Task 3/4、W3 Task 6/7，Quality round-3/4 |
| `P-05` | `Q2-INV-01/02/06/07/08/12`；非法单位/生效时间/hash/血缘/Journal/规则日期返回稳定业务错误 | W2 Task 3/4、W3 Task 7，Quality round-3/4 |
| `P-06` | `Q2-INV-05/08/10`、`Q2-MIG-04`；重复请求幂等、并发同版本/sequence 有可识别冲突且不静默覆盖 | W2 Task 4、W3 Task 6/7，Quality round-3/4 |
| `P-07` | `Q2-GOV-01..04`、`Q2-P0-08`、Product final recheck；README/中文 docs/UI/接口索引只写实际证据，Phase 2–9/完整 DMQuant 保持未实现 | W1 Task 10、Task 11 Product/Review |

worker 报告必须同时列适用的 `P-*` 与 `Q2-*`，Quality final 必须逐项给 P-01..P-07 verdict；不能用技术测试总体绿色代替 Product 验收，`P-07` 也不能在最终文档事实复核前关闭。

---

### Task 1：建立 integration worktree 与 W1 Phase 0 基线

**Owner：** Orchestrator 创建 worktree；W1 写入。

**Files：**

- Create: `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`
- Create: `binaries/ficant-server/`, `binaries/ficant-worker/`, `binaries/ficant-web/`
- Create: `deploy/dev/docker-compose.yml`, `deploy/dev/config/ficant.toml`
- Create: `.github/workflows/ci.yml`, `.github/scripts/verify-repo-policy.sh`
- Create: `cpp/fixed-income-kernel/CMakeLists.txt`, `cpp/fixed-income-kernel/include/ficant_kernel.h`, `cpp/fixed-income-kernel/src/abi_version.cpp`, `cpp/fixed-income-kernel/tests/abi_smoke.cpp`
- Create: `python/pyproject.toml`, `python/uv.lock`, `python/node-runtime/Dockerfile`
- Modify: `.gitignore`

**Produces：** 可供 W2/W3/W4 使用的 Workspace glob、工具、Compose、初始锁和 CI 合同；新增 crate 的成员 manifest/lock 仍按串行 handoff 更新。

- [ ] **Step 1：建立安全工作区**

Run:

```bash
git fetch origin main
git rev-parse main
git rev-parse origin/main
git worktree add .worktrees/iteration-2 -b iteration-2/phase0-phase1 origin/main
git worktree add .worktrees/iteration-2-w1-bootstrap -b iteration-2/w1-bootstrap iteration-2/phase0-phase1
```

Expected: `main` 与 `origin/main` 相同；integration 与 W1 bootstrap worktree clean 且路径/branch 与合同一致；旧 `master` 不变化。若网络失败或 ref 不同，停止并记录，不猜测基线。

- [ ] **Step 2：先写 repo-policy 红灯**

`verify-repo-policy.sh` 提供 `--stage baseline` 与 `--stage final` 两个严格集合。两者都检查禁止后台语言、禁止根 `proto/`、禁止 tracked `.proqaid/.codex/.claude/hidden/UI-DM`、secrets 和已存在文件的中文/锁规则；`baseline` 只要求 W1 所有路径，`final` 还要求 W2/W3/W4 的全部 Phase 0/1 根。首次 `baseline` 必须因 W1 Workspace/目录缺失失败，而不是脚本语法错误；这不是放宽最终门。

Run: `bash .github/scripts/verify-repo-policy.sh --stage baseline`

Expected: FAIL，并列出尚未建立的 Phase 0 路径。

- [ ] **Step 3：验证并固定工具版本**

在 Ubuntu 24.04 运行 `rustc --version --verbose`、`python --version`、`uv --version`、`clang++ --version`、`cmake --version`、`ninja --version`、`node --version`、`pnpm --version`、`buf --version`、`docker compose version`。将实际版本、下载 SHA-256/OCI digest 和许可证写入 lock/config/Delivery 证据，对应 `Q2-P0-06`。任何提案版本不可用时停止路由。

- [ ] **Step 4：建立最小可构建基线**

Rust binaries 只提供真实健康/readiness 入口；C++ 只导出 `uint32_t ficant_kernel_abi_version(void)` 并由 CTest 验证返回已定义 ABI 版本；Python 镜像只验证固定解释器/依赖/生成契约可导入，不伪造节点业务。

- [ ] **Step 5：使 W1 基线变绿并保留最终门红灯**

运行 `bash .github/scripts/verify-repo-policy.sh --stage baseline`、W1 Rust binaries build、Python runtime/image build 和 C++ CTest，分别记录 `Q2-P0-08` baseline、`Q2-P0-02`、`Q2-P0-03` 的 runtime 部分、`Q2-P0-04`。Expected: W1 所有基线真实 PASS；`bash .github/scripts/verify-repo-policy.sh --stage final` 必须仍因尚未由 W2/W3/W4 创建的 `interface/`、领域/存储和 `web-dm/` 路径 FAIL，并准确列出缺项。`Q2-P0-01/05/07` 与 `Q2-P0-08` final 保持 incomplete。Web 构建只在 Task 8 由 W4 变绿，三语言契约只在 Task 2 变绿，完整四类构建和 final policy 只在 Task 10 变绿；不得用空目录、占位包或跳过检查提前制造绿色。

- [ ] **Step 6：提交 W1 基线**

```bash
git add Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml clippy.toml binaries deploy .github cpp python .gitignore
git commit -m "build: establish reproducible phase 0 baseline"
```

### Task 2：W2 唯一契约与生成防漂移

**Files：**

- Create: `interface/buf.yaml`, `interface/buf.gen.yaml`, `interface/proto/ficant/core/v1/{common,error}.proto`
- Create: `interface/proto/ficant/market/v1/{instrument,definition,fact,rule}.proto`
- Create: `interface/proto/ficant/research/v1/{snapshot,experiment,artifact,signal,journal}.proto`
- Create: `interface/proto/ficant/app/v1/{registry,session}.proto`
- Create: `interface/tests/descriptor_inventory_test.rs`
- Create: `crates/ficant-contracts/Cargo.toml`, `crates/ficant-contracts/src/lib.rs`
- Create: `crates/ficant-contract-tests/Cargo.toml`, `crates/ficant-contract-tests/tests/descriptor_inventory.rs`
- Create: `python/tests/test_contract_import.py`
- Generate: `crates/ficant-contracts/src/generated/`, `python/node-contracts/src/ficant_contracts/generated/`, `web-dm/packages/contracts-generated/src/`

**Produces：** 17 对象唯一契约、共享 error/identity/version/decimal/time/lineage、App Registry/Session 契约。

- [ ] **Step 0：crate scaffold 与 lock handoff**

W2 先提交 `ficant-contracts`、`ficant-contract-tests`、domain/application/runtime 的成员 manifest、空测试 target 与精确依赖申请，不提交行为实现、不改 root/lock。Orchestrator 合入后派 W1 lock checkpoint；W2 对更新后的 integration 执行 `git merge --ff-only iteration-2/phase0-phase1`，并先验证 `cargo metadata --locked`。只有此后测试到达目标断言的失败才是有效红灯。

该 W1 lock checkpoint 还必须同步把真实 `crates/` 加入 `RustService.Dockerfile` build context，并运行同一 Compose 启动命令；root Workspace、lock 与容器 build context 任一不同步都不得放行 W2 行为实现。

- [ ] **Step 1：写 descriptor inventory 红灯**

测试解析 descriptor set，要求 17 个对象全存在、package 固定为 `ficant.<area>.v1`、跨对象字段只引用共享类型、错误含 `code/message/trace_id`、TypeScript 不存在手写替代类型。首次运行因消息缺失失败。

Run: `cargo test -p ficant-contract-tests --test descriptor_inventory`

Expected: FAIL，列出缺失消息，而不是找不到 test target。

- [ ] **Step 2：实现最小 Protobuf 合同**

共享字段语义冻结为：ULID 使用受约束 string message 并验证 26 字符规范格式；`DecimalValue` 唯一表示为 `coefficient(string) + scale(uint32) + UnitRef`，系数规范化且首版精度上限/PostgreSQL `numeric` 映射由契约与属性测试固定，禁止 `units/nanos`、裸 decimal string 或浮点平行表示；时间使用 protobuf Timestamp + `market_timezone`；版本、content hash、owner/tenant、lineage refs 显式存在。禁止在 Web/Python/Rust 另定义跨边界结构。

- [ ] **Step 3：运行 contract gates**

```bash
buf format --diff --exit-code interface
buf lint interface
buf generate interface --template interface/buf.gen.yaml
git diff --exit-code -- crates/ficant-contracts/src/generated python/node-contracts/src/ficant_contracts/generated web-dm/packages/contracts-generated/src
cargo test -p ficant-contract-tests --test descriptor_inventory
uv run pytest python/tests/test_contract_import.py
```

`main` 是首次发布基线且不存在 `interface/`。W2 必须以 `git rev-parse main` 和 `git cat-file -e main:interface/buf.yaml` 的预期缺失证据，把 `Q2-CTR-02` 记为 `not-applicable-initial-contract-baseline`，不得把缺少比较基线记为 PASS 或静默跳过。W2 commit 合入 integration 后记录精确 `CONTRACT_BASE_SHA` 与 descriptor hash；本轮此后的每次契约变化以及 Task 10/CI 都使用 pinned Buf 支持的 exact-ref Git input 对该 SHA 运行 breaking，禁止改用浮动 `main`/branch。

W2 在本任务证明 Rust/Python/TypeScript 三类生成物存在且重复生成 tree 无漂移，并使 Rust/Python consumer 编译/导入通过。由于 pnpm workspace/package/lock 由 W4 独占且尚未创建，TypeScript consumer compilation 在此时保持 `collected/incomplete`，不是 PASS；由 W4 在 Task 8 编译，再由 Task 10/Quality round-4 连同 exact-ref breaking 关闭 `Q2-CTR-03`。

W2 是三个生成目录的唯一写入者；W1/W4 只能在各自分支读取已集成的生成物。生成命令不得改写根 lock、Compose、Python 其他目录或 WebApp 代码；若生成器必须改共享文件，停止并交由 W1 串行集成。

Expected: PASS；三语言生成物可编译；重复 generate 不产生 diff。

- [ ] **Step 4：提交契约**

```bash
git add interface crates/ficant-contracts crates/ficant-contract-tests python/node-contracts/src/ficant_contracts/generated web-dm/packages/contracts-generated/src
git commit -m "feat: define phase 1 protobuf contracts"
```

### Task 3：W2 领域原语与 17 个对象（TDD）

**Files：**

- Create: `crates/ficant-domain/src/primitives/{id,version,decimal,time,content_hash,lineage}.rs`
- Create: `crates/ficant-domain/src/market/{instrument,bond,futures_contract,cashflow,calendar,unit,quote,trade,valuation,curve_snapshot,market_rule_pack}.rs`
- Create: `crates/ficant-domain/src/research/{data_snapshot,universe_snapshot,experiment_run,artifact,signal_set,run_journal}.rs`
- Create: `crates/ficant-domain/tests/{object_contracts,domain_properties,negative_invariants}.rs`

**Interfaces：**

对象与文件一一对应：`Instrument`→`instrument.rs`、`Bond`→`bond.rs`、`FuturesContract`→`futures_contract.rs`、`Cashflow`→`cashflow.rs`、`Calendar`→`calendar.rs`、`Unit`→`unit.rs`、`Quote`→`quote.rs`、`Trade`→`trade.rs`、`Valuation`→`valuation.rs`、`CurveSnapshot`→`curve_snapshot.rs`、`MarketRulePack`→`market_rule_pack.rs`、`DataSnapshot`→`data_snapshot.rs`、`UniverseSnapshot`→`universe_snapshot.rs`、`ExperimentRun`→`experiment_run.rs`、`Artifact`→`artifact.rs`、`SignalSet`→`signal_set.rs`、`RunJournal`→`run_journal.rs`。

```rust
pub trait VersionedDefinition { fn identity(&self) -> &str; fn version(&self) -> u64; }
pub trait ContentAddressed { fn content_hash(&self) -> &ContentHash; }
pub trait Lineaged { fn lineage(&self) -> &[LineageRef]; }

pub enum DomainErrorCode {
    InvalidId, InvalidUnit, InvalidEffectiveTime, VersionConflict,
    ContentHashMismatch, BrokenLineage, InvalidStateTransition,
    JournalSequenceConflict,
}
```

Repository 端口不得退化为统一 CRUD；按 Definition、Fact、Snapshot、Run、Journal、Artifact、SignalSet 语义分别定义 create/get/query/append/publish。

- [ ] **Step 1：按 `Q2-OBJ-01..17` 写失败测试**

每个对象测试合法构造、至少一个非法构造、身份/版本/时间/单位/来源/血缘字段。`Valuation/CurveSnapshot/Cashflow` 测试只验证输入事实；不得调用数值计算。

- [ ] **Step 2：写 `Q2-INV-01..04` 属性红灯**

使用 `proptest` 生成非法单位、时区/生效时间、历史覆盖和快照漂移场景。首次运行必须因不变量未实现失败。

- [ ] **Step 3：实现最小 domain 行为**

每个文件只定义一个清晰对象/聚合；构造函数返回稳定 `DomainErrorCode`；发布对象不提供通用 setter/update/delete；版本变化创建新值。

- [ ] **Step 4：同命令变绿并回归**

```bash
cargo nextest run -p ficant-domain --test object_contracts --test domain_properties --test negative_invariants
cargo test -p ficant-domain --doc
```

- [ ] **Step 5：提交领域模型**

```bash
git add crates/ficant-domain
git commit -m "feat: implement phase 1 domain invariants"
```

### Task 4：W2 application/runtime 端口、状态与重放

**Files：**

- Create: `crates/ficant-application/src/ports/{definitions,facts,snapshots,runs,journal,artifacts,signals,blob_store}.rs`
- Create: `crates/ficant-application/src/use_cases/phase1_business_loop.rs`
- Create: `crates/ficant-runtime/src/{journal,replay,digest}.rs`
- Create: `crates/ficant-runtime/tests/{journal_ordering,replay_determinism}.rs`

- [ ] **Step 1：写 Journal 顺序/并发/重放红灯**

测试相同 run 的 sequence 只允许连续追加；重复幂等键返回原事件；并发同 sequence 只有一个成功；规范化重放摘要相同。

- [ ] **Step 2：实现端口和纯内核状态机**

application 用例依赖 ports，不依赖 SQLx/MinIO。runtime 只处理事件顺序和摘要，不访问数据库。

- [ ] **Step 3：运行**

`cargo nextest run -p ficant-runtime --test journal_ordering --test replay_determinism`

Expected: PASS，并覆盖 `Q2-INV-08` 与 `Q2-TDD-*` 证据。

- [ ] **Step 4：提交**

```bash
git add crates/ficant-application crates/ficant-runtime
git commit -m "feat: add phase 1 application ports and replay runtime"
```

### Task 5：Quality round-2 契约/Migration 设计门

在 W2 契约和 W3 首批 Migration 可执行后，Orchestrator 创建 Quality round-2 inbox。Quality 独立运行 `Q2-CTR-*`、`Q2-MIG-01..03`，核对红绿证据、生成防漂移和对象清单；其中 `Q2-CTR-02` 首次基线只能按有 SHA/路径缺失证据的 `not-applicable-initial-contract-baseline` 记录，`Q2-CTR-03` 在 TypeScript consumer 尚未编译时保持 incomplete。Blocking/important finding 未关闭前 W3/W4 不进入最终集成。

### Task 6：W3 PostgreSQL/MinIO 与 Migration（TDD）

**Files：**

- Create first: `crates/ficant-storage/Cargo.toml`, `crates/ficant-acceptance/Cargo.toml`（依赖申请经 W1 lock checkpoint 固定）
- Create: `migrations/postgresql/0001_primitives.sql`, `0002_market_definitions.sql`, `0003_market_facts.sql`, `0004_research_assets.sql`, `0005_run_journal.sql`, `0006_indexes.sql`
- Create: `crates/ficant-storage/src/postgres/{definitions,facts,snapshots,runs,journal,artifacts,signals}.rs`
- Create: `crates/ficant-storage/src/minio/{staging,content_addressed,orphan_cleanup}.rs`
- Create: `crates/ficant-storage/tests/{migration_acceptance,postgres_repository,minio_object_store,concurrency}.rs`

- [ ] **Step 0：storage scaffold 与 lock handoff**

W3 先提交两个成员 manifest、空测试 target 和精确依赖申请；Orchestrator 合入后派 W1 lock checkpoint。W3 `--ff-only` 同步 integration 并通过 `cargo metadata --locked` 后，才进入有效 TDD 红灯。W3 全程不得改 root/lock。

- [ ] **Step 1：写空库/升级/失败原子性红灯**

测试新 PostgreSQL 16 空库、已有 schema fixture、重复 migrate、注入失败后无半表/半数据。首次因 Migration 缺失失败。

- [ ] **Step 2：实现前向 Migration 与约束**

依次建立 primitives、definitions、facts、assets、journal、indexes。使用唯一/外键/check/expected revision 约束；禁止 extension、触发器领域逻辑、存储过程和通用 down。

- [ ] **Step 3：写真实 Repository/MinIO 红灯**

覆盖 `Q2-OBJ-*` create/get/query、同版本并发、幂等、staging/hash/promote/metadata、DB 失败 orphan、已有引用保护。

- [ ] **Step 4：实现 adapters 并运行**

使用 Quality round-1 的 `sqlx migrate`、`postgres_repository`、`minio_object_store` 命令，并记录 `Q2-P0-01` 的依赖 DAG 部分与 `Q2-P0-07`。Expected: PostgreSQL/MinIO 真实容器通过；服务重启后仍可读取。

- [ ] **Step 5：提交**

```bash
git add migrations/postgresql crates/ficant-storage
git commit -m "feat: persist phase 1 objects in postgres and minio"
```

### Task 7：W3 真实业务闭环与负向验收

**Files：**

- Create: `tests/golden-cases/china-rates/phase1-business-loop.json`
- Create: `crates/ficant-acceptance/tests/{phase1_business_loop,negative_invariants}.rs`

- [x] **Step 1：写 `Q2-BIZ-01/02` 红灯**

fixture 包含国债、期货、日历、单位、来源 Cashflow/Quote/Trade/Valuation/CurveSnapshot/RulePack。测试按 P-01 创建 Snapshot→Run→Journal→Artifact/SignalSet→反向血缘，因 use case/adapters 未接通失败。

- [x] **Step 2：写剩余负向红灯**

覆盖 hash 不符、断裂血缘、Journal 乱序/并发、MinIO 中断、同版本竞争、发布对象篡改、错误规则生效日。

- [x] **Step 3：接通真实 use case**

不得在测试中绕过 API/use case 直接插表制造成功。规范化重放摘要排除 ULID/时间等非确定字段，固定 fixture hash 和随机种子。

- [x] **Step 4：运行、重启、重复环境验证**

在 Ubuntu 24.04、真实 PostgreSQL/MinIO 上执行唯一 Quality wave acceptance；重新连接持久化服务后通过四类 required read 重读正式内容，并用相同 fixture 完成两次确定性 production replay。Compose 专项仍只属于 Task 10/最终 Delivery 门，不在本业务波次重复运行。

- [x] **Step 5：提交**

```bash
git add tests/golden-cases crates/ficant-acceptance
git commit -m "test: prove phase 1 business lineage loop"
```

Closure：业务 SHA `dbcff34793e79e73ed63872e28ed6298feedfbc4` 上的唯一 Quality wave 为 14/14、exit 0；Quality 证据提交 `3dfe71b6ffe671317f97ef689c17fa5de7145d2f`。Delivery 与 Review 均为 `PASS — C0 / I0 / M0`，`Q2-INV-11`、Task 7 与 Phase 0/1 语义闭环已关闭。Task 10 与 iteration exit 保持未关闭。

### Task 8：W4 Platform Shell 与多 WebApp 边界

**Files：**

- Create: `web-dm/package.json`, `pnpm-lock.yaml`, workspace config
- Create: `web-dm/platform-shell/src/{app,registry,session,error,loader}.tsx`
- Create: `web-dm/platform-shell/tests/{states,permissions,accessibility}.test.tsx`
- Create: `web-dm/platform-shell/e2e/platform-shell.spec.ts`
- Preserve: `web-dm/webapps/dmquant/design.md` 仅作为中文页面设计事实源；本轮不创建 `app.yaml`、可加载包或假 DMQuant 注册项，直到后续真实 WebApp 实施轮次。

- [ ] **Step 1：写 Shell 状态/权限/a11y 红灯**

覆盖 registry 空/加载/错误、测试 fixture app 的授权/加载/拒绝/失败、会话过期、错误 code/trace、键盘/焦点/live region/iframe title/200% 缩放/reduced motion。fixture 仅位于测试目录，不能写入 `web-dm/webapps/dmquant/` 或被生产构建注册。

- [ ] **Step 2：写真实 gRPC-Web 红灯**

Playwright 指向 Compose Rust 服务，使用生成 TypeScript client。禁止 route fulfill/mock response 伪造成功。

- [ ] **Step 3：实现最小 Shell**

只实现宿主/registry/session/error/loader；验证 origin/CSP/sandbox/短期 App Token；主 token 不入 URL/localStorage。

- [ ] **Step 4：运行**

执行 Quality round-1 pnpm/Vitest/Playwright 命令和 axe 检查，记录 `Q2-P0-05`。Expected: `Q2-WEB-01`、`03`、`04` 以及 TypeScript consumer compilation PASS；`Q2-WEB-02` 必须到达真实 Rust 服务并因 App Registry/session gRPC-Web 行为尚未实现而形成有效红灯。连接失败、端口失败或 mock 成功都无效。`Q2-WEB-02` 只在 Task 10 的 W1 API/server wiring 后使用同一 Playwright 命令变绿。

- [ ] **Step 5：提交**

```bash
git add web-dm
git commit -m "feat: add platform shell and multi-webapp boundary"
```

### Task 9：Quality round-3 领域/真实存储门

W3 纵向切片完成后，Quality 运行全部 `Q2-OBJ-*`、`Q2-INV-01..10`、`Q2-MIG-04`，抽查 PostgreSQL 行、MinIO 对象、重启持久性、反向血缘和重放摘要。任何 mock 替代、历史覆盖、断裂血缘发布或摘要不一致为 blocking。

### Task 10：W1 串行集成、CI、allowlist 与中文文档

**Files：** 根 Workspace/locks/Compose/CI、`crates/ficant-api/`、`binaries/ficant-server/` wiring、`.gitignore`、README、当前中文 docs、`docs/adr/template.md`。

- [x] 集成 W2 后再集成 W3/W4，解决 shared dependency/lock 只由 W1 写。
- [x] 在不改 W4 测试的前提下先运行 `Q2-WEB-02` 同一 Playwright 命令并确认真实服务返回未实现行为；实现 `crates/ficant-api/src/{registry,session,error,grpc_web}.rs`、对应 Rust integration tests 与 `ficant-server` wiring，只经 application ports 暴露 Registry/session/error mapping，不创建 DMQuant manifest 或独立后台；随后用同一命令变绿。
- [x] 使用 W2 合入时记录的精确 `CONTRACT_BASE_SHA` 运行 Buf breaking，并编译 Rust/Python/TypeScript 三个 consumer；关闭 `Q2-CTR-03` 与初始基线后的兼容性证据，禁止用浮动 branch 或当前 commit 自比。
- [x] 扩展 GitHub allowlist 到 checklist 精确根目录，并用 deny scan 确认 PROQAID/工具约束/hidden/旧 UI-DM/secrets/worker 资料不在 tree。
- [x] 原位更新中文 Product/Architecture/Quality/Delivery/Review 文档；不创建平行报告。
- [x] CI 显示十项独立 gates，固定 action/image SHA，生成 SBOM/许可证/漏洞/敏感扫描和 reproducibility 证据。
- [x] 在干净 integration commit 执行全部 Quality 命令，逐项关闭 `Q2-P0-01..08`；任何失败回到对应 owner，不在集成层放宽断言。
- [x] 提交：最终 integration 为 `a8a3847c1c8d92e5a1ef4c02b9e692f07ea4da13`，发布树为 `ec826ac928546aa996d10ef6ebf7d10813d685c9`。

Closure：完整 Ubuntu 24.04 CI run `29193249268` 在单提交候选 `ef96c5edea11b0d5f6ebc693501f40a9b40df061` / tree `2d1fa3a1be11e563c486d7c67df349ec06faf4d0` 上十项全绿。随后仅两份 Quality/Delivery 中文证据文档收口，形成候选 `07a0104b99c361a0ac945e6eceb69db8f90b09fd` / tree `ec826ac928546aa996d10ef6ebf7d10813d685c9`；该 docs-only 候选在固定 Ubuntu 24.04 上 fresh 通过 repo-policy 与 authoritative Supply：620=607+13、secret base/range/tree=0、唯一 `async-std 1.13.2` 结果按 D-026 标记 `accepted-unfixed`。完整七服务 Compose runtime、重启持久性与零残留清理已通过；Task 10 与交付闭环关闭。

### Task 11：Quality round-4、角色复核、Review 与清理

- [x] Quality round-4 在唯一责任门下形成完整 `Q2-*`、四类构建、Web、供应链、真实业务与可重放证据，并由 Delivery 单独完成完整 Compose 专项；中文证据索引已更新。
- [x] Product/Architecture/Interface/Delivery 对实际实现做退出复核；README 与中文权威文档只声明已被证据证明的行为。
- [x] Review 退出审计由 D-027 的明确用户授权跳过；状态为 `Review skipped by explicit human authorization`，既有 Review 证据继续有效，不伪造 Review PASS。
- [x] Orchestrator 已路由全部确定性 blocking/important finding；D-026 为用户接受的 `accepted-unfixed`，D-027 为用户批准的 Review deviation。
- [x] 清理 worker worktree/branch、缓存、构建产物、Compose 资源、测试数据和草稿；iteration-2 `.proqaid` 产物及只读 `.superpowers/` 历史资料已归档。最终 `main` 为 `737807302351fe8feee425a89d666caf3d611f96`，CI run `29194877792` 十项全绿。

## 计划自检合同

- Phase 0 十项交付、四项退出条件与 Phase 1 17 个对象必须在任务/测试映射中各有唯一 owner。
- `Q2-P0/CTR/MIG/OBJ/INV/BIZ/WEB/TDD` 均必须出现在 worker prompt 和最终证据。
- 文件路径、package/test target 或工具版本变化必须先路由 Architecture/Delivery/Quality，并更新本计划；worker 不得静默改名。

## Validity

Valid: iteration-2 only
