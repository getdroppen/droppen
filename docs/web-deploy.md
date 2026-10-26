# 部署静态网页（my. / www.）

`my.droppen.app`（app 角色）和 `www.droppen.app`（site 角色）是同一个仓库、
同一份源码构建出的两个 Cloudflare Worker。构建和发布由 **Cloudflare Workers
Builds** 完成：合并到 `main` 即自动构建并部署，不需要 GitHub Actions 里的
Cloudflare API token。

## 两个 Worker 的配置

| | `droppen-my` | `droppen-www` |
|---|---|---|
| Git 仓库 | `getdroppen/droppen` | `getdroppen/droppen` |
| 生产分支 | `main` | `main` |
| Root directory | `/` | `/` |
| Build command | 留空（部署脚本自己安装和构建） | 同左 |
| Deploy command | `pnpm deploy:web:my` | `pnpm deploy:web:www` |
| 构建变量 | `NODE_VERSION=22.23.3` | 同左 |
| Preview builds | 关（预览会跑 `wrangler preview`，不是生产命令） | 同左 |
| Build caching | 开 | 开 |

Root 必须是仓库根：`web/package.json` 依赖 workspace 包
`@droppen/plugin-sdk`，只有在根目录 `pnpm install` 才能解析。

## 命令在仓库里

`package.json`：

- `pnpm deploy:web:my` — 清理 `web/dist`、`pnpm install --frozen-lockfile`、
  以 `DROPPEN_WEB_MODE=cloud DROPPEN_WEB_ROLE=app` 和 `SOURCE_DATE_EPOCH`
  构建、用 `wrangler@4.152.0` 部署到 `droppen-my`；
- `pnpm deploy:web:www` — 同上，`DROPPEN_WEB_ROLE=site`，部署到 `droppen-www`。

两个命令各自完整，Workers Builds 的 Deploy command 填其中一个即可。
`.github/workflows/web-deploy.yml`（需要 `WEB_DEPLOY=true` 与 Cloudflare
secrets）调用同样的两个命令，作为手动/备用通道。

## 手动触发

Workers Builds 在 push 时构建。要强制重建而不改代码，可在 Cloudflare
Dashboard 的 Worker → Deployments 里用 Deploy Hook，或推一个空提交；本仓库
不使用这两种做法，正常合并即可。

## 自定义域名

两个 Worker 的 `wrangler.my.jsonc` / `wrangler.www.jsonc` 里已经有
`routes`（`custom_domain: true`）；连接 Git 时选**现有** Worker，域名不动。
