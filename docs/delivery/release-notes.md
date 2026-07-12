# 交付发布说明

## 候选与证据绑定

- GitHub Actions `ci` run [`29193249268`](https://github.com/kayz/ficant/actions/runs/29193249268) 于 2026-07-12 完成，repo-policy、contract、python、migration、rust、cpp、business-loop、supply-chain、web、reproducibility 十个 gate 均为 `success`。其候选为 `ef96c5edea11b0d5f6ebc693501f40a9b40df061`，树为 `2d1fa3a1be11e563c486d7c67df349ec06faf4d0`。
- 最终 integration commit 为 `9f044b796a912746df2080c5d42bf696797c4424`，树同为 `2d1fa3a1be11e563c486d7c67df349ec06faf4d0`。此前 run `29191239090` 是较早发布树的成功检查点，不再作为当前最终 CI 结论。
- 最终 Compose 验收基线为 commit `87db3897d82b0bea4e35eee3595178f366bbf041`、树 `e8fb65c5a86bac93382e93e50c90926954e4298f`。该基线到最终树之间仅许可证清单和证据文档发生变化，Compose 配置、镜像构建配置及运行时安全检查 blob 未变化，因此下述 live Delivery 证据适用于当前最终树；它与最终 CI 证据互补而不互相替代。

## 前两轮阻断与关闭

- 第一轮在树 `3baed89aa1791bd32a9d19d5ab244d6c279c1ef1` 上失败：MinIO 用户 `1000:1000` 无法写入新命名卷 `/data`。修复 `af0197c299a7baf9a92f4fe129e9849bdca89601` 使用锁定基础镜像构建 hardened MinIO runtime image，并在镜像层建立所有者为 `1000:1000` 的 `/data`。
- 第二轮在包含上述修复的树 `6e5898bf2db9c0dca5f29b4a8d73a9f06163de68` 上失败：Compose 将未配置的 `FICANT_BOOTSTRAP_*` 变量作为空字符串注入，server 返回 `bearer credential must not be empty`。修复 `87db3897d82b0bea4e35eee3595178f366bbf041` 在未配置时从容器环境中省略这组三个可选变量。
- 两轮失败后均执行了带卷和 orphan 的完整清理；本轮预检确认项目资源为零。

## 最终 Docker/Compose 验收

- 环境：Docker Desktop Linux `linux/x86_64`，kernel `6.18.33.2-microsoft-standard-WSL2`；Docker client/server `27.5.1`；Docker Compose `v2.32.4-desktop.1`。
- 项目名为 `ficant-iteration-2`，配置为 `deploy/dev/docker-compose.yml`，profile 为 `dev`。凭证、签名密钥和 trace 密钥仅通过验收进程环境变量注入，未写入仓库或本文。
- resolved gate 命令为 `docker compose ... config --format json | python python/compose_security_gate.py resolved --project ficant-iteration-2`，结果为 `PASS`。
- 唯一启动命令为 `docker compose ... up -d --build --wait --wait-timeout 600`。七服务 DAG 完整启动：PostgreSQL、MinIO、ficant-server、ficant-worker、ficant-web 均健康；migration 与 minio-init 均以退出码 0 完成。
- live inspect 通过 `python/compose_security_gate.py runtime --project ficant-iteration-2`：七个容器均为非 root、只读根文件系统、`CapDrop=ALL`、无 `CapAdd`、启用 `no-new-privileges`、仅回环端口绑定，并具有冻结的 CPU、内存、PID、tmpfs、只读挂载、镜像来源和健康检查。
- MinIO live 检查确认进程 UID/GID 和新卷 `/data` 所有者均为 `1000:1000`，可创建、读取并删除写入标记；其 HTTP live probe 通过。PostgreSQL `pg_isready` 通过，八个 migration 版本可读。
- 最小交付 smoke 通过：server 容器内 readiness probe 成功；worker 和 web 的 health/readiness HTTP 路径返回 `ok`；server gRPC-Web CORS preflight 从允许的精确 origin 返回 HTTP 204。未重复 Quality 的完整业务套件。
- 对五个常驻服务执行一次 restart 后，五者均重新达到 healthy；PostgreSQL 测试行和 MinIO 卷写入标记均可读取，证明重启后的持久性。测试表与写入标记在销毁前已主动删除。

## 清理证据

- 已执行 `docker compose ... down --volumes --remove-orphans --rmi local`。
- 复核 `ficant-iteration-2` 项目标签及 `ficant/minio-runtime` 镜像名范围：容器、网络、卷、构建镜像、测试数据和项目 cache tag 均为零。
- 主机上的其他 Compose 项目未被停止、删除或改写。

## 当前交付判断

当前已验收树的 Docker/Compose 专项结论为 `PASS`，前两轮 runtime blocker 均已由真实证据关闭。用户随后明确授权跳过剩余 Review；状态必须记录为 `Review skipped by explicit human authorization`，不得伪造 `Review pass`。全部确定性门与发布拓扑验证完成后，iteration-2 以 `closed-with-human-approved-review-deviation` 关闭。

## Human-approved Review deviation

- 状态：`Review skipped by explicit human authorization`。
- 既有 Review 证据继续有效，但不再追加新的 Review 轮次或等待 Review verdict。
- 该偏差不豁免 CI、真实业务、Migration、数据完整性、Supply、secret、许可证、漏洞、Compose 安全或 D-025 单提交发布拓扑门。

## 有效期

有效至当前候选树、Compose 配置、镜像锁或运行时合同发生变化。
