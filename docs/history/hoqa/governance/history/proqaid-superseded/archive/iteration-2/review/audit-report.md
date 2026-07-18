# Process Audit Report：iteration-2 最终退出审计

## 最终退出裁决

**BLOCK — C0 / I1 / M0**

审计对象为 integration `a8a3847c1c8d92e5a1ef4c02b9e692f07ea4da13` / tree
`ec826ac928546aa996d10ef6ebf7d10813d685c9`，以及从可信基线
`42f570f309e20c867f65cffbce76e7f6d64d65d5` 生成的单提交发布候选
`07a0104b99c361a0ac945e6eceb69db8f90b09fd`。候选 parent 精确等于可信基线、范围提交数为 1，tree
精确等于最终 integration tree。

Ubuntu 24.04 完整 CI run `29193249268` 在候选 `ef96c5edea11b0d5f6ebc693501f40a9b40df061` /
tree `2d1fa3a1be11e563c486d7c67df349ec06faf4d0` 上 10/10 jobs success；随后 docs-only 候选
`07a0104...` 在固定 Ubuntu 24.04 上 fresh repo-policy 与 authoritative Supply 均 exit 0。Supply 为
620=607+13、secret base/range/tree=0、accepted-unfixed=1，provenance 精确绑定。Delivery 的七服务
Compose 最终 PASS 绑定 `87db3897...` / tree `e8fb65c...`；相关配置 blob 到最终树未变化，运行时安全、
重启持久性与 cleanup 0 均有效。

存在一项开放 Important finding：README 顶部仍把当前状态写成“尚无生产源码、可运行系统或已验证产品
行为”，同时声明自身为当前唯一系统技术基线；这与同一树中的已验证 Phase 0 / Phase 1 实现及中文
Product/Quality/Delivery 文档直接冲突，也未标为历史快照。须由 Orchestrator 路由 README 所有者原位
纠正，形成新的 clean tree/单提交候选，fresh 运行 docs-only repo-policy 与 authoritative Supply 后再审。

唯一已接受但未修复项是 D-026：
`RUSTSEC-2025-0052` / `pkg:cargo/async-std@1.13.2` 经 `pkg:cargo/minio@0.4.0` 可达，状态继续为
`accepted-unfixed`。必须在 iteration-3 入口或首次外部发布前（较早者）重新评估；不得自动继承或续期。
历史 BLOCK/失败 runs 只作修复追溯，已由当前精确候选证据取代，不是开放 finding。

README finding 关闭前禁止 GitHub `main` fast-forward、iteration-2 归档或关闭。之后的 PROQAID 归档以及
worktree/branch 清理仍属于 Orchestrator 后置动作。本 Review 未重跑测试、CI 或 Compose。

## 历史 focused Review 记录

## 结论

**PASS — C0 / I0 / M0**

Final license refresh `9f044b796a912746df2080c5d42bf696797c4424` 精确基于
`73384fec150c1929a2e28f79549ad21c4ec8bc57`，只修改 tracked inventory 单文件。run
`29192844481` 的已完成 jobs 中唯一失败为 Supply，原因是 `ficant-node-runtime` first-party source binding
随最终 Python/Delivery tree 变化。620=607+13 与全部 keys/策略不变，唯一 package value 更新为该包的
source integrity。actual final archive + fixed Syft1.46 再生 byte-identical，候选 **PASS — C0 / I0 / M0**。

Optional env candidate `87db3897d82b0bea4e35eee3595178f366bbf041` 精确基于
`af0197c299a7baf9a92f4fe129e9849bdca89601`，只修改 Compose 三个 bootstrap mappings 与对应 gate/tests。
旧 `${VAR:-}` 在宿主未设置时仍向容器注入空字符串，Rust 正确作为 `Some("")` 拒绝；candidate 使用
Compose null pass-through，使 unset 时 Config.Env 真正缺席、configured 时精确透传。显式空值、scope-only
与单边身份仍 fail-closed。真实 unset/configured ficant-server 均 healthy，候选 **PASS — C0 / I0 / M0**。

MinIO volume candidate `af0197c299a7baf9a92f4fe129e9849bdca89601` 精确基于
`911edeaf1cc58f15d72ed37900c495ee28e93438`。官方固定 base 的 `Config.User` 为空、声明
`VOLUME /data`，且 `/data` 为 `0:0 0755`；fresh volume 在 UID/GID 1000 下真实写入失败。candidate
用最小派生镜像在 build layer 固定 `/data=1000:1000 0700`，最终 `USER 1000:1000`，Compose 其余
read-only/cap/resource/port 安全合同不变。真实 empty-volume write、MinIO health、身份与清理门禁均 PASS，
候选 **PASS — C0 / I0 / M0**。

License refresh candidate `4a8fd93731f1e3000e1d9c76092bf6fe2215d3d4` 精确基于
`b7599edc83ff0acd75c77bbd75d2d45609def29e`。GitHub run `29190374218` 的 Supply 真实失败是 reviewed
Rust test 改变 `ficant-bootstrap` release-tree 后，tracked first-party source binding 未同步。candidate
保持 620=607+13 精确 key universe，唯一 package 字段变化为 `ficant-bootstrap.source_integrity`；generator
v2 进一步把全部 first-party source bindings 纳入 input digest。actual candidate archive + fixed Syft 1.46
机械再生与 tracked inventory byte-identical，专属门禁全 PASS。候选 **PASS — C0 / I0 / M0**。

Web race candidate `36f6f0877d68da0b745ce8cf36aeef18674419ee` 精确基于
`b7599edc83ff0acd75c77bbd75d2d45609def29e`，只修改 iframe loader 与既有 states test。GitHub run
`29190374218` 的 Web 真实失败为 `fireEvent.error` 后找不到 alert，DOM 仍是 `app-ready`。根因是原生
iframe error listener 在 passive `useEffect` 才安装，存在 commit 后、passive effects 前的丢事件窗；
candidate 只前移到 `useLayoutEffect`。确定性 TDD 用例在旧 loader 精确 spy=0 RED、候选 PASS；原错误
状态语义与 29 个 Vitest、typecheck 通过。候选 **PASS — C0 / I0 / M0**，可进入最终 squash。

Rust deadline candidate `eeacb00ac2250fd67790ae69a149191a2d280cbb` 精确基于
`3745c169fa8043eb8f4c5ddea9eaeb6d5db08379`，只修改 `ficant-bootstrap` 的 test module，production
deadline/read/write 代码不变。GitHub run `29189731911` 的旧测试在 response read 精确收到 Linux
`ConnectionReset (os error 104)`；candidate 仅把 test helper 的终止分类为 CleanEof 与 PeerReset，前者
仍要求完整 408，后者只允许期望 408 的精确前缀，无关 I/O error 继续失败。server join 与 350ms 上限
保持强制。Linux 固定镜像中 helper tests、slow-drip 10 次与 bootstrap lib 全部通过，候选
**PASS — C0 / I0 / M0**，可进入最终 squash。

License authority successor `1ab894aff269c29712ebe7afbd1e435d0f40371b` 精确基于
`4d1b6ee65096464af4da35e582af7285284b7e03`，只修改 supply gate 与既有 gate/risk fixtures。
raw Syft/SBOM 继续完整保留，但 Syft 仅提供精确的 crates.io/PyPI/npm package-key universe；许可证、
source integrity、SPDX、scoped exception 与 first-party 判定均由 tracked inventory 唯一授权。完整
`verify-license-inventory.py verify` 在 evidence 验收入口执行，随后 provenance 精确绑定 inventory
digest/file SHA/generator，不存在 digest-only shortcut。Ubuntu 24.04 指定门禁与风险负例全部通过，
候选 **PASS — C0 / I0 / M0**，可进入最终 squash。

Repro Node candidate `68ec891f137d3d48c10f2256a1521f83ba914680` 精确基于
`0fab0cc9792019dd883c0707ab54d5c0a1078c29`，只修改 CI workflow 与 repo-policy fixture。
GitHub run `29189267286` 的 Repro 真实失败确认为 runner Node `v22.23.1` 与项目精确要求 `22.17.0`
不符。candidate 在任何 Web/pnpm 操作前使用 Contract 同一官方 Node 22.17.0 URL/size/SHA，依次完成
size/hash、extract、PATH/GITHUB_PATH、exact version 与 Corepack pnpm 10.12.4 激活；Contract job 合同
保持不变。候选 **PASS — C0 / I0 / M0**，可进入最终 squash。

Supply secret successor `4d1b6ee65096464af4da35e582af7285284b7e03` 精确基于
`0fab0cc9792019dd883c0707ab54d5c0a1078c29`，只修改 release-topology fixture 一个文件。
旧 GitHub run `29189267286` 的真实 artifact 复核为 base 0 / range 3 / tree 3；successor 从 tracked
archive 移除可识别 secret literal，改为运行时机械拼接，并显式证明 base-history/range/tree 临时仓库
各精确命中 1 个 `generic-api-key`。以 trusted base `42f570f` 和 successor tree 构造的单提交候选三层
fresh scan 均为 0。D-026 final 与 repo-policy 既有 PASS 无回退，候选可进入最终 squash。

D-026 final successor `0fab0cc9792019dd883c0707ab54d5c0a1078c29` 以
`fe336948038a0a6fcf1eb8c831e965c9e93589df` 为 overall base，包含完整六提交 license chain。
前次 LF provenance I1 已关闭：candidate archive 的 Cargo/uv/pnpm hashes、header 与 input digest 精确
匹配，tracked inventory 机械重算 byte-identical，fixed Syft 1.46 实扫为 620 unique/0 duplicate=
607 third-party+13 first-party，candidate/tree runtime provenance 通过。repo-policy 只精确放行三个新增
Python gate tool 路径，完整 fixtures 与 final stage 均 PASS。候选可进入最终 squash/发布候选验证。

D-026 successor `f33c5229357d26cbd0ace035258fa18b8b46a771` 精确基于
`dfa33aca4cc2846a52807eb7740a91af6b599000`。前次 Syft scope/duplicate 与 fixture executable
问题已关闭：production 无 exclude 的 fixed Syft 1.46 scan 现为 620 unique/0 duplicate，完整 gate
fixtures 通过。但 tracked inventory 的 Cargo/uv lock SHA 来自 Windows CRLF 工作树，而 production
`git archive` 是规范 LF 字节，导致 lock header 与 `input_tree_digest` 漂移；实际 inventory verify 仍
fail-closed。因此 successor 当前不可集成。

D-026 license closure candidate `dfa33aca4cc2846a52807eb7740a91af6b599000` 以
`fe336948038a0a6fcf1eb8c831e965c9e93589df` 为 base，包含 `8a71d8b`、`c9a2a7f`、`dfa33ac`
三提交，当前不可集成。SPDX、scoped exception、NOTICE、accepted-unfixed 与 Cargo reachability 设计
均通过 focused 复核；但 production 同命令的实际 Syft 1.46 scan 得到 624 unique keys（并有两组
duplicate），与 tracked 620-package inventory 不符；此外完整 gate fixture 因新增 risk fixture 未提交
可执行位而在 Ubuntu 24.04 报 Permission denied。

此前 Supply topology successor、LLVM successor 与 Contract Node 的 PASS 保持有效。

Supply topology successor `87eaabb2d40c53c9006cf0273472573ca969cd45` 精确基于
`2f8ce723ff882fac4622f20ce7d82054efde2336`，已关闭前次 I1。D-025 将 trusted base 冻结为
`42f570f309e20c867f65cffbce76e7f6d64d65d5`，只接受以该 base 为唯一 parent、范围内精确一个提交的
squashed candidate，并分别扫描 base 完整历史、base..candidate 与 candidate release tree。
当前多提交迭代分支在任何工具获取前被拒绝是预期行为；本 verdict 批准 successor 进入最终 squash，
不把 `87eaabb` 本身声明为可发布 candidate。

Supply recovery candidate `2f8ce723ff882fac4622f20ce7d82054efde2336` 精确基于
`75c4fe043b564d39a997c76c74c395d581f91697`，当前不可集成。pytest 9.0.3、candidate/lock ledger、
空结果的已执行语义与 Rust fake-secret fixture 修正均成立；但 Gitleaks history 从 `--all` 收窄为
`-1 HEAD`，在没有已证明 clean 的 trusted base 或发布历史策略时，只能证明当前树和候选单提交，
无法排除祖先历史中的 secret 泄漏。

LLVM runner successor 的既有 PASS 保持有效。

LLVM runner successor `e9b146edd4e4d966fa9c84c91aeabe5ad3b43952` 精确基于
`5992c2d2fd8ff049781ccc4e0a46f1dcb35e793d`，并与 overall base `75c4fe0` 形成两提交完整候选。
前次 I1 已关闭：生产 `--install` 以不可覆盖的字面路径 `/etc/os-release` 严格绑定
`ID=ubuntu`、`VERSION_ID=24.04`、`VERSION_CODENAME=noble`，并同时复核 lock runner；该检查位于
任何下载、卸载或安装之前。两提交候选可在 `75c4fe0` 后串行集成。

Contract Node candidate `ead0806c6274f24aef9261d13958088b0426d165` 精确基于
`75c4fe043b564d39a997c76c74c395d581f91697`，可 fast-forward 集成。Contract job 使用官方固定
Node 22.17.0 linux-x64 artifact，按 retry→size/hash→extract→PATH/version→Corepack/pnpm 10.12.4
顺序建立工具链；Rust job 不再重复运行 contract crate，而 Contract gate 仍独占其 11 个测试。

LLVM runner recovery candidate `5992c2d2fd8ff049781ccc4e0a46f1dcb35e793d` 精确基于
`75c4fe043b564d39a997c76c74c395d581f91697`，当前不可集成。精确 preinstalled conflict、reverse
dependency、audit、dry-run/remove/install 顺序均 fail-closed；但 `runner = ubuntu-24.04` 仅作为 lock
metadata 比较，生产脚本未验证实际宿主 OS，违反受控卸载“仅 Ubuntu 24.04”边界。

CI syntax recovery candidate `75c4fe043b564d39a997c76c74c395d581f91697` 精确基于
`926a6401ab78124cede9d971e82dc73d5a17e87e`，可 fast-forward 集成。唯一语义变化是把
`FICANT_GATE_OUTPUT_DIR` 从不允许使用 `runner` context 的 job env 下移到 supply verification step env；
upload path、`always()`、missing-file fail-closed 与 redacted evidence 语义保持不变。

Dependency successor `26fdb388a9b0d62afa9d99a914b0dcfe2b3db1bb` 精确基于前候选
`0f011d6353abca5e27c7fdcb3dcdbb84cf910d36`，并以 iteration base `91f088f` 形成五文件完整范围。
前次 I1 已关闭：platform-shell 直接声明 Vite 6.4.3/Vitest 3.2.6，root ignored overrides 已移除，
exact pnpm 10.12.4 删除后重生成 byte-identical lock。候选可进入 GitHub Ubuntu 24 外部验证；在该环境
完成 frozen install/typecheck/Vitest 前，不批准最终集成或 iteration exit。

Dependency upgrade candidate `0f011d6353abca5e27c7fdcb3dcdbb84cf910d36` 精确基于
`91f088fc52ba9313e21ca19fb5c758b955fe9ff1`，当前不可集成。固定 OSV 快照确认四个目标升级已清零，
Python contract 与当前 frozen Web lock 的 typecheck/Vitest 通过；但 pnpm 10.12.4 明确忽略候选新增的
root `pnpm.overrides`，而 platform-shell 仍声明旧 Vite/Vitest，因此安全修复不可从权威 manifest 重建。

LLVM Tool-First candidate `6ef07607a20b25b8ffc51bfbfed58034ad73269c` 精确基于
`91f088fc52ba9313e21ca19fb5c758b955fe9ff1`，可 fast-forward 集成。固定 Noble Packages.gz、
精确六包闭包、系统依赖/冲突、逐 deb 验证及 dry-run→单次 install 路径均 fail-closed；C++ 与
Repro 共用同一安装脚本。

CIRecovery2 rebase candidate `91f088fc52ba9313e21ca19fb5c758b955fe9ff1` 精确基于当前 integration
`2967fcbd21b9ceaa8039846633104ce0c3b74869`，可 fast-forward 集成；旧 `8155255` 不可集成。
Web、Rust 与 supply-chain CI 所有权及 evidence upload 语义已恢复，focused fixture、final policy、
Bash 与 YAML 结构检查全部通过。

ContractRecovery candidate `2967fcbd21b9ceaa8039846633104ce0c3b74869` 精确基于
`b93255767c92e73f28206f3af0910032b6b15d26`，可 fast-forward 集成。descriptor 构建现已明确使用
Buf `--as-file-descriptor-set`，恢复权威 `d1832ff...`/59848 证据格式；未改契约、Proto 或生成树。

W10-GateRecovery successor `b93255767c92e73f28206f3af0910032b6b15d26` 精确基于
`7ad13a8321db7cf4a255738925b0df905f5fce34`，已关闭剩余 I1。business-loop block 内 provenance
注释与精确 digest export 唯一且相邻，Docker 只透传该变量；fixture 按 job block 校验并拒绝 global/
migration 错位。两提交可在 `ccc005a` 后 fast-forward 集成。

W10-Integration final successor `ccc005a33c4a50c53f1df9352f03824e5d45418e` 精确基于
`b685541c24050b2431366466464ed5f4d347125d`，已关闭剩余 I1。`.gitignore` 现在只精确反向允许
lowercase `.github/scripts/tests/fixtures/secret/` 及内容；policy 例外同样大小写精确。三提交完整候选
`dfa3185 + b685541 + ccc005a` 可在 `f7ca476` 后 fast-forward 集成。

W10-Gates final successor `f7ca4765653b0d3ddc7cdcc033104996f9a75ee9` 精确基于
`0451b79af4ef4244799b2f56ef0896036d8499a4`，已关闭剩余 I1。contract/repro 的 finding 分类现在仅
将原生 1 映射为 1；0 保持 0，其他全部非零，包括 42/101/127/143，统一映射为 2。三提交完整候选
`a1d64b1 + 0451b79 + f7ca476` 可在 `8ec6d2d` 后 fast-forward 集成。

W10-Web 候选 `8ec6d2d882659c72f0681c96de625bb2891db5a8` 精确基于
`f576bebc7a07cffba1502c1efac364a8c40b5656`，可 fast-forward 集成。候选只在 Playwright gRPC
配置中从测试进程环境读取 bearer，并通过 browser context `extraHTTPHeaders` 注入 Authorization；
没有进入应用 bundle、URL、localStorage、日志或生产配置。

W10-Runtime successor `f576bebc7a07cffba1502c1efac364a8c40b5656` 精确基于
`1549efc701f5593ace470ea7919870b5c451c159`，已关闭前次 I1；两提交完整候选可在
`3dfe71b6ffe671317f97ef689c17fa5de7145d2f` 后串行集成。`minio-init` 现在显式使用
`MC_CONFIG_DIR=/tmp/.mc`，其 `/tmp` 为 tmpfs；resolved gate 要求配置目录是 `/tmp/` 下规范化绝对
路径，并拒绝缺失、`/etc`、`/var/lib` 和非规范路径。

里程碑一 D-023 W2/W1/W3 语义业务闭环既有 **PASS — C0 / I0 / M0** 保持有效。

W3 Task 7 successor `08365586a0e4086f69d40d56b2073c00e40ed168` 精确基于
`7b64c49c2b756308207e8ae61325c2cf31f3eade`，已关闭前次 I1；可以作为两提交完整候选在
integration `7dca5ef12264e2f3b1082240d285fc20b660489a` 后串行集成。W2 Application 与 W1 production
adapter 的既有 PASS 保持有效。

本次唯一一次 focused Review 审查完整候选范围
`87ae2ea8e98bd24ee8fb909ed2b972654f9ad780..51b22b7d8ec59f5dbceeeabc83c7c06df1716f25`。
候选包含原始实现 `ea66829b481a3406ccaa5bfa35c0c0bbd476eb94` 及其最小修正
`51b22b7d8ec59f5dbceeeabc83c7c06df1716f25`。先前发现的三项 Important 偏差均已关闭：
safe trace 固定为 32 位小写十六进制、事件名和安全字段集合与 D-023 精确一致、Signal 与
Artifact lineage 改为双向精确集合校验。

该 W2 PASS 当时仅批准 Application 合同进入集成；其后 W3、Delivery 与 Quality 已在当前集成波次
完成真实 MinIO 缺失/替换、生产事件与业务闭环证据，Q2-INV-11 和 Task 7 现已关闭。

后续 W1 production adapter focused Review 同样为 **PASS — C0 / I0 / M0**：候选
`7dca5ef12264e2f3b1082240d285fc20b660489a` 精确基于 integration
`dc55e2f110396572349956b0a8fe68e7e21467f0`，可在该基线后串行集成。

## Findings

### Critical

- 无。

### Important

- Repro Node candidate `68ec891` 无开放 finding：旧 runner Node mismatch 已由同一 frozen Contract
  artifact 关闭；policy fixture 对 Repro URL/size/hash/version/PATH/pnpm activation 漂移均 fail-closed。
- Supply secret successor `4d1b6ee` 无开放 finding：fixture 保持 fail-sensitive，同时 tracked release
  archive 与最终单提交候选不再因测试字面量产生 secret finding；未新增 ignore/allowlist/rule change。
- 前次 LF provenance I1 已由 final successor `0fab0cc` 关闭：`.gitattributes` 冻结根 Cargo.lock 与
  python/uv.lock 为 LF，inventory header 记录 archive-native hashes `0920f796...85d3`、
  `866c8707...ad8` 与 input digest `b7b68063...a62e3`；Ubuntu 24.04 actual archive verify 与 canonical
  byte-identical regeneration 均 PASS，runtime candidate/tree binding 通过。
- **I1（前任 `f33c522` finding；已由 `0fab0cc` 关闭）：mechanical inventory 未按 candidate archive 字节重生成。** actual
  Syft key universe 已精确等于 tracked 620 packages，inventory package digest 也同为
  `49355da3...a3cc3`；但 tracked header 记录 Cargo.lock `3b0363e0...ed02`、uv.lock
  `81a2b240...07a0`，而 Ubuntu 24.04 candidate `git archive` 实际分别为 `0920f796...85d3`、
  `866c8707...ad8`。因此 expected `input_tree_digest` 为 `b7b68063...a62e3`，tracked 仍是
  `3b271897...baeb`，production verifier 报 `inventory header or digest drift`，runtime provenance 无法
  建立。应从 candidate archive/native LF tree 用 fixed Syft 输出机械重生成 header/inventory，并在
  Ubuntu 24.04 对 tracked 文件做 byte-identical regeneration + verify-provenance；不要从 autocrlf
  Windows worktree 计算 lock SHA。
- 前次 I2 已由 `f33c522` 关闭：`run-gates-tests.sh` 现显式以 `bash` 调用 risk fixture，Ubuntu 24.04
  完整 gate fixture PASS，无 Permission denied。
- 前次 Syft scope/duplicate 部分已由 `f33c522` 关闭：fixture locks/manifests 改为非标准 `.fixture`
  模板并仅在临时目录恢复；production scan 无 exclude，actual 为 620 unique/0 duplicate；scope fixture
  证明普通 production Cargo.lock 仍被扫描，模板不被识别。剩余阻断仅为上述 tracked header 字节漂移。
- **I1（前任 `dfa33ac` finding；Syft scope 已由 `f33c522` 关闭）：tracked 620-package inventory 不等于 production Syft universe。** fixed
  Syft 1.46.0 对 candidate `git archive HEAD` 按 production 同一 `scan dir:` 命令得到 626 selected
  artifacts、624 unique keys；tracked inventory 只有 620。四个额外 unique key 均来自新增 fixture：
  `rust-ok@1.0.0`、`python-ok@1.0.0`、`web-ok@1.0.0`、`reachable@1.0.0`；fixture Cargo.lock 还让
  `rsa@0.9.10` 与 `sqlx-mysql@0.8.6` 各重复一次。`verify-license-inventory.py` 在 package partition 前
  即以 `duplicate Syft package key` 拒绝，因此 620=607+13 的 tracked 断言无法成为 runtime evidence。
  应使 fixture lock 不被 Syft 识别（例如精确重命名/物化为非 catalog 文件），或冻结并验证精确排除
  路径，然后用 production 同命令重生成 inventory；不得只在生成 inventory 时临时删 fixture。
- **I2（前任 `dfa33ac` finding；已由 `f33c522` 关闭）：完整 gate fixture 在 Linux 不可执行。** `run-gates-tests.sh` 直接调用
  `.github/scripts/tests/fixtures/risk-acceptance/run.sh`，但该文件 Git mode 为 `100644`；Ubuntu 24.04
  fresh run 在 Supply fixture 段报 `Permission denied`。应提交 executable bit，或与其他 fixture 一致
  显式使用 `bash` 调用，并 fresh 重跑完整 gate fixtures。
- 前次 Supply secret-history I1 已由 successor `87eaabb` 关闭：trusted base、唯一 parent、单提交范围与
  base-history/range/release-tree 三层扫描已冻结；provenance 绑定拓扑、工具与三份报告。最终发布仍须
  先生成以 `42f570f` 为 parent 的 squashed candidate，当前迭代分支拒绝不构成 finding。
- **I1（前任候选 `2f8ce72` 的历史 finding；已由 `87eaabb` 关闭）：发布候选的 secret history 边界未被证明。** `gitleaks dir` 覆盖
  `git archive HEAD` 当前树，`gitleaks git --log-opts="-1 $candidate"` 只覆盖候选单提交；任何在祖先
  commit 引入、又在 HEAD 前删除的 secret 均不属于两者覆盖。当前临时分支确有旧 secret-like fixture
  历史，虽为 fake，但证明候选并非没有祖先历史；同时尚无“trusted base 已全历史扫描 clean”或
  “只发布 squash 后单提交、不发布该祖先链”的冻结规则。应先冻结发布拓扑：若 fast-forward 发布，
  必须扫描所有将发布的 ancestry；若 squash/cherry-pick 到已证明 clean 的 base，则扫描并记录
  trusted-base..candidate 精确范围及 base 证明。不得以 Gitleaks ignore 绕过。
- 前次 LLVM runner I1 已由 successor `e9b146e` 关闭：生产 `--install` 直接、不可覆盖地读取
  `/etc/os-release`，严格要求 Ubuntu 24.04 noble 并复核 lock runner；检查先于 curl、remove、install。
  Ubuntu 24 正向及 26/非 Ubuntu/缺失/malformed 负向 fixture 均已通过。
- 前次 dependency I1 已由 successor `26fdb38` 关闭：platform-shell 权威 manifest 直接
  声明 Vite 6.4.3/Vitest 3.2.6，root ignored overrides 与 lock overrides 均移除；exact pnpm 10.12.4
  fresh-delete/regenerate byte-identical。Ubuntu 24 CI 环境证据为外部验证条件，不计 candidate finding。
- GateRecovery provenance/job-scope I1 已由 successor `b932557` 关闭。
- 前次 I1 已关闭：严格 UTF-8 解码和完整 CJK codepoint ranges 在 `C.UTF-8` 下正向通过，并拒绝纯英文、
  空文档和非法字节；final policy 已 PASS。
- 前次 I3 已关闭：lock 与 CI 使用同一 official Clang `.deb` URL、size `119448` 和 SHA-256；cpp/repro
  均先校验 size/hash 再 `dpkg`，已移除动态 apt key/index/repository。
- 前次 I2 已关闭：successor 在两个隔离副本中基于现有 `pyproject.toml`、`uv.lock` 与当前 Python
  源码生成 wheel/sdist；归档可解包，规范化摘要忽略 mtime/成员顺序，源码漂移稳定产生 mismatch。

### Minor

- 无。

## 合同复核

### Required-read capability

- `VerifiedBlobReader::read_required` 返回 `ApplicationResult<VerifiedBlobPayload>`，正式发布内容读取
  不使用 `Option`。
- `RequiredVerifiedBlobRead` 与 `VerifiedBlobPayload` 字段私有；checked constructor 绑定 scope、
  tenant、owner、resource kind/id、blob role、expected hash/size 与 safe trace。
- constructor 前置验证授权、tenant 一致、非零 size，以及 Artifact/Signal/Data/Universe 的合法
  kind-role matrix；没有 unchecked constructor 或可直接构造字段的旁路。
- `SafeTraceContext` 只接受精确 32 位 lower-hex；短值、长值、大写、非 hex 和 token-like 字符串
  均被拒绝。
- compile-fail 证明外部调用者不能直接构造 `RequiredVerifiedBlobRead`；另一个 compile-fail 证明
  `IntegrityEvent` 不暴露 owner accessor。

### 对象、角色与 lineage

- Artifact 绑定 ArtifactPayload；SignalSet 绑定 SignalPayload；DataSnapshot 必须依次验证 Parquet
  与 Manifest 两个 role；UniverseSnapshot 验证 Members Manifest。
- facade 在 required reader 前校验请求 ID、scope、tenant、owner、对象种类、hash 和 size；Data
  任一 role 失败均不返回 partial result。
- Signal 精确校验 Artifact kind、owner、object ID、无 version、Signal/Artifact content hash，并要求
  Signal 去除唯一 Artifact 自身引用后的 lineage 与 Artifact lineage 双向集合、基数完全相等；
  missing、extra 或 duplicate 均失败。

### 稳定错误与安全事件

- required blob missing、hash drift、size drift 均稳定映射为 `HashMismatch`、`retryable=false`。
- 每次完整性失败恰好调用一次 event sink；sink 自身失败不会覆盖业务 `HashMismatch`。
- 事件名精确为 `storage.published_content_integrity_failure`，severity 固定为 `error`，reason 仅为
  `missing|hash_mismatch|size_mismatch`。
- 事件只暴露 tenant、resource kind/id、blob role、expected hash/size、safe trace、severity 与 reason；
  不含 owner、bucket、key、endpoint、credential、token、payload、SQL、stack 或 raw cause。

### Metadata 与 transport 分界

- metadata 缺失在 reader 前返回 `NotFound`、`retryable=false`，不发完整性事件。
- transport 无法判定内容状态时返回 `StorageUnavailable`，不伪装为 integrity loss，也不发完整性事件。
- `SignalRepository::get`、`SnapshotRepository::get_by_id` 和 Artifact metadata 读取的文档明确说明其
  metadata-only 性质；它们不能作为 payload 已存在的证明。
- Application 没有引入 optional storage probe；W3 的可选探测仍须命名为 `probe_verified` 并留在
  Storage 边界之外的正式业务读取路径。

### 范围与稳定性

- 完整候选只修改 8 个 `ficant-application` 源码/测试文件，共 1493 insertions、2 deletions。
- 无 `interface/`、Proto、generated contract、Domain、Storage 或 Migration 变更。
- fingerprint 实现未改；既有 write fingerprints 保持不变。
- 新生产代码未发现 `cfg(test)`、test-only、fake/mock、TODO/FIXME 或未实现入口；公开 helper 均属于
  W3 adapter 实现 required reader 所需的正式合同能力。
- `git diff --check` clean，候选 worktree clean。

## Fresh 验证证据

- 固定环境：WSL `ficant-ubuntu-24.04`。
- `cargo test --locked -p ficant-application --test required_verified_reads`：8/8 passed。
- `cargo test --locked -p ficant-application --doc RequiredVerifiedBlobRead`：1/1 passed，14 filtered。
- `cargo test --locked -p ficant-application --doc IntegrityEvent`：1/1 passed，14 filtered。
- 按本次 brief 未运行全 Application 套件，也未重复 Docker/Compose；候选没有容器配置变更。

## W1 production adapter focused Review

- **Verdict：PASS — C0 / I0 / M0**。
- exact range：`dc55e2f110396572349956b0a8fe68e7e21467f0..7dca5ef12264e2f3b1082240d285fc20b660489a`；
  HEAD、唯一 parent、ancestor chain、clean worktree 与 diff-check 均正确。
- 差异只包含 `ficant-server` 的 JSONL sink、公开生产 builder 和专属风险测试，共 3 个文件、282
  insertions；无 Proto/RPC、W3、acceptance、manifest、lock、fingerprint 或其他模块漂移。
- sink 使用冻结的安全 accessor，输出精确 10 字段单行 JSON：event name、severity、reason、tenant、
  resource kind/id、blob role、64 位 lower-hex expected hash、expected size 与 32 位 lower-hex trace。
- schema 不含 owner、bucket/key、endpoint、credential/token、raw bytes、SQL、stack、cause 或自由
  message；resource kind 与 blob role 使用穷尽枚举映射。
- serialize、writer、flush 与 poisoned-lock failure 均映射 `StorageUnavailable, retryable=true`；
  `RequiredVerifiedBlobRead::fail_integrity` 仍只尝试一次 sink，并保持已知
  `HashMismatch, retryable=false`，sink failure 不覆盖业务错误。
- `build_integrity_event_sink()` 在生产代码中构造真实 stderr sink 并返回
  `Arc<dyn IntegrityEventSink>`，供 W3 组合根注入；候选没有在 W3 reader 尚不存在时制造伪 wiring。
- 新生产代码没有 `cfg(test)`、test-only、fake/mock、TODO/FIXME 或未实现入口。
- 固定 WSL Ubuntu 24.04 fresh target：
  `cargo test --locked -p ficant-server --test integrity_event_sink`，3/3 passed；未重复全 server、
  全套、Docker、Web 或 W3 测试。

## W3 Task 7 focused Review

- **I1 closure：PASS — C0 / I0 / M0**。successor exact range：
  `7b64c49c2b756308207e8ae61325c2cf31f3eade..08365586a0e4086f69d40d56b2073c00e40ed168`；
  SHA、唯一 parent、raw/cached clean、diff-check 与精确 2 个批准路径均正确。
- required reader 现在按 resource kind/blob role 分支查询 Artifact、SignalSet+Artifact、DataSnapshot
  Parquet/Manifest 与 UniverseSnapshot 当前正式行，复核 tenant、resource ID、owner、resource hash、
  request declared size、`storage.blobs` hash/size 与 immutable key 后才读取 MinIO。
- owner 漂移走一次 `hash_mismatch` event；正式 ref 删除但同 tenant blob 仍存在走一次 `missing`
  event；两者均返回 HashMismatch/retryable=false，读取前后 blob、Artifact、Signal、Data/Universe、Run、
  Journal 计数不变。
- Ubuntu 24.04 fresh only：
  `q2_inv_11_required_reads_fail_closed_for_missing_corrupt_and_wrong_size` 1/1 passed，12 filtered。
- 本次未重复 migration、D-020/D-021、其他 INV、完整 13/13 或 27/27、phase1、Docker、Web。
- successor focused Review 当时未验证 runtime image digest；该前置现已由最终 Quality/Delivery wave
  的派生 linux/amd64 OCI manifest provenance 关闭。
- exact range：`7dca5ef12264e2f3b1082240d285fc20b660489a..7b64c49c2b756308207e8ae61325c2cf31f3eade`；
  HEAD、唯一 parent、ancestor chain 与 diff-check 正确。
- Windows `status` 的大量 `M` 是 autocrlf/stat 假阳性；worktree `git diff --raw`、
  `git diff --name-only`、cached raw/name diff 均为空。
- commit 精确修改 10 个批准路径：5 个 Storage 生产文件与 5 个 Storage/Acceptance 测试文件；无
  Proto/RPC、Domain、Application fingerprint、manifest/lock、W1 server、fixture、Migration 文件或
  其他范围扩张；生产文件无 test-only/fake/mock/TODO/placeholder 入口。
- `probe_verified` 已留在 Storage optional probe；Artifact/Signal/Data 双 role/Universe 正常 facade、
  missing/hash/size 的 HashMismatch/non-retry 与恰好一次事件路径均有真实 PostgreSQL/MinIO 证据。
- 0007/0008 migration 的 legacy fail-closed、合法前向升级、重复运行与原子性 4/4 passed。直接
  `cargo test` 默认并行会让共享 schema reset 相互干扰；按仓库 `.config/nextest.toml` 的
  `shared-postgres-minio max-threads=1` 模型串行重跑后 4/4 通过，Quality 必须沿用该隔离模型。
- exact Q2-INV-03/04/06/08/11：5/5 passed；Q2-INV-11 真实覆盖 MinIO 删除、同尺寸 hash 篡改、
  size 漂移、单事件及 HashMismatch/retryable=false。
- D-020/D-021、Snapshot durable refs、Signal exact binding、Run/Journal CAS 的 repository/concurrency
  exact filters：5/5 passed。
- 候选 focused Review 中正向测试的 runtime digest 前置曾保持 `unverified`；最终 Quality wave 已用
  当前业务 SHA 派生的 linux/amd64 `python-node-runtime` OCI manifest digest 完成同一正向闭环，旧前置
  不再开放。
- 按 brief 未运行 worker 完整 13/13、27/27、全套、Docker、Web 或容器验收。

## W3 handoff

- 已完成：W3 两提交与 fixture-path successor 均已进入业务 SHA `dbcff347`，唯一 Quality wave 与
  Delivery provenance 均 PASS；Q2-INV-11 和 Task 7 已关闭。

## Quality fixture-path blocker closure

- **PASS — C0 / I0 / M0**；candidate
  `dbcff34793e79e73ed63872e28ed6298feedfbc4` 精确基于
  `08365586a0e4086f69d40d56b2073c00e40ed168`，可在该 parent 后集成。
- diff 仅修改 `crates/ficant-acceptance/tests/phase1_business_loop.rs`；SHA、parent、raw/cached clean 与
  diff-check 正确。无生产代码、fixture 内容、manifest、lock、本机路径或其他范围漂移。
- 未设置 `FICANT_ACCEPTANCE_FIXTURE` 时保持既有默认路径；absolute 配置不变；relative 配置从编译期
  `CARGO_MANIFEST_DIR` 上溯 `crates/ficant-acceptance -> crates -> repository root` 后拼接，不依赖
  nextest/cargo 当前工作目录。
- Ubuntu 24.04 真实 stage、relative
  `tests/golden-cases/china-rates/phase1-business-loop.json`：exact positive 1/1 passed。
- Delivery 授权注入的 digest 是当前 SHA 锁定 Python 基础 runtime **manifest digest**，来自
  `deploy/dev/toolchain.lock.toml [python].image` 与 `python/node-runtime/Dockerfile FROM` 的一致值；它
  **不是派生 runtime image 摘要**，后续最终 Delivery/Quality 证据不得重标其 provenance。
- 未运行 negative、Quality 双 target、Storage/Migration/Server、Docker 或 Web。

## 里程碑一集成波次最终 Review

- **PASS — C0 / I0 / M0**。
- integration HEAD `3dfe71b6ffe671317f97ef689c17fa5de7145d2f` clean；唯一父提交是业务 SHA
  `dbcff34793e79e73ed63872e28ed6298feedfbc4`，集成提交只更新
  `docs/quality/evidence.md`，未改变已 Review 的生产候选。
- Quality 唯一持久 wave：nextest run ID `24556685-366c-4060-821d-bf94b58a6802`，exit `0`，
  14/14 passed、0 skipped；持久证据位于
  `/var/tmp/ficant-iteration2-quality-wave-dbcff347/`，其 start/end metadata、full log 与 exit code 对
  业务 SHA、Ubuntu 24.04.4、fixture hash、真实 PostgreSQL 16.10、真实 MinIO 和测试摘要一致。
- 正向闭环完成市场定义/事实 → Curve/Data/Universe Snapshot → Run revision `1→2→3` →
  Artifact/SignalSet → Journal `1..5`；重连后四类 required read 成功，两次 replay 一致，5 个正式
  MinIO 对象且 staging/orphan 为零。
- Q2-INV-01..12 全部通过。Q2-INV-11 证明 object missing、同尺寸篡改、尺寸漂移、正式引用 hash
  漂移和正式引用缺失均为 HashMismatch/retryable=false、恰好一次结构化事件，并保持 metadata、Run、
  Journal、正式引用及七维副作用计数不增加。
- Delivery runtime/provenance verdict **PASS — C0 / I0 / M0**；业务 wave 使用当前业务 SHA 派生的
  linux/amd64 `python-node-runtime` OCI **image manifest digest**
  `sha256:8e97031468b2ad51ab8484d06d8af9d63f1b73f8c04654f17be40ac629076cd9`，明确不是 base
  image digest、config digest 或 OCI tar hash；真实 PG/MinIO、secret scan、SHA 与 clean 均已复核。
- 既有 W2/W1/W3/fixture-path Review 均为 PASS C0/I0/M0，无开放 Critical/Important/Minor finding。
- **Closure：** Q2-INV-11、Task 7 与里程碑一 Phase 0/1 语义业务闭环关闭。Task 10、发布/容器专项与
  iteration-2 最终退出门不在本次 Review 结论内。

## Delivery handoff

- 里程碑一 runtime/provenance 已由 Delivery PASS；派生 image manifest provenance 与安全出口事实已被
  最终 wave 消费，不再保留待验证行动。
- Task 10、容器专项和 iteration exit 的 Delivery 责任保持独立，未被本次语义闭环 PASS 提前关闭。

## Orchestrator 后续动作

- [x] W3 与 fixture-path successor 已集成至业务 SHA `dbcff34793e79e73ed63872e28ed6298feedfbc4`。
- [x] 唯一 Quality business wave 与 Delivery runtime/provenance 已在同一业务 SHA 完成并通过。
- [x] Q2-INV-11、Task 7 与里程碑一语义业务闭环已关闭。
- [x] `minio-init` `/tmp/.mc` 可写配置目录 I1 已由 `f576bebc` 关闭。
- [x] Gates I1/I2 已由 `0451b79`、`f7ca476` 关闭。
- [x] Gates 三提交已集成至当前 parent `f7ca476`。
- [x] Integration I1–I3 已由 `b685541`、`ccc005a` 关闭。
- [ ] 在 `f7ca476` 后 fast-forward 集成 `dfa3185`、`b685541`、`ccc005a`，再继续 Task 10 最终验收。
- [x] GateRecovery I1 已由 `b932557` 关闭。
- [ ] 在 `ccc005a` 后 fast-forward 集成 `7ad13a8`、`b932557`，再继续 Task 10 最终验收。
- [ ] 在 `b932557` 后 fast-forward 集成 ContractRecovery `2967fcb`。
- [ ] Integration successor focused Review 通过后再继续 Task 10 最终验收与
  iteration-2 退出门。

## W10-Runtime focused Review

- **Verdict：PASS — C0 / I0 / M0；两提交完整候选可集成。**
- exact range：`3dfe71b6ffe671317f97ef689c17fa5de7145d2f..1549efc701f5593ace470ea7919870b5c451c159`；
  HEAD、唯一 parent、raw/cached clean 与 diff-check 均正确。
- 差异精确为 4 个批准路径：`deploy/dev/docker-compose.yml`、`deploy/dev/toolchain.lock.toml`、
  `python/compose_security_gate.py`、`python/tests/test_compose_security_gate.py`；无业务、CI、语言 lock
  或其他范围漂移。
- PostgreSQL、MinIO、mc 均使用完整 RepoDigest；7-service DAG、持久卷、migration 幂等记录、建桶
  `--ignore-existing`、loopback 端口、non-root/read-only/tmpfs/cap-drop/no-new-privileges、CPU/内存/PID
  限制和必需凭证 fail-closed 注入均已静态复核。
- Ubuntu 24.04 fresh：`python3 -m unittest python.tests.test_compose_security_gate -v`，19/19 passed。
- Windows Docker CLI 仅执行 `docker compose config --quiet` 和 JSON 解析后 resolved gate，均 PASS；
  临时非敏感测试凭证已在命令结束后清除且未输出。未执行 `up`、`build`、`inspect`、runtime gate 或
  任何容器资源操作。
- 固定 Ubuntu 24.04 WSL 内没有 Docker CLI，因此 Compose 配置解析使用本机 Docker CLI 完成；该环境
  差异不归因于候选。
- successor exact range：`1549efc701f5593ace470ea7919870b5c451c159..f576bebc7a07cffba1502c1efac364a8c40b5656`；
  SHA、唯一 parent、raw/cached clean、diff-check 与精确 3 个批准路径均正确。
- successor 只增加 `MC_CONFIG_DIR=/tmp/.mc` 及 resolved gate/测试约束；gate 通过 `/tmp/` 前缀、
  `posixpath.normpath(value) == value` 与 `/tmp` tmpfs 三项联合条件，拒绝缺失、`/etc`、`/var/lib`、
  路径穿越或其他非规范路径。
- successor fresh only：新增专属 target 1/1 passed；`docker compose config --quiet` 与 JSON
  resolved gate 均 PASS。未重复 19/20 全模块、其他静态范围或任何 live Docker 操作。

## W10-Web focused Review

- **Verdict：PASS — C0 / I0 / M0；可在 `f576bebc` 后 fast-forward 集成。**
- candidate `8ec6d2d882659c72f0681c96de625bb2891db5a8`，唯一 parent
  `f576bebc7a07cffba1502c1efac364a8c40b5656`；SHA、raw/cached clean 与 diff-check 正确。
- diff 精确为 `web-dm/playwright.grpc.config.ts` 3 insertions；未改 spec、Shell、API、Proto、Compose、
  lock 或其他路径。旧 `d62df12` 仅为 rebase 前等价提交，不可集成。
- bearer 只从 `FICANT_GRPC_WEB_BEARER_TOKEN` 读取；仅在 Playwright browser context 的
  `extraHTTPHeaders` 形成 Authorization。没有硬编码值、输出语句、Vite client env、URL 参数、
  localStorage 或应用源码引用。
- Ubuntu 24.04 fresh：按候选 SHA 导出原生文件系统快照，使用冻结 pnpm lock；直接启动真实
  `target/debug/ficant-server` 进程并确认监听 `127.0.0.1:50051`，同一
  `pnpm@10.12.4 test:e2e:grpc` 命令 Q2-WEB-02 1/1 passed。spec 无 route fulfill/MSW/mock，两个 POST
  请求命中真实 Rust PlatformService 后得到空正式 Registry 与有效 session 页面状态。
- 测试凭证仅存在于 server/Playwright 测试进程环境且未输出；服务、快照和依赖目录均已清理，端口
  已关闭。未运行完整 Web、业务、Storage、Docker/Compose 或其他套件。

## W10-Gates focused Review

- **Verdict：PASS — C0 / I0 / M0；三提交完整候选可集成。**
- candidate `a1d64b1571959418eeeba350e8ad39c0b0f795f3`，唯一 parent
  `8ec6d2d882659c72f0681c96de625bb2891db5a8`；SHA、raw/cached clean 与 diff-check 正确。
- 27 个新增路径全部位于批准的 `.github/scripts/` gate/fixture/config 写域；无 workflow、repo-policy、
  `.gitignore`、deploy、docs、业务、Proto、生成树、manifest 或语言 lock 漂移。旧 `e3c3`、`8f48`
  为 rebase 前提交，不可集成。
- contract 使用权威 exact `CONTRACT_BASE_SHA=591dfcaf...` 与 descriptor SHA-256 `d1832ff4...`，Buf
  exact-ref breaking、双生成、tracked tree digest 及 Rust/Python/TypeScript consumer 均存在。
- Rust/C++/Web 在两个 archive 隔离副本中构建并对规范化路径/内容摘要；Python 当前只同步依赖并摘要
  lock/freeze，形成 I2。
- OSV-Scanner 2.4.0、Syft 1.46.0、Gitleaks 8.28.0 的官方版本、asset、完整 SHA-256、许可证和官方
  checksum 元数据均锁定；三类 OSV generation URL、size/hash、24 小时采集新鲜度与 aggregate 计算
  均 fail-closed。实际下载抽查 `crates.io` generation 的 size/hash PASS；GitHub release 页面确认精确
  版本，release asset 正文哈希抽查因当前 TLS/CDN 中断未形成 fresh 证据，不归因于候选。
- OSV 使用三类本地 generation 数据库及 `--offline`；未知或 `>=7.0` 漏洞阻断；许可证严格 SPDX
  allowlist，当前无例外项，因此不存在未到期豁免；Gitleaks 同时扫描 tracked release tree 与完整
  `--all` 历史，并使用 `--redact`。脚本无 skip、浮动 latest、源码上传或自动提交路径。
- Ubuntu 24.04 fresh：`run-gates-tests.sh` PASS，4 个脚本 `bash -n` PASS。未运行真实全仓供应链扫描、
  完整四类 build、Docker 或 Compose。
- gate 默认输出位于临时目录，证据只含 redacted secret report；本次验证前后 candidate clean，无秘密
  或 tracked-tree 污染。
- successor exact range：`a1d64b1571959418eeeba350e8ad39c0b0f795f3..0451b79af4ef4244799b2f56ef0896036d8499a4`；
  SHA、唯一 parent、raw/cached clean、diff-check 与精确 3 个批准脚本路径均正确；无 manifest/lock
  或其他范围漂移。
- I2 closure：实际 wheel/sdist 包含当前 Python 源码及现有 project/lock 元数据；固定 zip/tar 元数据并
  按成员路径与内容做规范化摘要。fixture 验证 wheel/sdist 可解包、mtime 改变仍相等、源码改变 mismatch。
- I1 partial：两个完整脚本的外部工具边界不再泄漏原生退出码；tool+101/127/42/143 为 2，finding+1
  为 1。但 finding+42/127/143 仍为 1，且 fixture 未覆盖，所以 I1 保持开放。
- successor fresh only：`run-gates-tests.sh` PASS；三个变更 Bash 脚本 `bash -n` PASS。未运行真实扫描、
  完整 build、Docker 或 Compose。
- final successor exact range：`0451b79af4ef4244799b2f56ef0896036d8499a4..f7ca4765653b0d3ddc7cdcc033104996f9a75ee9`；
  SHA、唯一 parent、raw/cached clean、diff-check 与精确 3 个脚本、6 insertions/2 deletions 均正确。
- final I1 closure：两个 gate 都仅在 `class=finding && rc=1` 时返回 1；native 0→0，finding 1→1，
  finding/tool 的 42/101/127/143→2。fixture 对两个脚本逐项覆盖 finding+1/42/101/127/143 与
  tool+101/127/42/143。
- final fresh only：full fixture PASS；三个变更脚本 `bash -n` PASS；未运行其他检查。

## W10-Integration focused Review

- **Verdict：PASS — C0 / I0 / M0；三提交完整候选可集成。**
- candidate `dfa3185c91feae5cafbb3fbe7e528c18e66f8c02`，唯一 parent
  `f7ca4765653b0d3ddc7cdcc033104996f9a75ee9`；SHA、raw/cached clean 与 diff-check 正确。
- diff 精确为四个批准路径：`.github/workflows/ci.yml`、`.github/scripts/verify-repo-policy.sh`、
  `.github/scripts/tests/run-repo-policy-tests.sh`、`.gitignore`；无 deploy、docs、业务、Proto、生成树、
  manifest 或 lock 漂移。
- YAML 结构解析成功，jobs 恰好为 repo-policy、contract、rust、python、cpp、web、migration、
  business-loop、supply-chain、reproducibility 十项；无 `needs`，不会因上游 skip 假绿。十 job 均使用
  checkout 完整 SHA 和 `fetch-depth: 0`；Actions 与 CI/服务镜像均为完整 SHA/RepoDigest。
- contract 调用已 Review exact-ref/三 consumer gate；Web 启动真实 Rust gRPC-Web 并运行 exact
  `test:e2e:grpc`，既有 spec 无 mock；migration/business-loop 使用固定 PostgreSQL/MinIO digest、
  串行测试与非敏感 CI fixture credentials；supply/repro 调用已 Review 脚本。
- compose static module 归 repo-policy job；CI 无 live Compose 或 runtime inspect。无 job dependency、
  secret expression、credential echo 或源码上传路径。
- Policy 顶层 allowlist、未知 root、治理/私有根、旧 UI-DM 与多数 build/cache deny 正确，并允许当前
  Phase 0/1 根及 `domain-packs`；web lock 要求精确为 `web-dm/pnpm-lock.yaml`。但嵌套目录变体存在 I2。
- Ubuntu 24.04 fresh：policy fixture PASS；final repo-policy FAIL（2 个中文文档误报，伴随两次
  `Invalid collation character`）；两个 policy Bash 文件 `bash -n` PASS。宿主 PyYAML 解析十 job PASS。
- gitignore probe：治理/未知 root 被忽略，`crates`、`domain-packs`、web lock 发布源码不被忽略；
  `docs/key|secret|worker|worktree|temp|cache` 均未忽略，形成 I2 实证。
- 未访问 GitHub，未运行真实十门、业务/full build/scan、Docker 或 Compose。
- successor exact range：`dfa3185c91feae5cafbb3fbe7e528c18e66f8c02..b685541c24050b2431366466464ed5f4d347125d`；
  SHA、唯一 parent、raw/cached clean、diff-check 与允许的 5 文件范围均正确。
- I1 closure evidence：strict UTF-8+CJK 正向、纯英文/空/非法字节负向均由 fixture 覆盖；Ubuntu 24.04
  full policy fixture 与 final policy PASS。
- I2 partial：任意深度 component 大小写归一 deny 与 recursive ignore 对 key/secret/worker/worktree/
  temp/cache 生效；`worker_pool`、`secretary`、`cache-policy` 与发布源码不误伤。唯一受控 secret fixture
  policy 例外精确，但 gitignore 同步缺口保持 I1 finding。
- I3 closure evidence：lock 新增 official URL/size 且保留 SHA；CI 两处下载后 size+SHA 校验再 dpkg，
  无 LLVM apt key/index/repo。policy fixture 静态锁定 CI/lock 三值一致。
- successor fresh only：policy fixture PASS、final policy PASS、两个 Bash 文件 `bash -n` PASS、YAML
  十 job parse PASS；gitignore 正负探针除受控 fixture 同步缺口外均符合。未运行其他门。
- final successor exact range：`b685541c24050b2431366466464ed5f4d347125d..ccc005a33c4a50c53f1df9352f03824e5d45418e`；
  SHA、唯一 parent、raw/cached clean、diff-check、精确 3 文件及 28 insertions/2 deletions 正确。
- final I1 closure：policy 使用原始 path 做 lowercase 精确 fixture 例外；gitignore 在递归 secret deny 后
  只反向允许该精确 lowercase 目录及内容。已知/新受控 evidence 均 not ignored；其他深度
  `secret/Secret/secrets` 与 uppercase fixture 变体仍 ignored/rejected；`secretary` 不误伤。
- Windows probe 显式使用 `git -c core.ignoreCase=false`，与 Ubuntu release gate 语义一致。
- final fresh only：full policy fixture、final policy、Bash 语法、YAML 十 job 与独立 gitignore probe
  全部 PASS；未运行真实十门或其他检查。

## W10-GateRecovery focused Review

- **Verdict：PASS — C0 / I0 / M0；两提交可集成。**
- candidate `7ad13a8321db7cf4a255738925b0df905f5fce34`，唯一 parent
  `ccc005a33c4a50c53f1df9352f03824e5d45418e`；SHA、raw/cached clean、diff-check 与精确 4 个批准
  路径正确。
- `download_verified` 对所有 3 个固定 tools 与 3 个 generation-pinned OSV DB 统一执行最多 3 次；
  transport/OSError/URLError（含 TLS EOF）失败后固定 sleep 1s、2s，每次先后清理 `.tmp`。第三次永久
  失败为 exit 2；size/hash mismatch 首次即清理并 exit 2，不重试。
- tool URL/hash 与 OSV official generation URL/size/hash、aggregate、freshness、local DB 及
  `--offline` 扫描语义未放松；无浮动 fallback。
- Download fixture 实证 transient 第 2 次成功、permanent 精确 3 次后 exit 2 且零残留、integrity
  mismatch 精确 1 次 exit 2 且零残留。
- business-loop runtime env 值精确为 `sha256:8e97031468b2ad51ab8484d06d8af9d63f1b73f8c04654f17be40ac629076cd9`，
  是 `dbcff347` 证据冻结的派生 python-node-runtime linux/amd64 OCI image manifest，不是 base、config
  或 tar digest；格式为 64 lower-hex。值本身与 invalid replacement fixture 正确，注释归属形成 I1。
- Windows 挂载 worktree 的 mixed CRLF/LF 曾使精确 awk policy parser 全量误报；按候选 SHA 导出的
  Ubuntu 原生 LF 快照重跑后，gate fixtures、policy fixtures、final policy、Bash 语法、YAML 十 job
  全部 PASS。该行尾差异不归因于候选。
- 未运行真实 Supply/Contract/Repro、业务、GitHub、Docker 或 Compose。
- successor exact range：`7ad13a8321db7cf4a255738925b0df905f5fce34..b93255767c92e73f28206f3af0910032b6b15d26`；
  SHA、唯一 parent、raw/cached clean、diff-check、精确 2 文件与 34 insertions/5 deletions 正确。
- successor 将 provenance 注释移入 business-loop，并在下一行 export 精确派生 manifest；Docker
  `--env FICANT_TEST_RUNTIME_IMAGE_DIGEST` 只透传该值。注释准确说明 `dbcff347` 已验证、runtime inputs
  未变化且当前 job 不重建。
- policy fixture 解析 business-loop job block，要求注释与 export 各恰好一次且相邻；全局或 migration
  错位、base/config/tar/缺前缀或任意无效 digest 均 RED。
- successor fresh only：候选 SHA Ubuntu 原生 LF 快照的 full gate fixtures、policy fixtures、final
  policy、Bash 语法和 YAML 十 job 全部 PASS；未运行其他门。

## ContractRecovery focused Review

- **Verdict：PASS — C0 / I0 / M0；可在 `b932557` 后 fast-forward 集成。**
- candidate `2967fcbd21b9ceaa8039846633104ce0c3b74869`，唯一 parent
  `b93255767c92e73f28206f3af0910032b6b15d26`；SHA、raw/cached clean、diff-check 与精确两个批准
  路径正确。
- diff 只修改 `.github/scripts/verify-contract-generation.sh` 与既有 gate fixture；D-012、descriptor
  authority hash、Proto、interface、三语言生成树和其他路径均未改，interface diff 为 0。
- `build_descriptor` 参数顺序精确为 `buf build <input> --as-file-descriptor-set -o <output>`；完整 gate
  descriptor 路径复用该函数。fake Buf fixture 记录全参数并与精确 FileDescriptorSet 命令逐行比较，
  防止默认 Buf image 静默替代权威证据格式。
- Ubuntu 24.04 候选原生 clone fresh：Buf 1.56.0；format PASS；lint PASS；breaking 精确绑定
  `591dfcaf46eb9fdc8a68d879edbc542dd9ded448` PASS。
- 当前默认 Buf image SHA-256 为
  `bb144a02a4ad486e4e649ce11d779c58ecbf9a50a66fb0e034324bf05d0d4edb`；FileDescriptorSet SHA-256
  为 `d1832ff40a3057d9ae11c7e7dcc8c847efbf13c76f4e18a14f8d905be3fdf1d0`，size `59848`。
- Gate fixture PASS。按 brief 未运行 Rust/Python/TypeScript 三 consumer 或完整 Contract gate。

## CIRecovery2 focused Review

- **Verdict：PASS — C0 / I0 / M0；可在 `2967fcb` 后 fast-forward 集成 `91f088f`。**
- candidate `91f088fc52ba9313e21ca19fb5c758b955fe9ff1`，唯一 parent
  `2967fcbd21b9ceaa8039846633104ce0c3b74869`；candidate worktree clean，raw/cached diff 为空，
  `git diff --check` PASS。diff 精确为四个批准路径：CI workflow、gate fixture、policy fixture 与
  supply-chain gate；共 80 insertions/3 deletions。旧 `8155255` 已失效，不可集成。
- Web Node 容器在同一 `sh -ec` 内先 `corepack enable`，再固定激活 `pnpm@10.12.4`；首个 install
  直接使用 exact Corepack pnpm，后续根脚本及其嵌套 `pnpm --filter` 通过已激活 shim/PATH 执行。
  CI 先启动真实 `ficant-server` 并等待 50051，就绪后向该地址运行 `test:e2e:grpc`；既有 Playwright
  配置/spec 不做 route interception 或 mock，并通过请求与 UI 响应验证真实 PlatformService。
- Rust job 先执行 `cargo build --workspace --all-targets --locked`；随后 workspace test 排除
  `ficant-acceptance` 与 `ficant-storage`，避免在无外部服务 job 重跑 acceptance/storage integration，
  但用独立 `cargo test --locked -p ficant-storage --lib` 保留 storage library 单元测试。migration 与
  business-loop 为无 `needs` 的独立 job，分别保留真实串行 migration 与业务验收，不会因其他 job skip。
- Supply cache 缺文件由 `cache_file_size` 安静返回 `0`，fixture 精确验证 stdout `0`、stderr 为空；
  正式 DB cache probe 复用该 helper。evidence 输出唯一绑定 `${{ runner.temp }}/ficant-supply-evidence`；
  gate 后的 upload step 使用 `if: always()`、`if-no-files-found: error`，上传完整固定目录。官方仓库
  `refs/tags/v4.6.2` fresh 解析为完整 SHA
  `ea165f8d65b6e75b540449e92b4886f43607fa02`，与 workflow 完全一致。
- Gitleaks 仍同时扫描 release tree 与完整历史，两个报告均启用 `--redact`；artifact 只上传固定 evidence
  目录，未发现 secret expression、凭证原文输出或源码上传路径。
- WSL Ubuntu 26.04 原生 LF clone fresh：gate fixture PASS、repo-policy fixture PASS、final policy PASS；
  三个变更 Bash 文件 `bash -n` PASS；PyYAML 结构检查确认十 job、Rust/Web/Supply 命令、upload 顺序与
  migration/business-loop 无 `needs` 全部 PASS。
- 按 brief 未运行真实 GitHub CI、Docker、业务、完整 Contract/Supply/Repro 门。该 focused evidence
  允许集成，但 Ubuntu 26.04 不替代 iteration exit 所需的 Ubuntu 24.04/真实 CI 兼容性证据。

## LLVM Tool-First focused Review

- **Verdict：PASS — C0 / I0 / M0；可在 `91f088f` 后 fast-forward 集成 `6ef0760`。**
- candidate `6ef07607a20b25b8ffc51bfbfed58034ad73269c`，唯一 parent
  `91f088fc52ba9313e21ca19fb5c758b955fe9ff1`；worktree clean，raw/cached diff 为空，
  `git diff --check` PASS。diff 精确为 setup script、专属 fixture、CI C++/Repro steps 与 toolchain lock
  LLVM metadata 四个批准路径，共 460 insertions/10 deletions。
- lock、setup constants 与 fresh 下载的 Noble `Packages.gz` 精确绑定 URL、size `12613` 和 SHA-256
  `8cf692ec3dd86f484d2db39877b35a7f8124bb60b7f66c03f78e030fe33d3919`；strict gzip/UTF-8 与重复字段
  解析后只接受六个唯一 package paragraph。
- 六包名称/顺序、Source `llvm-toolchain-18`、完整 Debian version、`amd64`、pool prefix、逐包 filename/
  size/SHA-256 均须与 lock 和 verified index 同时一致；从 `clang-18` 做 dependency reachability，六包
  必须全部可达。其他 Pre-Depends/Depends、Breaks/Conflicts 元数据只取自已验签索引，不使用动态 resolver。
- 依赖 parser 拒绝 alternatives、未知包和非冻结语法；闭包内版本约束用 `dpkg --compare-versions`，
  外部系统依赖必须属于精确 allowlist 并由 live `dpkg-query` 逐约束验证。fresh index 生成 22 条系统
  约束，唯一 Breaks 为 `llvm-18-dev << 1:18.1.8~++20240730104741`，Conflicts 为空；缺失依赖、命中
  Breaks/Conflicts fixture 均 RED。
- 六个 deb 下载后先逐文件 size/hash，再用 `dpkg-deb -f` 复核 Package/Version/Architecture/Source；
  然后恰好一次 `dpkg --no-act --install`，成功后才恰好一次 `sudo dpkg --install`。无 apt/apt-get、
  GPG、keyserver、sources.list、动态 LLVM repository、pipe-to-shell 或 fallback 路径。
- C++ 与 Repro job 各调用同一 `.github/scripts/setup-llvm-toolchain.sh --install`，总计精确两次；旧的
  单 deb 安装已移除。既有 CI policy 的 root identity marker 继续通过，专属 fixture 另行锁定共用调用次数。
- WSL Ubuntu 26.04 原生 LF clone fresh：LLVM fixture PASS，两个 Bash 文件 `bash -n` PASS，CI policy
  风险子门 PASS，固定 Packages.gz 实物 size/hash 与 index manifest 语义检查 PASS。fixture 构造 deb 的
  非 root owner warning 不影响包生成或正负断言。
- 按 brief 未下载六个真实 deb、未运行 `dpkg --no-act` live 路径、未执行真实安装、完整 CI、业务或
  其他门。该 focused evidence 允许集成，但不替代 Ubuntu 24.04 runner 的真实 Tool-First 安装证据。

## Dependency upgrade focused Review

- **Verdict：BLOCK — C0 / I1 / M0；candidate `0f011d6` 当前不可集成。**
- candidate `0f011d6353abca5e27c7fdcb3dcdbb84cf910d36`，唯一 parent
  `91f088fc52ba9313e21ca19fb5c758b955fe9ff1`；worktree clean，raw/cached diff 为空，
  `git diff --check` PASS。diff 精确为 `python/pyproject.toml`、`python/uv.lock`、
  `web-dm/package.json`、`web-dm/pnpm-lock.yaml` 四个批准路径，共 123 insertions/85 deletions。
- Python manifest/uv lock 将 protobuf 精确锁为 6.33.5；Web 当前 lock 将 Playwright 精确锁为 1.55.1、
  Vite 6.4.3、Vitest 3.2.6，且相应 Playwright/Vite/Vitest 子树与 peer resolution 同步。Playwright >=18、
  Vite/Vitest 的 Node range 均覆盖冻结 Node 22.17；protobuf 6.33.5 可加载既有 6.31.1 generated code。
- OSV scanner 2.4.0 与三类 generation-pinned DB 均按项目 lock size/hash fresh 校验。相同命令对
  base→candidate：protobuf 两条 `GHSA-7gcm-g887-7qv7`/`PYSEC-2026-1805` 清零；Playwright 一条
  `GHSA-7mvr-c777-76hp`、Vite 七条、Vitest `GHSA-5xrq-8626-4rwp` 清零。candidate Web 为 0 finding；
  candidate Python 只保留本次非目标 pytest 8.4.1 的两条 fixed-at-9.0.3 advisory。
- Cargo 未改且 fixed snapshot 仍报告 `async-std@1.13.2` 的 `RUSTSEC-2025-0052` 与
  `rsa@0.9.10` 的 `RUSTSEC-2023-0071`，均无 fixed event。Cargo.lock 明确证明 async-std 由冻结
  `minio@0.4.0` 引入；minio 本身未被该 snapshot 标记。候选四文件、supply lock 与验证器均无
  ignore/exception，未把这些既存无修复风险伪关闭。
- Python 3.12.13 + protobuf 6.33.5 fresh：版本 import PASS，代表性 generated contract pytest 1/1 PASS。
- exact pnpm 10.12.4 对当前 lock 的 frozen install PASS，实际解析 Playwright 1.55.1、Vite 6.4.3、
  Vitest 3.2.6；platform-shell typecheck PASS，Vitest 4 files/28 tests PASS。该 fresh 单元链在可用的
  Node 24.14 diagnostic 环境运行；项目 exact Node 22.17 不可用，因此不把它作为冻结 runtime 证据。
- Playwright E2E 在断言前因本机缺 corepack 无法启动 webServer；现有 browser cache 只绑定其他项目的
  Playwright 1.61.1，未形成 1.55.1 固定浏览器证据。按 brief 未安装浏览器，不将环境 blocker 记为
  candidate assertion failure，也不宣称 E2E 通过。
- **I1 实证：** exact pnpm 10.12.4 明确警告 root `package.json#pnpm.overrides` 与
  `pnpm.onlyBuiltDependencies` 不再读取；本候选新增 overrides 因此无效。真正的 package owner
  `web-dm/platform-shell/package.json` 未修改，仍精确声明易受影响的 Vite 6.3.5/Vitest 3.1.4；当前
  fixed lock 只是结果快照，不能证明下一次 lock 重建维持修复版本。离线重建探针因本机缺 registry
  metadata 在解析前停止，未作为额外断言；pnpm 自身的 ignored-config warning 已足以构成阻断证据。
- 未运行完整 Supply gate、真实 Node 22.17 CI、Playwright 浏览器、Docker 或业务测试。

## Dependency successor focused Review

- **Verdict：CONDITIONAL PASS TO EXTERNAL VALIDATION — C0 / I0 / M0。** exact candidate 可推送并
  触发 GitHub Ubuntu 24 CI；在 candidate-bound frozen install、typecheck、Vitest 通过前，不批准最终
  fast-forward 集成，也不把 Task 10/iteration exit 标为完成。
- successor `26fdb388a9b0d62afa9d99a914b0dcfe2b3db1bb`，唯一 parent
  `0f011d6353abca5e27c7fdcb3dcdbb84cf910d36`；iteration base
  `91f088fc52ba9313e21ca19fb5c758b955fe9ff1` 是 ancestor。successor/完整范围 worktree clean，raw/cached
  diff 为空，`git diff --check` PASS。
- 相对 iteration base 的完整范围精确为五个批准文件：Python manifest/uv lock、Web root manifest、
  platform-shell manifest 与 pnpm lock，共 131 insertions/101 deletions；无 Cargo、supply policy、源码、
  contract、CI、Docker 或其他路径漂移。
- 前次 I1 关闭：`web-dm/platform-shell/package.json` 直接精确声明 `vite: 6.4.3`、`vitest: 3.2.6`；
  root `pnpm.overrides` 和 lock overrides 均已移除。Playwright 1.55.1 仍由 Web root manifest/lock 精确
  绑定，protobuf 6.33.5 仍由 Python manifest/uv lock 精确绑定。
- Windows 原生临时 clone 使用 exact pnpm 10.12.4，删除 `pnpm-lock.yaml` 后只执行 lock regeneration；
  重建文件与 candidate 逐字节相同：SHA-256
  `c6187a35e341da2cc7830546216df13702939224da6c137b0afc8c26c2ee7c04`，68140 bytes。Node 24 的
  engine warning 不影响 lock-only resolution；没有执行 install、script、typecheck 或测试。
- fixed OSV scanner/snapshot 证据仍适用到目标版本：protobuf 6.33.5、Playwright 1.55.1、Vite 6.4.3、
  Vitest 3.2.6 均保持前次 target advisory clear。Python files、Cargo.lock、supply-chain lock/validator
  blob 与前候选完全相同；Web target versions 未变。
- Cargo fixed snapshot 的既存 `async-std@1.13.2`/`RUSTSEC-2025-0052`（由 `minio@0.4.0` 引入）与
  `rsa@0.9.10`/`RUSTSEC-2023-0071` 继续保持无 fixed event 且可见；candidate 无 ignore/exception 或
  supply verifier 修改，未伪关闭这些风险。
- 遵守 brief，未在 WSL→NTFS 重跑连续 EACCES 的 install。successor 改动 pnpm lock，按 PROQAID 使
  前次本地 frozen install/typecheck/Vitest 证据失效；该缺口应由 GitHub Ubuntu 24 exact candidate
  外部门一次性关闭。它阻止最终集成/exit，但不阻止候选进入外部验证。

## CI syntax recovery focused Review

- **Verdict：PASS — C0 / I0 / M0；可在 `926a640` 后 fast-forward 集成 `75c4fe0`。**
- candidate `75c4fe043b564d39a997c76c74c395d581f91697`，唯一 parent
  `926a6401ab78124cede9d971e82dc73d5a17e87e`；worktree clean，raw/cached diff 为空，
  `git diff --check` PASS。commit 精确只修改 `.github/workflows/ci.yml`，2 insertions/2 deletions。
- 两行语义移动移除 supply-chain job-level `env`，并把同一
  `FICANT_GATE_OUTPUT_DIR: ${{ runner.temp }}/ficant-supply-evidence` 绑定到
  `bash .github/scripts/verify-supply-chain.sh` step；`runner.temp` 现在只在合法 step env 与 upload
  `with.path` context 中使用。
- gate step 后的 upload 顺序不变，仍为 `if: always()`、官方完整 action SHA
  `ea165f8d65b6e75b540449e92b4886f43607fa02`、固定 artifact name/path 与
  `if-no-files-found: error`；即使 gate 失败仍尝试上传，证据缺失继续 fail-closed。
- supply gate、fixture 与 lock 均未改；Gitleaks directory/history 两条命令仍各自使用 `--redact`，
  evidence 目录与 digest manifest 语义未放松。无 secret expression、凭证原文或源码上传路径新增。
- fixed actionlint 1.7.12 Windows/amd64 PASS；固定 archive SHA-256
  `6e7241b51e6817ea6a047693d8e6fed13b31819c9a0dd6c5a726e1592d22f6e9` 与既有 release checksum 一致。
- WSL Ubuntu 26.04 原生 LF clone fresh：repo-policy fixtures PASS、CI policy 子门 PASS、PyYAML supply
  job/step/upload scope 与顺序结构 PASS、redaction/unchanged-script probe PASS。
- 按 brief 未运行业务、Docker、真实 Supply scan、完整 CI 或其他门。

## LLVM runner recovery focused Review

- **Verdict：BLOCK — C0 / I1 / M0；candidate `5992c2d` 当前不可集成。**
- candidate `5992c2d2fd8ff049781ccc4e0a46f1dcb35e793d`，唯一 parent
  `75c4fe043b564d39a997c76c74c395d581f91697`；worktree clean，raw/cached diff 为空，
  `git diff --check` PASS。diff 精确为 LLVM setup script、专属 fixture、toolchain lock 三个批准路径，
  共 149 insertions；未修改 CI、六包集合或其他路径。
- lock 只允许 `llvm-18-dev`、version `1:18.1.3-1ubuntu1`、`amd64`、runner `ubuntu-24.04`，并精确绑定
  `libllvm18` 的 `Breaks: llvm-18-dev (<< 1:18.1.8~++20240730104741)`。manifest 必须只有这一条 Breaks、
  零 Conflicts，且六包集合仍精确为原集合。
- status parser 只消费 `install ok installed` paragraph；目标必须精确 version/architecture 且非 Essential。
  所有已安装包的 Depends/Pre-Depends 均扫描 exact name、版本关系、alternatives 与 `:any` qualifier；
  任何 reverse dependency 阻断 removal。pre-audit 非空、未知 conflict、version/arch/shape drift 均 fail。
- removal plan 只能为空或精确单项 `llvm-18-dev`。生产顺序为 pre-audit clean → identity/reverse-dependency
  proof → 恰好一次 `dpkg --no-act --remove llvm-18-dev` → 恰好一次 `sudo dpkg --remove llvm-18-dev` →
  absence + audit clean → system dependency proof → 原六个 deb 验证 → 恰好一次六包 dry-run/install →
  再次 absence + audit clean。
- setup 脚本无 apt/apt-get、GPG、keyserver、sources.list、dynamic resolver 或 fallback；未扩展六包。
- **I1：** `runner` 虽在 lock/expected dict 中精确为 `ubuntu-24.04`，但生产脚本没有读取
  `/etc/os-release`、`ImageOS` 或其他实际宿主身份，也没有 OS mismatch fixture。因此 removal 的
  destructive boundary 未真实限制到 Ubuntu 24.04。
- WSL Ubuntu 26.04 原生 LF clone fresh：LLVM fixture PASS、两个 Bash 文件 `bash -n` PASS、TOML
  identity/six-package check PASS、CI policy 子门 PASS、调用次数与 forbidden resolver probe PASS；专属
  probe 输出 `runtime-os-binding=ABSENT`。fixture deb 的非 root owner warning 不影响断言。
- 按 brief 未执行实际 `dpkg --remove`/install、真实 Ubuntu 24.04 runner、业务或完整 CI。

## Contract Node focused Review

- **Verdict：PASS — C0 / I0 / M0；可在 `75c4fe0` 后 fast-forward 集成 `ead0806`。** LLVM runner
  candidate 的 I1 是另一独立分支 finding，不影响本候选 verdict。
- candidate `ead0806c6274f24aef9261d13958088b0426d165`，唯一 parent
  `75c4fe043b564d39a997c76c74c395d581f91697`；worktree clean，raw/cached diff 为空，
  `git diff --check` PASS。diff 精确为 CI workflow、既有 repo-policy fixture、toolchain lock 三个批准
  路径，共 92 insertions/1 deletion；无 Contract/Proto/generated、Cargo manifest/lock 或业务源码漂移。
- `[node]` lock 与 Contract job 同时精确绑定官方 URL
  `https://nodejs.org/dist/v22.17.0/node-v22.17.0-linux-x64.tar.xz`、size `30482736`、SHA-256
  `325c0f1261e0c61bcae369a1274028e9cfb7ab7949c05512c5b1e630f7e80e12`。
- Contract job 使用有界 `curl --retry 5 --retry-all-errors -fL`；下载后先校验精确 size/hash，再 xz 解压。
  同一 step 先 export frozen Node bin、写入 `GITHUB_PATH`，再验证 `node --version == v22.17.0`、执行
  `corepack enable` 与 `corepack prepare pnpm@10.12.4 --activate`。下一 gate step 的既有 preflight 仍
  精确要求 `corepack pnpm@10.12.4 --version == 10.12.4`。
- Contract job block 无 nvm、apt/apt-get Node、`latest` URL、dynamic resolver 或浮动版本；policy fixture
  对 workflow/lock 的 URL、size、hash、version、PATH 与 Corepack marker 做正负 mutation。
- Rust job 保留 `cargo build --workspace --all-targets --locked`，因此 contract crate 仍被编译；workspace
  test 增加 `--exclude ficant-contract-tests`，与既有 acceptance/storage integration 排除并列，避免重复。
  Contract gate 保留唯一 `cargo test --locked -p ficant-contract-tests` 调用，crate inventory 精确 11 个 test。
- fixed actionlint 1.7.12 PASS。WSL Ubuntu 26.04 原生 LF clone fresh：repo-policy fixtures PASS、CI policy
  子门 PASS、fixture Bash 语法 PASS、TOML/workflow/Rust ownership/11-test 结构 PASS。
- official artifact fresh 下载 size/hash PASS；解压后 Node `v22.17.0`，隔离 `COREPACK_HOME` 下 prepare
  并执行 pnpm `10.12.4` PASS；临时 artifact、extract 与 Corepack cache 已由 trap 清理。
- 按 brief 未运行完整 Contract gate、业务、Docker 或完整 CI。

## LLVM runner successor focused Review

- **Verdict：PASS — C0 / I0 / M0**。
- successor `e9b146edd4e4d966fa9c84c91aeabe5ad3b43952` / parent
  `5992c2d2fd8ff049781ccc4e0a46f1dcb35e793d`；successor 精确修改 setup 与既有 fixture 两个路径，
  overall `75c4fe0..e9b146e` 仍只有 setup、fixture、LLVM lock 三个批准路径。
- `verify_host` 同时要求 lock `preinstalled_conflict.runner = ubuntu-24.04`，并严格解析 UTF-8
  os-release，精确要求 `ID=ubuntu`、`VERSION_ID=24.04`、`VERSION_CODENAME=noble`；duplicate、
  malformed、缺失或不同发行版/版本均 fail-closed。
- production `--install` 只有一处字面调用 `verify_host /etc/os-release`，无 env/path override；调用先于
  `mktemp`、首个 curl、remove plan、dry-run/remove 与 install。显式 fixture CLI 只在专用参数下读取
  传入路径，不改变 production path。
- Ubuntu 26.04 实机调用 production `--install` 在下载/变更前失败并输出 unsupported host；未执行
  实际下载、卸载或安装。
- WSL 原生 clean clone：LLVM fixture PASS；setup/test `bash -n` PASS；CI policy 风险子门 PASS；
  `git diff --check` clean。fixture 覆盖 Ubuntu 24 正向、Ubuntu 26、Debian、缺失文件、malformed quote
  负向，并保持既有 exact conflict、reverse dependency、audit、唯一 dry-run/remove/install 约束。
- 未运行完整 CI、业务测试或实际安装；本次结论只批准两提交 LLVM runner recovery 候选。

## Supply recovery focused Review

- **Verdict：BLOCK — C0 / I1 / M0**。
- candidate `2f8ce723ff882fac4622f20ce7d82054efde2336` / parent
  `75c4fe043b564d39a997c76c74c395d581f91697`；clean、diff-check 正确，精确修改 8 个 supply fixture/
  gate、Python manifest/lock 与 Rust 测试路径，无生产业务源码或 Cargo.lock 漂移。
- Python manifest/uv.lock 精确升级 pytest `8.4.1→9.0.3`；Ubuntu 24.04 使用 frozen lock 与系统
  Python 3.12.3，pytest 9.0.3、protobuf 6.33.5 import PASS。先前固定 OSV 证据标注的两条 pytest
  advisory 均以 9.0.3 为 fixed version；本次未重新运行完整 offline OSV scan。
- vulnerability evidence 生成时绑定 `schema_version=1`、HEAD candidate、三个固定 lock path、各自
  release-tree SHA-256 与 result count；Web `results=[]` 由对应 scan ledger 的 count 0 表达已执行，
  不再误判为漏扫。fresh mutation 证明 bad schema、缺 scan、ledger/result count mismatch 均拒绝。
- Rust 唯一变化把 secret-like 字符串替换为普通非法 trace；Ubuntu 24.04 exact
  `safe_trace_context_accepts_only_exact_lowercase_hex32` 1/1 passed，原业务负向语义保持。
- Cargo.lock 未改；async-std 经 minio 与 rsa 的既有 no-fixed advisories 继续可见，未新增 ignore、
  exception 或伪关闭。
- I1：current-tree dir scan 与 `-1 candidate` history scan 无法覆盖未声明可信的祖先历史。旧 fake fixture
  本身不是 secret，但其历史存在说明必须先冻结 trusted base/发布拓扑，不能据此宣称发布历史 clean。
- WSL 原生 clean clone：gate fixture tests PASS；supply/gates Bash 语法与 diff-check PASS。未运行 full
  CI、真实 Supply 工具/OSV/license scan、Docker 或业务全套。

## Supply topology successor focused Review

- **Verdict：PASS — C0 / I0 / M0**。successor `87eaabb2d40c53c9006cf0273472573ca969cd45` /
  parent `2f8ce723ff882fac4622f20ce7d82054efde2336`；worktree clean、diff-check PASS，successor 精确
  24 个 supply lock/gate/fixture 路径，overall 相对 `75c4fe0` 保留 parent 的 Python/Rust 修复且无回退。
- frozen trusted base `42f570f309e20c867f65cffbce76e7f6d64d65d5` 对象存在，且自身只有 2 个历史提交。
  production 在任何工具获取前要求 candidate object 存在、唯一 parent 精确等于 trusted base，且
  `base..candidate` commit count 精确为 1；merge、多提交、wrong parent、missing base 均 fail-closed。
- 固定 Gitleaks 8.28.0/SHA 分别运行：`--log-opts=$trusted_base` 覆盖已发布 base 完整历史，
  `--log-opts=$trusted_base..$candidate` 覆盖唯一候选范围，dir scan 覆盖由 candidate tree materialize 的
  release tree。无 `--all`、`-1`、floating ref、ignore/allowlist/exclude 旁路。
- release provenance 精确绑定 trusted base、candidate、parent、candidate tree、commit count；同时绑定
  OSV-Scanner/Syft/Gitleaks 三个工具的 name/version/SHA，以及 base/range/tree 三份报告的 scope、文件名、
  SHA-256 与 finding count。vulnerability evidence 同时绑定 candidate commit/tree 与三个 lock ledger。
- Ubuntu 24.04 clean clone：gate fixture tests PASS；锁定 Gitleaks artifact hash/version PASS；release
  topology fixture PASS，覆盖 pass、base drift、multi-commit、merge/wrong-parent、base-history secret、
  range secret、tree secret 与 missing base。额外 report mutation 被 provenance mismatch 拒绝。
- 当前 `87eaabb` 的真实 gate 在预工具阶段因 parent 不是 `42f570f` 而拒绝，符合 D-025；最终 Delivery
  必须先把批准内容 squash 为 `42f570f` 的唯一子提交，再运行真实 Supply gate。不得将本 focused
  fixture verdict 代替最终 candidate 的真实扫描。
- parent 的 pytest 9.0.3 manifest/uv.lock 与 Rust safe-trace fixture blob 均 byte-identical；OSV ledger
  schema/count/result 逻辑保留。未运行 full CI、真实最终候选 Supply/OSV/Syft/license、Docker 或业务。

## D-026 license closure focused Review

- **Verdict：BLOCK — C0 / I2 / M0**。exact range
  `fe336948038a0a6fcf1eb8c831e965c9e93589df..dfa33aca4cc2846a52807eb7740a91af6b599000`
  包含 `8a71d8b`、`c9a2a7f`、`dfa33ac`；candidate/parent、clean worktree 与 diff-check 正确，范围精确
  20 个 license/supply fixture、gate、inventory 与中文 NOTICE 路径。
- tracked inventory 自洽声明 Syft 1.46.0/hash、620 unique packages、13 first-party-internal 与 607
  third-party；两集合按 exact name/version/purl/source 分区且无交集。first-party authorization 精确为
  `internal-no-open-source-grant`，source locator/integrity 绑定 release-tree 内容，无 prefix/regex 授权。
- 但锁定 Syft artifact（SHA `d654f678...b5ca`）对 candidate archive 的 production 同命令实扫得到
  628 artifacts、626 relevant entries、624 unique keys、2 duplicate。额外 unique 是四个 fixture package，
  duplicate 是 `rsa`/`sqlx-mysql`；实际 inventory verify 输出 `duplicate Syft package key`。I1 开放。
- third-party inventory 的 primary locator/integrity 与 Cargo/uv/pnpm 三 lock hash 均由 verifier 重算；
  package missing/extra/duplicate/key/source/lock drift fixture fail-closed。该机制设计通过，但 tracked
  inventory 当前不能消费实际 production Syft 输出。
- SPDX parser 实际验证 OR 可选择、AND 全部满足、括号优先级、WITH exact 与 malformed fail-closed。
  `r-efi` 的 `MIT OR Apache-2.0 OR LGPL-2.1-or-later` 可由 MIT/Apache 满足；LGPL-only 与
  `MIT AND LGPL` 均拒绝，不把 LGPL 加入 allowlist。
- CDLA-Permissive-2.0/CC-BY-4.0 不在 global allowlist，只按当前精确 purl/name/version/license/
  source locator/integrity 生效；version/source drift 与继承失败。三份锁定 asset 的 license/NOTICE 文本
  integrity、text SHA 与中文 attribution 重生成后和 tracked NOTICE byte-identical。
- async-std 1.13.2 是唯一 `accepted-unfixed`，精确绑定 crates.io source integrity、minio 0.4.0
  reachability/checksum，reassess boundary 为 iteration-3 entry 或首次外发前较早者；1.13.3 不继承、
  `ignored` 状态拒绝，raw OSV findings 保留并由 evidence/provenance 绑定。
- Ubuntu 24.04 Cargo 1.96.1 real `tree --locked --all-features --target all`：322 reachable、62
  unreachable_lock_only；async-std/minio reachable，`rsa 0.9.10` 与 `sqlx-mysql 0.8.6` 仅 lock-unreachable。
  reachability fixture 对 forged set、mislabel、tool/config/graph/manifest/lock drift 均拒绝。
- inventory digest 不含 candidate SHA，避免自引用；runtime provenance 在生成后绑定 candidate/tree、
  inventory digest/file SHA/generator/NOTICE SHA、reachability 与 accepted-unfixed evidence。
- license、risk、reachability 专属 fixtures PASS，Python/Bash 语法与 NOTICE PASS；完整 gate fixtures 在
  risk fixture 直接执行处因 mode 100644 报 Permission denied，形成 I2。未运行 full CI、业务或 Compose。

## D-026 successor focused Review

- **Verdict：BLOCK — C0 / I1 / M0**。successor `f33c5229357d26cbd0ace035258fa18b8b46a771` /
  parent `dfa33aca4cc2846a52807eb7740a91af6b599000`；overall `fe336948..f33c522` 保留完整 license
  chain。worktree clean、successor 16 个 fixture/gate 路径、diff-check 正确。
- fixture Cargo/uv/pnpm locks 与 Cargo manifests 改为非标准 `.fixture` 文件，runner 只在 mktemp project
  中恢复标准名；candidate archive 的 fixture tree 没有可识别 Cargo.lock/Cargo.toml/uv.lock/
  pnpm-lock.yaml。production Syft 命令删除 exclude，保持原始 `scan dir:$release_root`。
- 锁定 Syft 1.46.0/hash actual candidate archive scan：622 total artifacts、620 relevant entries、620 unique、
  0 duplicate；与 tracked inventory package key 精确相等，first-party 13 与 third-party 607 partition
  无回退。scope fixture 同时证明普通 production Cargo.lock 被识别一次、`.fixture` 模板不被识别，
  没有 prefix/regex/泛化隐藏。
- 但 actual production inventory verify 仍失败：package/inventory digest 一致，lock header 不一致。
  tracked Cargo/uv SHA 是 CRLF worktree hash，candidate archive 的 LF hash 不同，`input_tree_digest` 随之
  漂移；因此 tracked inventory 不是 archive-native mechanical output，candidate/tree provenance 无法建立。
- `run-gates-tests.sh` 已显式 `bash risk-acceptance/run.sh`；Ubuntu 24.04 license/risk/reachability 专属
  fixtures与完整 gate fixture 全部 PASS，无 Permission denied。前次 I2 关闭。
- NOTICE 三个锁定 source asset/text SHA/中文 attribution 验证 PASS；SPDX、scoped exception、
  accepted-unfixed/raw finding 逻辑无变更。Cargo 1.96.1 actual all-features/target-all：322 reachable、
  62 lock-only，async-std/minio reachable，rsa/sqlx-mysql lock-only。
- inventory package digest 无 candidate SHA 自引用；runtime provenance 代码仍绑定 candidate/tree/
  inventory/NOTICE/reachability/risk evidence，但因 header drift 未能对 actual archive 建立成功证据。
- 未运行 full CI、业务或 Compose。

## D-026 final successor focused Review

- **Verdict：PASS — C0 / I0 / M0**。candidate `0fab0cc9792019dd883c0707ab54d5c0a1078c29` /
  parent `f5711066975097a82ffad503ccde655b92d378eb`；overall `fe336948..0fab0cc` 为
  `8a71d8b+c9a2a7f+dfa33ac+f33c522+f571106+0fab0cc` 六提交完整 license chain。worktree clean、
  diff-check PASS。
- `.gitattributes` 精确冻结根 Cargo.lock 与 `python/uv.lock` 为 LF；candidate archive native hashes 为
  Cargo `0920f796...85d3`、uv `866c8707...ad8`、pnpm `c6187a35...c04`。tracked inventory header 与
  `input_tree_digest=b7b68063...a62e3` 精确匹配，`--require-native-lf` 防止非 archive-native 输入。
- fixed Syft 1.46.0/hash actual `git archive HEAD` 无 exclude `scan dir:`：622 total artifacts、620 relevant、
  620 unique、0 duplicate；tracked keys 精确相等，607 third-party 与 13 first-party-internal 精确分区。
- 基于 actual Syft keys 与 archive locks 重算 canonical inventory 得到 222017 bytes，与 tracked 文件
  byte-identical；inventory digest `49355da3...a3cc3` 保持。exact candidate
  `0fab0cc...` / tree `5ed671e...` 的 runtime provenance verify PASS，inventory 本身无 candidate SHA
  自引用循环。
- repo-policy 只精确允许 `.github/scripts/verify-cargo-reachability.py`、
  `.github/scripts/verify-license-inventory.py`、`.github/scripts/verify-risk-acceptance.py`；没有 glob、prefix
  或目录级放行。fixture 对 `.github/scripts/foo.py` 与 `root-tool.py` 均 RED，原 path/secret/language/CI
  规则无回退；Ubuntu 24.04 repo-policy fixtures 与 `--stage final` PASS。
- license/risk/reachability 专属 fixtures、完整 gate fixtures、Python compile/Bash 语法、Syft scope 与
  NOTICE 三 asset/text/中文 attribution 均 PASS。SPDX、scoped exceptions、accepted-unfixed/raw findings
  语义无回退。
- Ubuntu 24.04 Cargo 1.96.1 actual all-features/target-all：322 reachable、62 lock-only；async-std/minio
  reachable，rsa/sqlx-mysql lock-only。
- 本 verdict 批准完整 license chain 进入最终 squash/发布候选验证；未运行 full CI、业务或 Compose。

## Supply secret successor focused Review

- **Verdict：PASS — C0 / I0 / M0**。candidate `4d1b6ee65096464af4da35e582af7285284b7e03` /
  parent `0fab0cc9792019dd883c0707ab54d5c0a1078c29`；worktree clean、diff-check PASS，精确只修改
  `.github/scripts/tests/fixtures/release-topology/run.sh`，43 insertions/3 deletions。
- GitHub Actions run `29189267286`（head `04437e0`）真实 supply job 为 RED；下载其持久
  `ficant-supply-evidence` artifact 后，`secrets-base.json` 为 0，`secrets-range.json` 为 3，
  `secrets-dir.json` 为 3；六个 finding 的 RuleID 均为 `generic-api-key`，与旧 tracked literal 对应。
- successor tracked archive 不再包含完整 Gitleaks-recognized secret literal；name/value 被拆为普通机械片段，
  这些片段单独无 credential 语义。successor 未修改 supply gate、Gitleaks lock/rule、`.gitignore`、
  allowlist、exclude 或扫描 scope。
- runtime fixture 在临时 repo 中拼接测试值，并在调用 gate 前直接使用 fixed Gitleaks 8.28.0 验证：
  ancestor/base-history、candidate range、dirty tree 三类各精确 1 finding，RuleID 必须为
  `generic-api-key`；任一 detector 失效或 finding 数量/规则漂移均使 fixture RED。
- Ubuntu 24.04 fixed Gitleaks artifact hash/version PASS，release-topology fixture PASS；其余 topology
  pass、base drift、multi-commit、merge/wrong-parent、missing base 约束保持。
- 以 trusted base `42f570f309e20c867f65cffbce76e7f6d64d65d5` 为 parent、successor tree 为内容构造 fresh
  单提交 `d4734d8`：topology gate PASS；base full history、base..candidate range、materialized candidate
  tree 三份报告均 0 findings。这证明 fixture 本身不污染待发布 tree/history。
- Ubuntu 24.04 完整 gate fixtures、repo-policy fixtures、repo-policy final、相关 Bash 语法与 diff-check
  均 PASS；D-026 license inventory/risk/reachability/NOTICE 代码未改，无回退。
- 未运行 full CI、业务或 Compose。

## Repro Node focused Review

- **Verdict：PASS — C0 / I0 / M0**。candidate `68ec891f137d3d48c10f2256a1521f83ba914680` /
  parent `0fab0cc9792019dd883c0707ab54d5c0a1078c29`；worktree clean、diff-check PASS，精确修改
  `.github/workflows/ci.yml` 与既有 repo-policy fixture，共 48 insertions/3 deletions。
- GitHub run `29189267286`（head `04437e0`）Repro job 真实日志：pnpm 报 unsupported engine，项目 expected
  Node `22.17.0`、runner got `v22.23.1`，随后 reproducibility Web build a 以 native exit 1 失败；问题定位
  与 candidate 修复边界一致。
- Repro install step 使用 Contract 已冻结且先前验证的官方 artifact：
  `https://nodejs.org/dist/v22.17.0/node-v22.17.0-linux-x64.tar.xz`、size `30482736`、SHA-256
  `325c0f12...e80e12`。workflow 中该 URL 精确出现两次，分别属于 Contract 与 Repro。
- Repro 顺序静态复核：有界 curl → exact size → SHA → xz extract → export PATH → GITHUB_PATH →
  `node --version == v22.17.0` → `corepack enable` → `corepack prepare pnpm@10.12.4 --activate` → 下一
  step 执行 reproducibility gate。所有 Node 建立动作均先于 Web install/build；无 nvm、Node apt、latest URL、
  dynamic resolver 或 floating version。
- repo-policy fixture 复用 frozen Contract Node checker 并针对 Repro 独立构造 URL、size、hash、version、
  PATH 与 pnpm activation 漂移负例，全部 RED；Contract job 原 checker/负例保持 PASS，合同无回退。
- fixed actionlint 1.7.12 Windows/amd64 PASS；official archive SHA-256
  `6e7241b51e6817ea6a047693d8e6fed13b31819c9a0dd6c5a726e1592d22f6e9` 与 release checksum 一致。
  Ubuntu 24.04 repo-policy fixtures、CI 子门、final stage、Bash 语法与自定义顺序探针均 PASS。
- 未运行 Repro build、业务、完整 CI 或 Compose；本 verdict 只批准 Node mismatch closure。

## License authority successor focused Review

- **Verdict：PASS — C0 / I0 / M0**。candidate `1ab894aff269c29712ebe7afbd1e435d0f40371b` /
  parent `4d1b6ee65096464af4da35e582af7285284b7e03`；candidate branch 包含此前链，successor 精确只改
  `.github/scripts/verify-supply-chain.sh`、gate fixture 与 risk fixture，worktree clean、diff-check PASS。
- GitHub run `29189731911` 的 supply job 是真实 RED：secret reports 已全部为 0 后，仍在约 620 个
  Syft `NOASSERTION` 许可证上阻断；修复边界与失败原因一致。该 run 审查时尚未整体结束，因此不把
  其余并行 job 状态当作本 focused verdict 的证据。
- production 继续输出并保留 raw `packages.syft.json` 与 `sbom.cdx.json`。完整 inventory verifier 只从
  raw Syft 取得 exact `(purl,name,version,ecosystem)` universe，同时以 Cargo/uv/pnpm locks、supply lock、
  release root 验证 tracked inventory；Syft 的 license 字段不参与授权。
- evidence 验收第一步必调完整 `verify-license-inventory.py verify`，参数包含 inventory、raw Syft、三类
  lock、supply lock、release root、first-party，并在 production release-root 路径要求 native LF。
  随后的 provenance 再精确比较 inventory digest、inventory 文件 SHA-256 与 generator；仅计算 digest
  的早期步骤不能绕过完整验收，production 最终只调用一次统一 `verify_evidence`，无 double-skip。
- tracked inventory 为 complete、620 packages、620 unique purls，生态精确为 crates.io/PyPI/npm。
  Ubuntu 24.04 gate fixtures 以全 620 个 `NOASSERTION` raw Syft artifacts 验证正常 PASS；向 raw Syft
  首项注入 `GPL-3.0-only` 仍 PASS，证明 scanner 字段不能覆盖 inventory authority。
- license fixtures 保持 missing、extra、duplicate、key drift、unknown、disallowed、source-integrity、
  lock drift、first-party source/missing、scoped-version 与 provenance candidate/tree/inventory 负例，
  全部按预期 RED；risk acceptance fixture 与 supply high-vulnerability/secret/malformed/tool/db 语义无回退。
- Ubuntu 24.04 干净 clone：license fixtures、risk fixtures、完整 gate fixtures、repo-policy fixtures、
  repo-policy final、相关 Bash syntax、Python compile 与 diff-check 全部 PASS。
- 未运行 full CI、业务或 Compose；本 verdict 只批准 license authority closure。

## Rust deadline focused Review

- **Verdict：PASS — C0 / I0 / M0**。candidate `eeacb00ac2250fd67790ae69a149191a2d280cbb` /
  parent `3745c169fa8043eb8f4c5ddea9eaeb6d5db08379`；精确只改
  `binaries/ficant-bootstrap/src/lib.rs`，89 insertions/4 deletions，所有差异均位于 `#[cfg(test)]` module；
  production request deadline、request read、response write 与 socket close 代码未改，diff-check PASS。
- GitHub run `29189731911` rust job 的真实失败位于旧 slow-drip test 第 485 行
  `client.read_to_end(...).expect("response reads")`，精确错误为 Linux `ConnectionReset` / os error 104；
  server deadline 本身没有报错。旧 parent 的本地 Linux 单次重跑可 PASS，进一步表明这是 TCP 收尾
  时序的非确定性观测，而非 production deadline 退化。
- 根因：客户端在 40ms 间隔继续慢滴，server 在约 200ms absolute deadline 返回 408 后关闭仍可能有
  未读请求数据的 socket；Linux 可用 RST 收尾，客户端因此可能得到完整、截断或零字节响应后再见
  `ConnectionReset`。零字节在 reset 分支可接受，是因为同一 test 仍要求 server handler 成功返回并
  join，production `write_response` 任一写/flush 错误都会先使 server thread 失败；它不是通用空响应放行。
- test-only helper 明确区分 `CleanEof` 与 `PeerReset`：CleanEof 继续调用原 `assert_response` 验证完整
  status/content-length/body；PeerReset 只接受 `ConnectionReset` 且要求已收到字节是完整期望 408 的
  exact prefix；`BrokenPipe` 等无关错误返回 Err。三个确定性 helper tests 分别覆盖 reset-after-bytes、
  reset-before-bytes 与 unrelated error。
- slow-drip 主合同未削弱：drip thread 与 server thread 均必须 join，handler result 必须成功，elapsed
  仍必须 `<= 200ms + 150ms = 350ms`；没有 retry、sleep 放宽、blanket ignore、timeout 增长或 production
  fallback。Linux 10 次 fresh candidate 运行均 PASS，elapsed 约 201.75–202.61ms。
- Ubuntu 24.04 固定发行版当前无 Rust 工具链，因此按已授权的升/降一级规则使用 CI 同一固定
  Rust 1.96.1 Linux image。确定性 helper tests 3/3、slow-drip 10/10、bootstrap lib 12/12 PASS；临时
  worktree 与验证 volumes 已清理。
- 未运行 full workspace、业务或 Compose；本 verdict 只批准 test semantics closure。

## Web race focused Review

- **Verdict：PASS — C0 / I0 / M0**。candidate `36f6f0877d68da0b745ce8cf36aeef18674419ee` /
  parent `b7599edc83ff0acd75c77bbd75d2d45609def29e`；精确只改
  `web-dm/platform-shell/src/loader.tsx` 与 `tests/states.test.tsx`，26 insertions/2 deletions，clean、
  diff-check PASS。
- GitHub run `29190374218` 已完成 failure；Web job 的唯一 Vitest failure 在原 states 用例：
  `fireEvent.error(frame)` 后 `findByRole("alert")` 超时，失败 DOM 多次仍为
  `data-shell-state="app-ready"`，28 tests 中 27 passed。失败边界与 candidate 修复精确一致。
- 根因链：iframe 已在 React commit 中进入 DOM，但原生 `error` listener 直到 passive `useEffect` 才安装；
  commit 后、passive effects 前发生的 native error 会永久丢失。candidate 仅将同一个 listener lifecycle
  改为 `useLayoutEffect`，在浏览器绘制与 passive effects 前同步安装。
- cleanup 仍用同一 `frame.removeEventListener("error", onLoadError)`；credential post effect、load handoff、
  boundary validation、app error callback、alert/返回列表状态与 revoke 流程均未改。没有 sleep、retry、
  timeout 调整、act suppression、waitFor 包裹或断言放宽。
- 新确定性 TDD fixture 让 sibling layout effect 在同一 commit 后立刻派发 native iframe error。把该 test
  放回旧 parent loader 时精确 RED：`expected spy to be called once, but got 0 times`；candidate 精确 PASS，
  证明修复的是 layout/passive phase 差异，而非等待时长。
- Ubuntu 24.04 固定 Node `v22.17.0` / pnpm `10.12.4` / Vitest `3.2.6`：新 focused test PASS；原
  “加载失败隔离”用例连续 6 次正常 PASS，随后 WSL wall clock 明确倒跳 10–17 秒，导致约 276ms 用例
  被 Vitest 误判为 5000ms timeout；`/proc/uptime` 同期单调，故不计为产品 finding。时钟稳定窗口的
  full suite 为 4 files / 29 tests 全 PASS，typecheck PASS。
- 未运行 Playwright、full CI 或业务；本 verdict 只批准 iframe native-listener race closure。

## License refresh focused Review

- **Verdict：PASS — C0 / I0 / M0**。candidate `4a8fd93731f1e3000e1d9c76092bf6fe2215d3d4` /
  parent `b7599edc83ff0acd75c77bbd75d2d45609def29e`；精确修改 tracked inventory、既有 license fixture 与
  inventory verifier 三路径，clean、diff-check PASS。
- GitHub run `29190374218` Supply 真实失败为 `ficant-bootstrap` first-party binding mismatch；该 run 已
  包含 reviewed Rust deadline test change，因此 release-tree source hash 改变而旧 inventory 未刷新，
  与 candidate 边界一致。
- base/candidate inventory 机械比较：package keys 均为 620 且完全相同，分类仍为 607 third-party +
  13 first-party-internal；唯一 package value diff 是 `pkg:cargo/ficant-bootstrap@0.1.0` 的
  `source_integrity` 从 `33ca0909...7e446` 更新为 `bed1b920...a64626`。license expression、locator、
  classification、authorization 与其余 619 packages 均未变化。
- header 只更新 generator v1→v2、`input_tree_digest` 为 `e3d3c684...5a153`、`inventory_digest` 为
  `d079e25a...3e8d`。v2 将所有 first-party `(purl, source_locator, source_integrity)` 排序后纳入 input
  digest；这是把 source change 额外 fail-closed 绑定到 header，不是 constraint relaxation。fixture
  证明只改内部源码会同时改变 input/inventory digest，keys 与其他 package 不变。
- fixed Syft 1.46.0 artifact SHA-256 `d654f678...b5ca`，对 actual candidate `git archive` 无 exclude 扫描：
  620 relevant / 620 unique，精确等于 tracked keys。以 607 frozen third-party records + 13 actual
  release-tree first-party finalize，regenerated JSON 与 tracked inventory byte-identical；完整 verifier
  在 candidate archive locks 上以 `--require-first-party --require-native-lf` PASS。
- candidate `4a8fd93...` / tree `e96dffac...` provenance digest binding PASS；inventory file SHA-256
  `390ac1c8...89e7`，generator v2 与 runtime exact binding 一致。NOTICE verification PASS。
- Ubuntu 24.04 license fixtures、risk/accepted-unfixed fixtures、完整 gate fixtures、repo-policy fixtures/
  final、Python compile 与 Bash syntax 全 PASS。NOASSERTION authority、SPDX allowlist、scoped exception、
  first-party、source-integrity、provenance、accepted-unfixed/raw evidence 负例无回退。
- 未运行 full CI、业务或 Compose；本 verdict 只批准 license inventory refresh/strengthening。

## MinIO volume focused Delivery Review

- **Verdict：PASS — C0 / I0 / M0**。candidate `af0197c299a7baf9a92f4fe129e9849bdca89601` /
  parent `911edeaf1cc58f15d72ed37900c495ee28e93438`；六路径差异为最小 MinIO Dockerfile、Compose/lock、
  精确 repo allowlist 与 compose-security gate/tests，clean、diff-check PASS。
- 真实 root cause 复现：固定官方
  `minio/minio@sha256:a1ea29fa...b015e` 为 linux/amd64，`Config.User=""`、`Volumes={"/data":{}}`；
  容器内 `/data` 精确为 `0:0 0755`。fresh named volume 由 UID/GID `1000:1000` 挂载后，`touch /data/...`
  精确 `Permission denied`；不是 MinIO credential、healthcheck 或 Compose DAG 问题。
- `deploy/dev/Minio.Dockerfile` 只从 exact RepoDigest 构建，build-time 临时 `USER 0:0` 执行
  `mkdir/chown 1000:1000/chmod 0700 /data`，最终显式 `USER 1000:1000`。没有 entrypoint/init root、
  host chmod、privileged、cap_add、setuid helper 或 runtime chown。
- Docker/BuildKit 真实构建后，新 volume 的首次 copy-up 保留 `/data=1000:1000`；read-only rootfs、
  `cap_drop=ALL`、`no-new-privileges` 下 UID/GID 1000 可写并删除 smoke file，随后 MinIO `server /data`
  成功启动且 live endpoint PASS，runtime `id=1000:1000`、volume root `1000:1000`。
- Compose 对 MinIO 仅将直接 base image 改为 frozen local runtime image + 本地 build contract + exact
  `MINIO_IMAGE` arg；service `user=1000:1000` 保持。共享 `read_only:true`、`cap_drop:ALL`、
  `no-new-privileges:true`、CPU/memory/PID、tmpfs、127.0.0.1 port、named volume 与 health/DAG 均未改变。
  resolved compose security gate PASS。
- OCI labels 精确记录 base RepoDigest、`AGPL-3.0-only` 与 runtime title；toolchain lock 记录官方 source
  tag、RepoDigest、linux/amd64 manifest、local runtime image、Dockerfile、runtime user/license。repo-policy
  baseline 只精确增加 `deploy/dev/Minio.Dockerfile`，无目录/glob 放行。
- 派生层不新增第三方 package，只修改同一固定 MinIO base 的目录 ownership/metadata；既有 repository
  SBOM package universe 与 supply/license inventory 因此不变。base provenance 与 AGPL delivery metadata
  由 Compose resolved/runtime gate fail-closed 校验。
- compose-security 单元测试 23 项 PASS（普通运行中 live test 按设计 skip）；独立 live targeted test
  1/1 PASS。Ubuntu 24.04 repo-policy fixtures/final、Python compile、Bash syntax PASS。测试前后匹配
  `ficant-minio-runtime-test-*` 的 containers/volumes/images 均为 0，临时 volume/image/container 无残留。
- 未运行完整七服务 Compose、full CI 或业务；本 verdict 只批准 MinIO fresh-volume ownership closure。

## Optional env focused Delivery Review

- **Verdict：PASS — C0 / I0 / M0**。candidate `87db3897d82b0bea4e35eee3595178f366bbf041` /
  parent `af0197c299a7baf9a92f4fe129e9849bdca89601`；精确只改 `deploy/dev/docker-compose.yml`、
  compose-security gate 与既有 tests 三路径，clean、diff-check PASS。
- 旧 parent 真实 RED 已本地复现：宿主未设置 bootstrap vars 时，`${VAR:-}` 将 subject/token/scopes 全部
  解析成 `""` 并注入容器；ficant-server 立即以 `invalid server configuration: bearer credential must
  not be empty` 退出。Rust `env::var`/BTreeMap/bearer identity 代码未改，显式空值仍是 `Some("")`，
  不会被解释为 `None`。
- candidate 只把三个 mapping 改为 YAML null pass-through：`FICANT_BOOTSTRAP_SUBJECT:`、
  `FICANT_BOOTSTRAP_BEARER_TOKEN:`、`FICANT_BOOTSTRAP_SCOPES:`。unset resolved values 为 null，实际
  container `Config.Env` 三个 key 均缺席；configured subject+token+scopes 则逐值精确、非空透传。
- security gate 新增“omitted/null 或 non-empty”合同：显式三个空字符串 RED，subject-only、token-only、
  scope-only 与 scope 无 subject+token pair 均 RED；完整 pair/可选 scopes PASS。配置宿主
  `FICANT_LOOPBACK_*` 与无关 secret sentinel 时，它们均未进入 resolved/container environment。
- live targeted Compose 先构建同一 ficant-server image，再分别启动 unset 与 configured 两个 `--no-deps`
  容器；两者 runtime health 均 healthy。unset Config.Env 无 bootstrap keys；configured 精确为
  `fixture-subject` / `fixture-bearer-token` / `apps:read,rates:read`。
- required signing/trace keys、config mount、public bind、exact CORS origin、loopback ports、read-only rootfs、
  cap_drop ALL、no-new-privileges、CPU/memory/PID/tmpfs、healthcheck 与 Compose DAG 均未修改。Rust auth、
  default identity 与 token policy无变更，也未增加 fake/default token。
- resolved security gate 的 unset 与 configured 场景 PASS；24 个 compose-security unit tests PASS（两个
  live tests 在普通运行按设计 skip）；Ubuntu 24.04 repo-policy fixtures/final 与 Python compile PASS。
  parent RED 与 candidate live test 完成后，各自匹配前缀的 containers/volumes/images 均为 0。
- 未运行完整七服务 Compose、full CI 或业务；本 verdict 只批准 optional bootstrap environment closure。

## Final license refresh focused Review

- **Verdict：PASS — C0 / I0 / M0**。candidate `9f044b796a912746df2080c5d42bf696797c4424` /
  parent `73384fec150c1929a2e28f79549ad21c4ec8bc57`；精确只改
  `.github/scripts/license-inventory.lock.json` 单文件一行 canonical JSON，clean、diff-check PASS。
- GitHub run `29192844481` 审查时除仍在执行的 reproducibility 外，其余已完成 jobs 中仅 Supply RED；
  Rust/Web/Python/Contract/CPP/Migration/Business/repo-policy 均 success。Supply failure 被定位为最终 tree 中
  `ficant-node-runtime` first-party source binding mismatch，与本 candidate 唯一变更边界一致。
- base/candidate inventory 机械比较：620 keys 完全相同，分类仍为 607 third-party + 13
  first-party-internal；唯一 package value diff 是 `pkg:pypi/ficant-node-runtime@0.1.0.source_integrity`
  从 `sha256:9cd2...39b24` 更新为 `sha256:8b27d68b...7c12b`。其余 619 packages、所有 license
  expressions、source locators、classification/authorization 与 first-party policy 均 byte-equivalent。
- header 仅随该 source binding 更新：`input_tree_digest=a538ac4b...d5d56`、
  `inventory_digest=fcbaecdd...b0ba7`；generator 保持 v2，三个 lock hashes、Syft tool/version/hash、
  schema/status 与 package key universe不变。inventory file SHA-256 为 `6417277b...3d108`。
- fixed Syft 1.46.0 artifact hash `d654f678...b5ca`，对 actual final candidate `git archive` 无 exclude 扫描：
  620 relevant/620 unique，精确等于 tracked keys。以 607 frozen third-party + 13 actual release-tree
  first-party finalize，regenerated JSON 与 tracked inventory byte-identical；完整 verifier 在 archive locks
  上以 `--require-first-party --require-native-lf` PASS。
- candidate `9f044b7...` / tree `2d1fa3a...` provenance digest binding PASS，runtime exact binding 使用上述
  inventory digest/file SHA/generator。NOTICE PASS。
- Ubuntu 24.04 license fixtures、risk/accepted-unfixed fixtures、完整 gate fixtures、repo-policy fixtures/
  final、Python compile 与 Bash syntax全部 PASS。NOASSERTION authority、SPDX、scoped exceptions、
  first-party/source-integrity、accepted-unfixed/raw finding、provenance 约束无回退。
- 未运行 full CI、业务或 Compose；本 verdict 只批准 final license inventory mechanical refresh。

## Validity

Valid：iteration-2 Final license refresh focused Review；直到最终 squashed candidate 实跑或权威决策取代。
