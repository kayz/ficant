FROM node@sha256:b04ce4ae4e95b522112c2e5c52f781471a5cbc3b594527bcddedee9bc48c03a0 AS build

WORKDIR /workspace/web-dm
RUN corepack enable && corepack prepare pnpm@10.12.4 --activate
COPY web-dm/package.json web-dm/pnpm-lock.yaml web-dm/pnpm-workspace.yaml web-dm/.npmrc ./
COPY web-dm/platform-shell/package.json platform-shell/package.json
RUN pnpm install --frozen-lockfile
COPY web-dm/packages/contracts-generated packages/contracts-generated
COPY web-dm/platform-shell platform-shell
ENV FICANT_UI_BASE_PATH=/ficant/
ENV VITE_FICANT_GRPC_WEB_BASE_URL=/ficant-api
RUN pnpm build

FROM nginx@sha256:b5a9a3cfc86b81dd6f1458d53f2ad2b559cc255653d2efd6cacee5636bf2066f

COPY deploy/test/ui/nginx.conf /etc/nginx/nginx.conf
COPY --from=build /workspace/web-dm/platform-shell/dist /usr/share/nginx/html/ficant
USER 101:101
EXPOSE 8080

