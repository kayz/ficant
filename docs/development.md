# 本地开发说明

## 工具链最低要求

日常开发在 Windows 11 + PowerShell 7 上进行，无需启动外部数据库或对象存储即可完成全部编译、单元测试与实时 SDK 联调：

- **PowerShell 7**（`pwsh`）
- **Rust `1.96.1`**（由根目录 `rust-toolchain.toml` 固定）
- **LLVM / Clang++ `>= 18.0`**、**CMake**、**Ninja**（编译 `cpp/fixed-income-kernel`）
- **Buf `1.56.0`**（校验与生成 `interface/` Protobuf 契约及编码 `domain-packs/`）
- **uv `>= 0.7`** 与 **Python `3.12`**（运行 `python/` SDK 与 `tests/oracle/china-rates/` 独立数值对账）

## 常用命令

查看检查步骤（不实际执行）：

```powershell
.\scripts\check-fast.ps1 -ListOnly
.\scripts\check.ps1 -ListOnly
```

提交前快速检查：

```powershell
.\scripts\check-fast.ps1
```

完整本地回归（含严格 Clippy、RulePack 二进制漂移校验、C++ 内核 CTest、跨编译器数值一致性及全部独立 Decimal Oracle）：

```powershell
.\scripts\check.ps1
```

本地启动单进程服务（同时监听原生 gRPC 与浏览器 gRPC-Web，默认 `127.0.0.1:50051`，可通过环境变量 `FICANT_SERVER_ADDR` 覆盖）：

```powershell
.\scripts\dev-up.ps1
```

重新生成多语言契约（修改 `interface/proto/**` 后执行）：

```powershell
buf format -w interface
buf lint interface
buf generate interface --template interface/buf.gen.yaml
```
