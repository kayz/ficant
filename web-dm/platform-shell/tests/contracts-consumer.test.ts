import { create } from "@bufbuild/protobuf";
import {
  AppDescriptorSchema,
  PlatformService,
} from "../../packages/contracts-generated/src/ficant/app/v1/registry_pb";
import { SessionSchema } from "../../packages/contracts-generated/src/ficant/app/v1/session_pb";
import { describe, expect, it } from "vitest";

describe("Q2-CTR-03 TypeScript 生成契约消费", () => {
  it("直接导入生成 schema 与 service descriptor，不复制 DTO", () => {
    const app = create(AppDescriptorSchema, {
      appId: "consumer-proof",
      displayName: "契约消费证明",
      entrypoint: "/consumer-proof",
      allowedOrigin: "https://apps.example.invalid",
    });
    const session = create(SessionSchema, { sessionId: "session-proof" });

    expect(app.$typeName).toBe("ficant.app.v1.AppDescriptor");
    expect(session.$typeName).toBe("ficant.app.v1.Session");
    expect(PlatformService.typeName).toBe("ficant.app.v1.PlatformService");
  });
});
