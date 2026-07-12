import { create } from "@bufbuild/protobuf";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import {
  ErrorCode,
  GetAppRegistryResponseSchema,
} from "../../../packages/contracts-generated/src/ficant/app/v1/registry_pb";
import { PlatformShell } from "../../src/app";
import {
  fixtureClient,
  launchGrant,
  longFixtureApp,
  longFixtureSafeMessage,
  longFixtureTrace,
  registry,
  safeFailure,
} from "./platform";
import "../../src/styles.css";

const root = document.getElementById("root");
if (!root) throw new Error("fixture root is missing");

const scenario = new URLSearchParams(window.location.search).get("scenario");
const client = scenario === "long"
  ? fixtureClient({
    getAppRegistry: async () => registry([longFixtureApp]),
    authorizeAppLaunch: async () => launchGrant({}, longFixtureApp),
    refreshAppLaunch: async () => launchGrant({}, longFixtureApp),
  })
  : scenario === "long-error"
    ? fixtureClient({
      getAppRegistry: async () => create(GetAppRegistryResponseSchema, {
        result: {
          case: "error",
          value: safeFailure(ErrorCode.UNAVAILABLE, longFixtureSafeMessage, longFixtureTrace, true),
        },
      }),
    })
    : fixtureClient();

createRoot(root).render(
  <StrictMode>
    <PlatformShell client={client} now={() => new Date()} />
  </StrictMode>,
);
