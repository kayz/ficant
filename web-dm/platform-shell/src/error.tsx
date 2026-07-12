import { useState } from "react";
import {
  ErrorCode,
  type SafeError,
} from "../../packages/contracts-generated/src/ficant/app/v1/registry_pb";

interface SafeErrorPanelProps {
  error: Pick<SafeError, "code" | "safeMessage" | "traceId" | "retryable">;
  onRetry?: () => void;
  retryLabel?: string;
}

export function errorCodeLabel(code: ErrorCode): string {
  return `ERROR_CODE_${ErrorCode[code] ?? "UNSPECIFIED"}`;
}

export function SafeErrorPanel({ error, onRetry, retryLabel = "重试" }: SafeErrorPanelProps) {
  const [copyState, setCopyState] = useState("");

  async function copyTrace() {
    if (!error.traceId) return;
    await navigator.clipboard?.writeText(error.traceId);
    setCopyState("追踪编号已复制");
  }

  return (
    <section className="error-panel" role="alert" aria-labelledby="safe-error-title">
      <p className="eyebrow">安全错误</p>
      <h2 id="safe-error-title">{error.safeMessage}</h2>
      <dl className="error-facts">
        <div><dt>错误代码</dt><dd>{errorCodeLabel(error.code)}</dd></div>
        <div>
          <dt>追踪编号</dt>
          <dd>{error.traceId || "未提供"}</dd>
        </div>
      </dl>
      <div className="action-row">
        {error.traceId ? <button type="button" className="secondary-action" onClick={copyTrace}>复制追踪编号</button> : null}
        {error.retryable && onRetry ? <button type="button" className="primary-action" onClick={onRetry}>{retryLabel}</button> : null}
      </div>
      <span className="visually-hidden" aria-live="polite">{copyState}</span>
    </section>
  );
}
