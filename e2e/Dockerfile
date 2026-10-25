# The e2e runner: Playwright's image already carries Chromium, Firefox and
# WebKit for this exact version, so no browser cache step is needed. The
# repository is the build context (the web build needs `web/`).
FROM mcr.microsoft.com/playwright:v1.64.1-noble

RUN corepack enable

WORKDIR /app
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY web/package.json web/
COPY packages/plugin-sdk/package.json packages/plugin-sdk/
COPY e2e/package.json e2e/
RUN pnpm install --frozen-lockfile

COPY . .

WORKDIR /app/e2e
# The test run is the container's exit code (`docker compose ... --exit-code-from e2e`).
CMD ["bash", "-lc", "pnpm exec playwright test"]
