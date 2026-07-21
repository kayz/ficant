# 本地版本镜像构建收口

## 目标

- 在精确基线 `dc108ef1150d2dfdd05665258542d328f4a69638` 上修复 `ficant-server` 版本镜像缺失 C++ 固收内核源码的构建上下文。
- 在本机 Docker 上分别构建 `ficant-server`、`ficant-worker`、`ficant-web` 和 `ficant-ui` 四类冻结镜像合同，不向 registry 推送。

## 验收

- `ficant-server` 镜像可以编译 `ficant-kernel-sys` 所需的全部 `cpp/fixed-income-kernel` 源码并完成 release build。
- 四类最终运行镜像均在本地成功构建；UI 使用 `cicd.yml` 指定的正式 Dockerfile，Rust 服务使用同版本 Linux 工具链离线编译并按正式 runtime stage 组装。
- Rust 服务镜像中的 `/usr/local/bin/ficant --help` 或对应健康探针可启动；UI 镜像保留非 root 用户与固定路径合同。
- `./scripts/check-fast.ps1` 在最终候选上 exit code 0。

## 非目标

- 不修改 GitHub workflow、版本 tag 触发条件、镜像提升、扫描、部署或回滚合同。
- 不登录 GHCR、不 push 镜像、不创建或移动版本 tag、不部署测试环境。
- 不改变业务 API、C++ 数值实现、Protobuf、数据库 migration、Oracle、expected、断言或容差。

## 公共契约变化

- 无业务公共契约变化。
- `deploy/dev/RustService.Dockerfile` 的 builder context 新增仓库 `cpp/`，与 `ficant-kernel-sys/build.rs` 的相对路径合同一致。
- 仓库根新增 `.dockerignore`，排除 Git 元数据、本地 Rust/C++ build 输出、Web `node_modules` 和 Python 虚拟环境，避免本机产物进入版本镜像 context。

## 需 Human 决策

- 当前无待决业务语义或版本号决策；本迭代明确不授权版本交付。

## 最终真实测试证据

- `wsl.exe -d ficant-ubuntu-24.04 -- bash -lc '... cargo build --offline --locked --release --bin ficant-server --bin ficant-worker --bin ficant-web ...'`：exit code 0；Rust `1.96.1`、Cargo `1.96.1`、clang++ `18.1.8`，三个 Linux release binary 在 1m31s 内完成。
- `docker build --pull=false --file deploy/test/FicantUi.Dockerfile --tag ficant-local/ficant-ui:iteration2 .`：exit code 0；`pnpm --frozen-lockfile` 安装 178 个包，Vite production build 完成 166 个模块。
- 三个离线编译的 Rust binary 按正式 `debian:bookworm-slim@sha256:...` runtime stage 分别组装为本地镜像；四类镜像 ID 为 server `c95bd8201a88`、worker `eabe3bfbbdc2`、web `31d4ec8d8993`、UI `37cc5fa953e5`。
- `docker run --rm --entrypoint /bin/sh <rust-image> -ec 'id -u; test -x /usr/local/bin/ficant-entry'`：三类 Rust 镜像均 exit code 0，UID 均为 `1654`。
- `docker run --rm --entrypoint /bin/sh ficant-local/ficant-ui:iteration2 -ec 'id -u; test -f /usr/share/nginx/html/ficant/index.html; nginx -t'`：exit code 0，UID `101`，固定静态路径存在，nginx 配置有效。
- `docker build --pull=false --build-arg BINARY=ficant-server --file deploy/dev/RustService.Dockerfile ...`：已成功执行新增的 `COPY cpp ./cpp`，随后 Cargo 稀疏索引连续 300 秒网络超时；本命令未完成，未计为通过。
- `./scripts/check-fast.ps1`：exit code 0；工作区非环境测试与 doc tests 全部通过，storage library 3/3、Phase 3A 5/5、Phase 3B 2/2。

## 残余风险

- 本地 Docker build 只能证明 build context 和镜像运行闭包，不替代版本 tag 触发的 Linux CI、在线供应链扫描、GHCR 不可变制品或目标环境证据。
- 受本机 Docker Hub/稀疏索引网络超时影响，三个 Rust 服务未在正式多阶段 Dockerfile 内完成冷构建；本地已证明 C++ 源码进入 builder context、同版本 Linux 工具链可离线完成 release 编译、正式 runtime 合同可运行，但下一个获授权版本候选仍须由 Linux CI 完整验证该 Dockerfile。
