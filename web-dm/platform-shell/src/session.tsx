import type { Timestamp } from "@bufbuild/protobuf/wkt";
import type { Session } from "../../packages/contracts-generated/src/ficant/app/v1/session_pb";

const EXPIRING_WINDOW_MS = 60_000;

export type SessionTiming = "valid" | "expiring" | "expired" | "invalid";

export function timestampMilliseconds(value: Timestamp | undefined): number | undefined {
  if (!value) return undefined;
  return Number(value.seconds) * 1_000 + Math.floor(value.nanos / 1_000_000);
}

export function classifySession(session: Session, now: Date): SessionTiming {
  const issuedAt = timestampMilliseconds(session.issuedAt);
  const expiresAt = timestampMilliseconds(session.expiresAt);
  if (!session.sessionId || !session.subjectId || issuedAt === undefined || expiresAt === undefined || expiresAt <= issuedAt) {
    return "invalid";
  }
  const remaining = expiresAt - now.getTime();
  if (remaining <= 0) return "expired";
  if (remaining <= EXPIRING_WINDOW_MS) return "expiring";
  return "valid";
}

export function sessionExpiryLabel(session: Session): string {
  const expiresAt = timestampMilliseconds(session.expiresAt);
  if (expiresAt === undefined) return "未知";
  return new Intl.DateTimeFormat("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  }).format(new Date(expiresAt));
}
