import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { PlatformShell } from "./app";
import { createGrpcWebPlatformClient } from "./registry";
import "./styles.css";

const root = document.getElementById("root");
if (!root) throw new Error("platform shell root is missing");

const baseUrl = import.meta.env.VITE_FICANT_GRPC_WEB_BASE_URL || window.location.origin;
const client = createGrpcWebPlatformClient(baseUrl);

createRoot(root).render(
  <StrictMode>
    <PlatformShell client={client} transport="grpc-web" />
  </StrictMode>,
);
