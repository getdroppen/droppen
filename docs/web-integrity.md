# 网页完整性：droppen.app 上的网页是不是开源构建

> SPA 完整性的第一阶段（方案 a）：可复现构建、公开哈希、验证脚本。背景见 `docs/cross-machine-architecture.md` §7 的「网页前端投递」一行。

droppen.app 的网页由闭源的 Hub 下发。端到端加密防得住 Hub 读数据，防不住 Hub 下发一段改过的 JS：那段 JS 能用这台浏览器的设备钥，解开发给它的一切，还能把这台浏览器存着的私钥和 `UK` 读出来带走（WebCrypto 的「不可导出」挡不住同源代码）。本文说明怎样检查 droppen.app 下发的网页就是开源仓库 `web/` 的构建，以及这项检查证明不了什么。

## 1. 它证明什么，不证明什么

**能证明：** 检查的那一刻，droppen.app 发给验证脚本的每个文件，与某次发版构建出来、经 GitHub Actions 证明（attestation）过的文件逐字节相同。那次构建又能由任何人从源码重建出同样的字节。

**不能证明：**

- **别人、别的时候拿到的是什么。** 浏览器不校验它加载的 JS。Hub 可以只给某个账号、某个 IP、某一次请求下发恶意代码，对验证脚本照常下发干净的版本。验证脚本发现不了这种有针对性的、一次性的投递。检查越多、来源越分散，被盯上的代价越高，但永远不是保证。
- **已经装进浏览器的东西。** 网页带一个 Service Worker（`sw.js`）。恶意的一次投递可以在你的浏览器里留下缓存的恶意资源；之后的干净检查改变不了那台浏览器。
- **源码和依赖本身可信。** 可复现只说明「字节来自这份源码和这份 `pnpm-lock.yaml`」。依赖里的恶意代码会被如实复现出来。
- **Hub 的接口说了真话。** 那是端到端加密和设备链的事（`docs/hub-api/README.md`「The hub is not trusted」），与本文无关。

**高保障的做法**是不从 Hub 加载代码：用本机 `droppen` 二进制自带的界面（它内嵌同一份构建，发版时 CI 核对过两者字节相同），以后用签名的 Tauri 桌面应用。网页版适合方便，不适合对抗一个有针对性的 Hub。

## 2. 怎么验证

需要 Node 20 以上，核对证明还要装 GitHub CLI（`gh`）。不需要任何账号或令牌；脚本只访问被检查的站点和 GitHub 的公开发版（`gh` 还会取 Sigstore 的公开信任根），不发送 Cookie，User-Agent 是 `droppen-verify-web/1`。

```sh
node scripts/verify-web.mjs                       # 检查 droppen.app
node scripts/verify-web.mjs --release v0.2.109    # 指定对照哪次发版
node scripts/verify-web.mjs --site https://…      # 检查另一个 Hub
```

脚本做这几件事：

1. 问站点它跑的是哪个构建：`GET /.well-known/droppen-build` 返回提交和构建摘要（下一节）。这只是指路，不是证据。
2. 在最近的发版里找摘要相同的 `web-manifest.json`，用随发版附上的 Sigstore bundle 执行 `gh attestation verify`（有 bundle 时不用登录）。发版在 getdroppen/droppen（`--repo`）：从 v0.2.127 起，`release.yml` 构建、签名并发布每一次发版，清单和它的 bundle（`web-manifest.sigstore.json`）就在同一个发版上。v0.2.126 及以前的发版在 getdroppen/releases（`--repo getdroppen/releases`），清单由 `scripts/release-public.sh` 在本机生成，文件和摘要与 CI 签名的那份相同，字节却不同（CI 的还写了仓库名），而 bundle 签的是字节。所以发版没附 bundle 时，改从签名仓库（`--signer-repo`，默认 getdroppen/droppen）同一标签的发版上取 `release.yml` 挂的清单和它的 bundle：这份清单的摘要必须与发版的相同，站点的文件按它核对，证明也对它验。签名仓库那边没有（或取不到）算没验到证明；有、却是另一个构建的清单，或证明对不上，都算失败，`--allow-unattested` 也不放过。能匿名取到发版，说明仓库是公开的，而公开仓库的每次发版都有证明，所以**默认没验到证明就算失败**：没装 `gh`、或发版没附 bundle，都失败。确实要接受未证明的清单，加 `--allow-unattested`。用自己重建的清单（`--manifest`）时不要求证明，除非给了 `--bundle` 或 `--require-attestation`。
3. 下载清单里的每个文件，再取 `/`、`/link`、`/signin` 三个路由的页面外壳，逐个比较 sha256。外壳先去掉 Hub 加的唯一一处改动：`<head>` 紧后面的 `<meta name="rh-hub" content="<origin>" />`。只去掉这个确切的标签，别的差异照样算不同。
4. 这个标签的 origin 决定页面去哪里登录、所有 Hub 请求发往哪里，所以它必须是站点自己的（`https://droppen.app` 对应 `wss://droppen.app`），否则失败；别的 Hub 要用 `--hub-origin wss://…` 明确给出。字节相同、却指向别的 Hub 的页面，不算同一个页面。
5. 检查每个页面（各路由的外壳和清单里的每个 `.html`）响应头里的 CSP：`script-src`、`script-src-elem`、`script-src-attr`（以及兜底的 `default-src`）只许 `'self'`、`'none'` 和这些页面自己内联脚本的哈希。

结果是 `PASS` 或 `FAIL`，并列出不同的文件。退出码：0 通过，1 不通过，2 没能检查（网络、参数）。

### 不信 CI，自己重建

```sh
git clone https://github.com/getdroppen/droppen && cd droppen
git checkout <站点报告的提交或发版标签>
# Node 用 .nvmrc 里的版本（如 nvm use），pnpm 用 package.json 里的 packageManager
pnpm install --frozen-lockfile --filter "@remote-hand/web..."
pnpm --filter @remote-hand/web build
node scripts/web-manifest.mjs web/dist --out mine.json
node scripts/verify-web.mjs --manifest mine.json
```

要用干净的检出：`web/` 下多出的文件会被 Tailwind 扫到，改变 CSS。拿自己的构建对照发版清单：`node scripts/web-manifest.mjs web/dist --check web-manifest.json`。

不用 Node 也能算摘要：

```sh
cd web/dist && find . -type f | sed 's|^\./||' | LC_ALL=C sort | xargs sha256sum | sha256sum
```

## 3. 怎么做到的

**可复现构建。** Node 钉在 `.nvmrc`（22.23.3），Hub 的镜像构建网页时用同一版本；pnpm 是 `packageManager` 里的 10.6.3；依赖由 `pnpm-lock.yaml` 钉死。公开发行版不设 `DROPPEN_DISTRIBUTION`，只用仓库里的 `distribution.json`（不含插件）。构建产物里没有时间戳和本机路径；发版和 CI 仍设 `SOURCE_DATE_EPOCH`（提交时间），留给以后读时间的工具。

CI 的 `web-reproducible` 任务每次都检查：两个全新检出放在不同目录、不同时区分别构建，`web/dist` 必须逐字节相同。Hub 的镜像用同样的命令构建同一份网页，构建出的字节要与发版清单相同（这项检查在 Hub 的仓库里）。`web-reproducible-macos` 再在 macOS 上构建一次，必须与 Linux 的清单相同。发版时 `stage` 任务还核对：写清单的那次构建（macOS 上）与二进制内嵌的那份网页（`web` 任务在 Linux 上构建）逐字节相同。验证脚本自己的测试（`node --test scripts/verify-web.test.mjs`）也在这个任务里跑。

**清单。** 每次发版，`release.yml` 的 `web` 任务按 Hub 镜像构建网页时同样的命令构建，作为构建产物交给构建二进制的任务；`stage` 任务里的 `scripts/release-public.sh` 用同样的命令再构建一次，用 `scripts/web-manifest.mjs` 写出 `web-manifest.json`（[`docs/releasing.md`](releasing.md)）：

- `files`：每个文件的路径、大小、sha256；
- `digest`：构建摘要。每个文件一行 `<sha256>  <路径>\n`（即 `sha256sum` 的输出），按路径逐字节排序，整体再取 sha256；
- `commit`、`ref`、`repository`，以及构建输入：Node 和 pnpm 版本、`SOURCE_DATE_EPOCH`、`.nvmrc`、`package.json`、`pnpm-lock.yaml`、`distribution.json` 的哈希。

签名与构建分开。构建任务要跑依赖代码（安装、vite、cargo），所以没有签名权限（没有 `id-token`），也拿不到任何密钥。签名的 `attest` 任务只在真正发版时运行（试运行不生成证明），不装依赖、不编译，也不运行任何构建出来的程序，构建产物都先按产出它的任务给的 sha256 核对：它用 GitHub 自己的 `actions/attest-build-provenance` 为发版的每个文件（清单也在内）生成 SLSA 构建来源证明，再为清单单独生成一份；`publish` 任务把清单和这份 bundle（`web-manifest.sigstore.json`）连同发版的其他文件一起发布到本仓库同一标签的 GitHub Release 上。这样任何构建期依赖都拿不到能签证明的 OIDC 令牌。证明把清单绑定到这个仓库、`release.yml` 和发版标签。

**Hub 报告自己跑的构建。** Hub 启动时对它加载的文件（外壳取加标记之前的原样）算同一个摘要，在 `GET /.well-known/droppen-build` 返回 `{commit, web: {digest, files}}`（`docs/hub-api/http.md`）。提交是构建这个 Hub 时记下的那个。发版提交通常只改 Cargo 版本号，所以 Hub 部署的提交与发版标签不同也没关系：验证按摘要找清单，不按提交。

## 4. 现状和剩下的风险

- **v0.2.127 之前的发版没有证明。** 它们在发版用的 Mac 上构建，私有仓库也用不了构建证明，所以只附了未证明的 `web-manifest.json`；从 v0.2.127 起，每次发版都由 `release.yml` 构建并证明。对照更早的发版要加 `--allow-unattested`，或者拿自己重建的清单来比（`--manifest`）。
- **Hub 可能跑着没发过版的网页。** Hub 由运营者手动从选定的提交部署，`/.well-known/droppen-build` 报告的就是这个提交；它不一定是发版标签。如果这个提交里 `web/` 有还没发版的改动，站点的摘要就不在任何发版里，验证会失败。这时按站点报告的提交自己重建。只从发版标签部署 Hub 能彻底避免这种情况。
- **只核对列出的文件。** Hub 可以多放别的路径，但开源代码不会去加载它们，除非外壳或已核对的 JS 引用。外壳只抽查了三个路由；Hub 理论上能在别的路由上给出不同的外壳。
- **CSP 只检查脚本来源。** 其它指令和响应头不在证明范围内。
- **信任链的其余部分：** GitHub Actions、npm 注册表（锁文件带完整性哈希）、Node 镜像。可复现构建让人可以不信 CI，自己重建来比。

## 5. 以后

浏览器扩展在每次加载时核对；独立的静态源加版本化的不可变 URL；把各次检查的结果记到公开日志里；本机界面作为默认入口；签名的 Tauri 与手机应用。
