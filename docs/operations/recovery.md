# FICANT 本地恢复证明

本页说明 R7B 的隔离恢复协议。它证明一份 PostgreSQL dump 与完整 Ceph RGW immutable-object 清单能够在销毁源状态后恢复到全新实例，并让一个 ResearchGraph Artifact 与一个同步 Analytics 正式输出通过 required-read 得到相同 bytes、evidence 和 identity。它不是生产备份服务，也不承诺 HA、PITR、RPO、RTO 或跨地域容灾。

## 入口与前置条件

恢复证明只在干净、已提交的公共 worktree 上运行：

```powershell
.\scripts\check-recovery.ps1 -ListOnly
.\scripts\check-recovery.ps1
```

需要 PowerShell 7、Git、锁定 Rust 工具链、Docker/Compose，以及本机可用的 `ficant/worker:dev` 镜像。调用者可通过 `FICANT_TEST_RUNTIME_IMAGE_DIGEST=sha256:<64-hex>` 传入已经核验的 Worker image config digest；未设置时脚本只从本地 `ficant/worker:dev` 读取实际 image ID。缺失、占位或非 canonical digest 会失败关闭。

脚本不接触开发、测试发布或生产数据库。每次运行生成两个名称受限的独立 Compose project：`ficant-r7b-source-<token>` 与 `ficant-r7b-restore-<token>`，各自拥有独立 PostgreSQL/Ceph volume、bucket、凭据和回环端口。备份仅暂存在系统临时目录下的 `ficant-recovery-*` 路径。

## 证明流程

1. 从 clean `HEAD` 读取 40 位 Git commit 与 tree，绑定实际 Worker image config digest。
2. 启动 source PostgreSQL 16 与 Ceph RGW，应用 migration，生成一个带完整 `FormalOutputEvidence` 的 Graph Artifact 和一个同步 Analytics `FormalOutputRecord`。
3. 以 PostgreSQL custom dump 备份全部关系；稳定枚举 bucket 中全部 `immutable/*` 对象，逐个重算 key、size 与 SHA-256，并导出 bytes。
4. 生成 `ficant.recovery.bundle.v1` 清单，绑定 Code、Runtime、PG dump、完整对象集合及两类输出身份。
5. 删除 source project 的容器、网络、PostgreSQL volume 与 Ceph volume，并机械确认没有残留的同 project 容器或卷。
6. 启动名字、端口、数据库卷和 bucket 都不同的 restore project；恢复 PG dump，并只按清单恢复经内容寻址验证的对象。
7. 重新枚举 destination bucket，要求对象集合与清单完全相等；随后通过 `FormalOutputRepository::get` 和 `VerifiedReadFacade::read_verified_artifact` required-read 两个输出，并逐字段/逐字节比较。

恢复不是 PostgreSQL 与 Ceph 的分布式事务。清单是两种存储之间的离线一致性边界：任何一侧无法完整验证时，整次恢复证明失败，不能返回部分成功。

## 清单合同

`backup-manifest.json` 固定包含：

- `code.git_commit_sha` 与 `code.git_tree_sha`；
- `runtime.image_config_digest`；
- PostgreSQL dump 的相对路径、字节数和 SHA-256；
- 按 key 严格排序且无重复的全部 immutable object：`immutable/<sha256>`、相对备份文件、字节数和 SHA-256；
- Graph Artifact ID、Graph output identity 与 Analytics output identity。

验证器拒绝绝对路径、`..`、反斜杠路径、重复/乱序对象、非内容寻址 key、缺失/附加文件、size/hash 漂移、非 canonical ULID，以及 Code/Runtime 身份漂移。快速门禁执行 1 个正例和 5 个独立负例：

```powershell
.\scripts\test-recovery-check.ps1
```

该 fixture 门禁只验证清单协议，不替代真实 source-destroy/fresh-restore；后者只由 `check-recovery.ps1` 提供。

## 失败处理与安全边界

- `HashMismatch` / manifest validation failure：备份集合不完整或发生漂移，禁止恢复和手工跳过对象。
- `StorageUnavailable`：当前无法判断完整性，应排查本地 Docker/Ceph 后重新执行整轮证明。
- required-read failure：即使 PG restore 成功，也视为恢复失败；不得把 metadata-only 查询当成功。
- 脚本的清理仅接受自身生成的精确 project 名与系统临时目录前缀；不会删除开发 Compose volume。
- 本入口不创建 tag、不推送镜像、不连接目标服务器，也不能充当版本 CI、发布或生产恢复演练证据。

private authority 的 `MANUAL.md` 由 `scripts/check-manual.ps1` 在 exact authority commit 和临时 clean public checkout 中读取；公共仓库不复制其文本。MANUAL 的 recovery marker 原样调用本入口。
