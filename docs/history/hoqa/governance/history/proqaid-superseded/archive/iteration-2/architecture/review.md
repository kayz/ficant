# Architecture Review：iteration-2，round-4

## Decision

**`approve-invalid-value`**

批准 `DomainErrorCode::InvalidValue` 为内部第九个稳定领域错误码。它不改变 `ficant.core.v1.ErrorCode`；对外唯一映射固定为 `ERROR_CODE_VALIDATION_FAILED` / gRPC `INVALID_ARGUMENT` / `retryable=false`。

## Reasoning

八个既有专用码分别承载 ID、Unit、时间、版本、哈希、血缘、状态机和 Journal 顺序。必填文本、普通正值/范围、集合非空/去重、bid/ask 关系和规范 Decimal 值并不属于这些概念。移除 `InvalidValue` 只有两种结果：把通用值错误伪装成专用错误，或为每个字段扩展外部错误枚举。两者都比一个内部、严格限定的 generic value code 更损害 Ubiquitous Language 和契约稳定性。

候选增加该 variant 的问题是缺少事前批准，不是该语义本身错误。本决定只批准词汇和边界，不自动接受候选每个使用点。

## Allowed Boundary

`InvalidValue` 只适用于无法归入专用八码的普通领域值不变量：

- blank/required text；
- 正值、非负值或普通数值范围；
- Unit 已合法后的 Decimal coefficient/scale/precision；
- 必需集合为空、重复或普通组合关系；
- bid/ask、至少一侧存在等非状态/非时间/非血缘跨字段关系。

以下必须优先返回专用码：invalid ID → `InvalidId`；unit/dimension → `InvalidUnit`；timezone/date/effective interval → `InvalidEffectiveTime`；version/revision → `VersionConflict`；hash → `ContentHashMismatch`；lineage → `BrokenLineage`；lifecycle transition → `InvalidStateTransition`；journal sequence → `JournalSequenceConflict`。

## External Mapping

| Layer | Frozen result |
|---|---|
| Domain | `DomainErrorCode::InvalidValue` |
| Application classification | validation failure |
| Proto | `ficant.core.v1.ERROR_CODE_VALIDATION_FAILED` |
| gRPC | `INVALID_ARGUMENT` |
| Retry | `false` |
| Detail | safe message + `trace_id` + applicable `field_violations[]` |

Domain 不依赖 Proto/tonic。application/API 必须显式穷尽 mapping，不使用 wildcard 把未来 code 静默归入 validation。不得新增 `ERROR_CODE_INVALID_VALUE`，也不得把内部 variant 名作为客户端稳定字符串。

## Required Tests

1. 精确锁定内部九码 enum 集合。
2. 表驱动证明 blank text、非正值、空集合、bid > ask、非法 Decimal 表示返回 `InvalidValue`。
3. 表驱动证明八个专用边界优先，不会退化为 `InvalidValue`。
4. application/API mapping 精确证明 `VALIDATION_FAILED`、`INVALID_ARGUMENT`、non-retry 与 field detail。
5. Proto descriptor 证明没有新增 `INVALID_VALUE` enum。
6. 完整 domain regression 与 Review R3-I-01/R3-I-03 修复证据仍须独立通过。

## Review Finding Disposition

- R3-I-02 的 Architecture 决策依赖已关闭；W2 仍需按本边界修正/复核使用点和测试，再交 Review。
- R3-I-01、R3-I-03 不受本决定影响，Task 3 仍是 `blocked`。
- 本轮不裁决 candidate integration、Task 4 dispatch、Task 3 quality 或 iteration readiness。

## Evidence Checked

- Architecture round-4 inbox、current context 与 round-3 review。
- Review round-3 `R3-I-02` finding/outbox。
- candidate `a2b7d7c` 的 `DomainErrorCode`、全部 `InvalidValue` 使用点和专用八码使用点。
- `interface/proto/ficant/core/v1/error.proto` 的冻结外部 ErrorCode。
- current implementation plan Task 3 八码草案与 Architecture error mapping table。

## Runtime Policy

目标 GPT-5.6 Terra/high；实际 runtime/model/reasoning 未获 attestation，记为 **unverified**。

## Validity

Valid: iteration-2 only
