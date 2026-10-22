# ADR-0021：发布文件由 ficant 自己拥有

- 状态：Accepted
- 日期：2026-10-23
- 决策者：Human
- 范围：`.github/workflows/{ci,release-test,prepare-storage-runtime}.yml`、`cicd.yml`、`deploy/**`
- 修改：ADR-0009 与 ADR-0019 中"中央 `kayz/cicd` 是受管发布文件的源"的表述

## 背景

中央 `kayz/cicd` 原先规定：`ficant/managed/` 是发布工作流、测试机 Compose 和部署脚本的权威来源，ficant 仓库里的同名文件只是从中央某个不可变版本生成的副本，禁止在业务仓库修改后回写。

实际情况是：

- R9A–R9G 的发布修复直接改在 ficant 的副本里，中央没有同步。到 2026-10-21，`release-test.yml` 和 `compose.test.yml` 两边各差 26 行，中央 `cicd.yml` 还保留着 ficant 已去掉的 `ficant-web` 镜像。按规则从中央重新安装，会覆盖掉已在 `v0.1.0-alpha.12` 上部署成功的修复。
- dmquant 已废弃，ficant 是中央唯一的受管项目，"中央生成、多处复用"已经没有复用对象。
- ficant 在运行时不引用中央仓库：工作流里没有 `uses: kayz/cicd/...`，中央生成只发生在人工运行安装脚本的时刻。

## 决策

ficant 仓库中的发布文件就是权威来源，直接在 ficant 修改、审阅和验证。

中央 `kayz/cicd` 只保留与具体应用无关、属于测试机层面的内容：

- 测试机初始化（`ficant/server/bootstrap.*`）与宿主机 Nginx 片段；
- GitHub Secrets、Environment 与 GHCR 的配置说明；
- 可能被多个项目复用的工具（如 `cicd.yml` 的 JSON Schema 与校验脚本）。

中央删除 `ficant/managed/`、`ficant/cicd.yml` 与安装脚本，以及整套 dmquant 发布配置。

**不变的部分：**

- 发布触发与授权：只有 Human 创建指向 `main` 精确提交的不可变 `v*` tag 才进入版本 CI 与测试环境交付；失败的 tag 不移动，修复后用新版本号。
- `.github/**`、`cicd.yml`、`deploy/**` 仍是受保护路径，修改需要 Human 明确同意；它们的修改走普通 PR，PR 描述写明影响的发布行为。
- 发布制品只由 Linux GitHub Runner 构建，以 Commit SHA 标识；测试机只拉镜像、不编译。

## 代价

- 失去"一份模板供多个项目生成"的能力。将来再有项目上同一台测试机时，从 ficant 的文件复制起步，或届时再抽取 Reusable Workflow，而不是恢复生成副本的模式。
- 测试机层面的变更（Nginx、目录、部署账号）仍在中央，应用层面的变更在 ficant，跨两者的改动需要两个 PR。

## 替代方案

- 以 ficant 当前文件为准回写中央，继续"中央生成"：拒绝。只有一个受管项目，维护两份并保持一致的成本没有对应收益，本次漂移就是这种成本的结果。
- 迁移到中央 Reusable Workflow：暂不。需要先在中央重写并版本化 ficant 的发布流程，收益要等第二个项目出现才有。
