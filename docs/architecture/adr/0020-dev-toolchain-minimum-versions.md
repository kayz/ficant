# ADR-0020：开发机只检查工具链与最低版本

- 状态：Accepted
- 日期：2026-10-21
- 决策者：Human
- 范围：`scripts/check-fast.ps1`、`scripts/check.ps1` 及其调用的本地检查脚本
- 部分取代：ADR-0009 中"本地检查要求与 CI 完全一致的工具版本并离线运行"的要求

## 背景

本地检查原先要求每个工具与 CI 精确一致，包括小版本和构建标识：cargo/rustc `1.96.1 (… 2026-06-26)`、Visual Studio Build Tools LLVM `19.1.5`、uv `0.7.13 (…)`、Node `v22.17.0`、Buf `1.56.0`，并且 cargo、uv、corepack 全部离线运行。

结果是换一台机器、工具自动更新一个补丁版本，或第一次运行时本机缓存不全，检查就在跑任何测试之前失败。失败信息说的是"版本不对"或"缓存缺失"，而不是代码有问题。这些失败不提供关于代码正确性的信息，却占据了大部分本地排障时间。

真正需要精确版本的是可复现的发布制品，而发布制品只由 CI 在 Linux Runner 上构建。

## 决策

**开发机：** 只确认工具存在、属于正确的工具族、不低于最低版本：

| 工具 | 最低版本 | CI 锁定 |
|---|---|---|
| cargo / rustc | 1.96 | 1.96.1 |
| clang++ | 18 | 19.1.5（Windows），18.1.8（Linux） |
| uv | 0.7 | 0.7.13 |
| Node | 22 | 22.17.0 |
| pnpm（经 corepack） | 10 | 10.12.4 |
| Buf | 1.56 | 1.56.0 |

检查时打印实际版本和 CI 锁定版本，便于对照。依赖仍按锁文件解析（`--locked`），但不再强制离线：本机缺少的 crate、wheel 或 pnpm 可以从包源取回。脚本仍不安装工具本身。

C++ 编译器依次取 `FICANT_CXX`、Visual Studio Build Tools 自带 LLVM、`PATH` 上的 `clang++`。Cross-Clang 数值一致性检查需要 WSL 中的 Linux clang；缺少时打印 SKIPPED 警告并跳过，设 `FICANT_REQUIRE_CROSS_CLANG=1` 时改为失败。比较逻辑本身不变。

**不变的部分：**

- `rust-toolchain.toml`、`Cargo.lock`、`python/uv.lock`、`pnpm-lock.yaml`、`packageManager` 照旧锁定；
- `.github/**` 中 CI、供应链和可复现性脚本的精确版本校验不变；
- 契约包经 corepack 使用 `packageManager` 锁定的 pnpm 10.12.4；本地契约包测试比较的是两次全新打包的结果是否一致，不依赖 Buf 的精确版本，因此 Buf 同样只要求 ≥ 1.56。将来正式发布契约包时由 CI 用锁定工具链构建；
- `scripts/check-manual.ps1` 是 MANUAL 走查执行器，保持离线安装；
- 预期值、Oracle 与容差不因工具版本而调整。

## 代价

- 本地通过不再代表"与 CI 同一工具链下通过"。数值或生成物在不同补丁版本下出现差异时，本地可能先通过、CI 再失败，反之亦然。处理方式是用锁定版本复现，再判断是不是代码问题；不得为某个本地版本修改 expected。
- Cross-Clang 检查在没有 WSL 的机器上会跳过。跳过不能写成数值一致的证据。

## 替代方案

- 维持精确版本：拒绝。维护成本高，且失败不说明代码问题。
- 本地也完全不检查版本：拒绝。工具族或大版本不对时（例如 MSVC 代替 clang、Node 18），失败会出现在更晚、更难读的位置。
