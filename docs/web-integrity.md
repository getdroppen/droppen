# 网页完整性：droppen.app 上的网页是不是开源构建

> SPA 完整性的第一阶段（方案 a）：可复现构建、公开哈希、验证脚本。背景见 `docs/cross-machine-architecture.md` §7 的「网页前端投递」一行。

droppen.app 的网页由闭源的 Hub 下发。端到端加密防得住 Hub 读数据，防不住 Hub 下发一段改过的 JS：那段 JS 能用这台浏览器的设备钥，解开发给它的一切，还能把这台浏览器存着的私钥和 `UK` 读出来带走（WebCrypto 的「不可导出」挡不住同源代码）。本文说明怎样检查 droppen.app 下发的网页就是开源仓库 `web/` 的构建，以及这项检查证明不了什么。

## 1. 它证明什么，不证明什么

**能证明：** 检查的那一刻，droppen.app 发给验证脚本的每个文件，与某次发版构建出来、经 GitHub Actions 证明（attestation）过的文件逐字节相同。那次构建又能由任何人从源码重建出同样的字节。

**不能证明：**

- **别人、别的时候拿到的是什么。** 浏览器不校验它加载的 JS。Hub 可以只给某个账号、某个 IP、某一次请求下发恶意代码，对验证脚本照常下发干净的版本。验证脚本发现不了这种有针对性的、一次性的投递。检查越多、来源越分散，被盯上的代价越高，但永远不是保证。
- **已经装进浏览器的东西。** 网页带一个 Service Worker（`sw.js`）。恶意的一次投递可以在你的浏览器里留下缓存的恶意资源；干净的新 Worker 装上后只用与它自己脚本里的哈希表相符的缓存，别的都删掉（§3「Service Worker 的缓存」）。恶意的 Worker 本身也会被换掉：放在 `/sw.js` 的，浏览器下次检查更新时就换成 Hub 当时下发的；在别的地址（如 `/x.js`）为 `/` 注册的，Droppen Hub 和本机 `droppen` 服务器对那个地址的 Worker 请求回一段清空缓存、注销自己的小 Worker（同一节）。但那次投递在运行时做了什么（读走钥匙、写进 IndexedDB 和 localStorage 的东西），之后的干净检查改变不了。
- **源码和依赖本身可信。** 可复现只说明「字节来自这份源码和这份 `pnpm-lock.yaml`」。依赖里的恶意代码会被如实复现出来。
- **Hub 的接口说了真话。** 那是端到端加密和设备链的事（`docs/hub-api/README.md`「The hub is not trusted」），与本文无关。

**高保障的做法**是不从 Hub 加载代码：用 Droppen 桌面应用（macOS，目前是 v0.2.128；用 Apple 的 Developer ID 签名并经 Apple 公证，安装后只接受更新密钥为那个版本签名的更新，见 [`docs/releasing.md`](releasing.md)「Verifying a download」），或本机 `droppen` 二进制自带的界面。桌面应用带着本机的 `droppen` 服务器，窗口里显示的是这个服务器内嵌的网页，不从 Hub 加载；`droppen` 二进制内嵌的网页，发版时 CI 核对过与发版清单逐字节相同（§3）。网页版适合方便，不适合对抗一个有针对性的 Hub。

**检查 droppen.app** 的三个主机——应用所在的 `my.droppen.app`、站点 `www.droppen.app` 和 `droppen.app` 本身，三者由同一个 Hub 下发同一份构建——用的是每次发版（v0.2.127 起）附带、经 GitHub Actions 证明的网页清单（`web-manifest.json` 和它的 Sigstore bundle `web-manifest.sigstore.json`），加上验证脚本 `scripts/verify-web.mjs`，步骤见下一节。droppen.app/security 的 7.1 和 8.5 节链接到这里。

## 2. 怎么验证

需要 Node 20 以上，核对证明还要装 GitHub CLI（`gh`）。不需要任何账号或令牌；脚本只访问被检查的站点和 GitHub 的公开发版（`gh` 还会取 Sigstore 的公开信任根），不发送 Cookie，User-Agent 是 `droppen-verify-web/1`。

```sh
node scripts/verify-web.mjs                       # 检查 my.droppen.app、www.droppen.app、droppen.app，对照最新发版
node scripts/verify-web.mjs --release v0.2.109    # 指定对照哪次发版
node scripts/verify-web.mjs --allow-older         # 站点还在跑较早的发版时，对照那次发版检查，不判失败
node scripts/verify-web.mjs --site https://a.example --site https://b.example   # 检查另一个 Hub：它的每个主机都给一次 --site
```

不给 `--site` 时依次检查 droppen.app 的三个主机，每个主机单独检查、单独给出 `PASS` 或 `FAIL`，最后列出汇总。一个 Hub 的几个主机要在同一次运行里一起检查：三个主机互相重定向（下面第 3 步），只查其中一个时，送往另一个主机的那些路径没人核对，这个主机就判失败。某个主机连不上时，它报「没能检查」，其余主机照常检查。

对每个主机，脚本做这几件事：

1. 问站点它跑的是哪个构建：`GET /.well-known/droppen-build` 返回提交和构建摘要（下一节）。这只是指路，不是证据。站点不回答、回答的不是 JSON、或者里面没有构建摘要时，照样按最新发版的清单逐个核对它的文件（结果里注明站点没说它跑的是哪个构建），对不上就判失败。
2. 取**最新发版**（`--repo` 在 GitHub 上标为 latest 的那次发版，也就是 `install.sh` 安装的那次）的 `web-manifest.json`，站点报告的摘要必须与它相同。每次发版的构建都留着证明，一个停在较早发版的站点（没部署、或者故意给旧构建）照样能逐字节对上那次发版、证明也对得上，却没有之后的修改；所以只对得上较早发版时判**失败**，并说出是哪次发版、最新的是哪次。确实要对照较早的发版，加 `--allow-older`（在最近 `--releases` 次发版里找，默认 30；结果里注明不是最新发版），或用 `--release` 指明。站点报告的提交与清单的不同不算失败：比对的是摘要（字节），提交只是 Hub 自己说的标签，Hub 也可能从发版标签以外、网页相同的提交部署（§3）。对这份清单用随发版附上的 Sigstore bundle 执行 `gh attestation verify`（有 bundle 时不用登录）。发版在 getdroppen/droppen（`--repo`）：从 v0.2.127 起，`release.yml` 构建、签名并发布每一次发版，清单和它的 bundle（`web-manifest.sigstore.json`）就在同一个发版上。v0.2.126 及以前的发版在 getdroppen/releases（`--repo getdroppen/releases`），清单由 `scripts/release-public.sh` 在本机生成，文件和摘要与 CI 签名的那份相同，字节却不同（CI 的还写了仓库名），而 bundle 签的是字节。所以发版没附 bundle 时，改从签名仓库（`--signer-repo`，默认 getdroppen/droppen）同一标签的发版上取 `release.yml` 挂的清单和它的 bundle：这份清单的摘要必须与发版的相同，站点的文件按它核对，证明也对它验。签名仓库那边没有（或取不到）算没验到证明；有、却是另一个构建的清单，或证明对不上，都算失败，`--allow-unattested` 也不放过。能匿名取到发版，说明仓库是公开的，而公开仓库的每次发版都有证明，所以**默认没验到证明就算失败**：没装 `gh`、或发版没附 bundle，都失败。确实要接受未证明的清单，加 `--allow-unattested`。用自己重建的清单（`--manifest`）时不要求证明，除非给了 `--bundle` 或 `--require-attestation`。
3. 下载清单里的每个文件，再取各主机会给出页面外壳的路由——应用的 `/`、`/link`、`/signin`、`/sessions`、`/move`，站点的 `/security`、`/download`——逐个比较 sha256。外壳先去掉 Hub 加的唯一一处改动：`<head>` 紧后面的 `<meta name="rh-hub" content="<origin>" />`。只去掉这个确切的标签，别的差异照样算不同。
   脚本不跟随重定向，查的是这个主机自己给出的东西。一个回答若是重定向，只有它把浏览器送到**这次也在检查的另一个主机上的同一路径**（查询串也相同）才算通过，那边的内容在检查那个主机时核对；送去别处、或换了路径，都算失败。三个主机分工后正是这样：droppen.app 的 `/` 去 `www`，`www` 的 `/link` 去 `my`，`my` 的 `/download` 去 `www`。整个主机都重定向到另一个主机时（如分工之前的 `www.droppen.app`），连 `/.well-known/droppen-build` 也按那个主机的回答对照。
4. 这个标签的 origin 决定页面去哪里登录、所有 Hub 请求发往哪里，所以它必须是**这个主机自己的**（`https://my.droppen.app` 对应 `wss://my.droppen.app`，`https://droppen.app` 对应 `wss://droppen.app`），否则失败；别的 Hub 要用 `--hub-origin wss://…` 明确给出（对每个主机都有效）。字节相同、却指向别的 Hub 的页面，不算同一个页面。
5. 检查每个主机上每个页面（各路由的外壳和清单里的每个 `.html`）响应头里的 CSP。没有 CSP 响应头的页面算失败。`script-src`、`script-src-elem`、`script-src-attr`（以及兜底的 `default-src`）只许 `'self'`、`'none'` 和这些页面自己内联脚本的哈希；`frame-ancestors` 必须有，且只许 `'none'` 或 `'self'`（别的网站不能把页面框进去）；`object-src`（没有时看 `default-src`）只许 `'none'` 或 `'self'`。一个响应头里有几条策略时，每条都生效，所以每一项有一条策略够严就行。各主机的 CSP 各不相同（`connect-src` 写的是自己的主机），所以每个主机单独检查。
6. 读清单里那份 `sw.js` 的完整性表（[下文](#service-worker-的缓存)），它必须与清单逐项一致：`assets/` 下的每个文件和它的 sha256，不多不少，外壳是 `index.html` 的 sha256，离线外壳允许的内联脚本正是 `index.html` 自己的。没有这张表的 `sw.js`（早于这项检查的构建）也算失败：它的缓存不经核对就交给页面。`/sw.js` 还按浏览器检查更新时的样子（带 `Service-Worker: script`）再取一次。不放应用的主机（应用搬到 `my.` 之后的 droppen.app，以及 `www`）在 `/sw.js` 给的是「退役 Worker」：清空这个源的缓存、注销自己，别的什么都不碰（[下文](#service-worker-的缓存)）。脚本只接受与它内置的那段代码逐字节相同的退役 Worker，别的都算失败。

结果是每个主机一个 `PASS` 或 `FAIL`，并列出不同的文件。退出码：0 全部通过，1 有主机不通过，2 有主机没能检查（网络、参数）、其余都通过。

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

### Service Worker 的缓存

网页带的 Service Worker（`web/public/sw.js`）把 `/assets/` 下的文件存进浏览器的缓存，以后先用缓存。这是一个能留下来的入口：页面里的脚本能写 Cache Storage，一次恶意投递就能在那里放下改过的资源；Vite 的文件名只随内容变，很少变的块（如 `vendor`）名字一直不变，之后干净的发版也不会把它换掉。

所以构建时（`web/build/service-worker.ts`）把 `assets/` 下每个文件的 sha256、外壳 `index.html` 的 sha256 和外壳内联脚本的 CSP 哈希写进 `sw.js` 自己（`INTEGRITY`、`SHELL_SHA256`、`SHELL_SCRIPTS`）。Worker 只信自己脚本里的这张表，不信缓存里的任何东西：

- 资源的字节与表里的 sha256 相同才写进缓存；每次从缓存取出都重新算一遍，不同就删掉，改从网络取。
- 从网络取到的对不上，就绕过浏览器的 HTTP 缓存（`/assets/` 在那里一年不过期）再取一次；还对不上就拒绝（`502`），不交给页面，也不缓存。表里没有的资源（Worker 还是旧版、页面已是新发版时）直接走网络，不缓存。
- 激活时删掉 `CACHE` 以外的所有缓存，再删掉表里没有、或字节对不上的每一项。离线外壳也一样：它要么没有 Hub 的标记，要么只有本站自己的那一个（`https` 上是 `wss://<本站主机>`，`http` 上是 `ws://`，紧跟在 `<head>` 后面，与验证脚本的规则相同），去掉后是这次构建的 `index.html`，才保存，才在离线时使用。标记指向别的 Hub 的外壳会去那里登录、把一切发过去，所以不算。
- 离线时交给页面的外壳用 Worker 自己设的响应头，不用缓存里的（页面能连头一起写进缓存）：`Content-Security-Policy` 与 Hub 给外壳的相同——脚本只许本站文件和外壳自己的内联脚本（按 `SHELL_SCRIPTS` 的哈希）——只有一处不同：中继的 origin 由 Hub 在每个响应里列出，离线的 Worker 不知道，所以 `connect-src` 用 `https:` 和 `wss:` 代替，好让离线打开、之后恢复联网的页面还连得上中继；能运行什么（`script-src`）与 Hub 的完全相同。另有 `X-Content-Type-Options: nosniff`、`Referrer-Policy: no-referrer` 和 Hub 的 `Permissions-Policy`。没有标记的外壳（本机服务器的）用同一策略，`connect-src` 只有本站，允许本站框住它。
- 每次发版这张表都变，`sw.js` 的字节随之改变，浏览器就会装上新的 Worker：`sw-register.ts` 每次回到前台都调用 `update()`，浏览器自己也至少每 24 小时检查一次 `sw.js`（取它时不经 HTTP 缓存）。

`scripts/web-manifest.mjs` 不为表与文件不符的构建写清单（`--check` 也会报告），`verify-web.mjs` 拿这张表对照证明过的清单。

代价：以前缓存会把旧构建的懒加载块再留一周，好让还开着旧版本的标签页继续用。这些块不在新表里，无法核对，现在新 Worker 一激活就删掉；旧标签页之后要用的块改从网络取，Hub 已不提供时就加载失败，刷新页面即可。

**紧急开关。** 怀疑缓存被投毒，或缓存逻辑本身出了错：改掉 `web/public/sw.js` 里的 `CACHE`（如 `rh-sw-v1` → `rh-sw-v2`）并发版。新 Worker 激活时删掉所有别的缓存，从空缓存开始。它管不到的：已经在运行被篡改代码的页面（要等它关掉或刷新）；还没取到新 `sw.js` 的浏览器（浏览器在页面导航时检查 `sw.js` 有没有变，`sw-register.ts` 每次回到前台也调用 `update()`；没再打开过这个网站的浏览器就一直没换）；恶意代码写进 IndexedDB、localStorage 的东西（那要在浏览器设置里清除这个网站的数据）。

**不放应用的主机上的 Worker。** 应用搬到 `my.droppen.app` 之后，droppen.app 和 `www.droppen.app` 上不再有应用，但浏览器里可能还注册着以前的 `/sw.js`，它会在离线时继续拿出缓存的旧外壳。这两个主机对 `/sw.js`（以及任何 Worker 请求）回一段「退役 Worker」：装上后清空这个源的所有缓存并注销自己。它不碰 IndexedDB 和 localStorage：这台浏览器的设备钥还要从那里搬到 `my.`。`verify-web.mjs` 内置这段代码（`RETIRING_WORKER`），只接受与它逐字节相同的回答。

**别的地址上的 Worker。** 一次恶意投递还可以从别的地址（如 `/x.js`）为 `/` 注册一个 Worker，替掉 `/sw.js` 的注册。以前诚实的 Hub 对那个地址回 HTML 外壳，浏览器的更新检查因 MIME 类型失败，就一直留着旧 Worker，改 `CACHE` 也换不掉它。现在 Droppen Hub 对带 `Service-Worker: script` 请求头、路径不是 `/sw.js` 的请求，回一段小 Worker（`text/javascript`，`no-store`）：它装上后清空本站所有缓存并注销自己，页面下次加载时再注册 `/sw.js`。本机 `droppen` 服务器也这样回答（`server-rs/src/static_files.rs`）。

## 4. 现状和剩下的风险

- **v0.2.127 之前的发版没有证明。** 它们在发版用的 Mac 上构建，私有仓库也用不了构建证明，所以只附了未证明的 `web-manifest.json`；从 v0.2.127 起，每次发版都由 `release.yml` 构建并证明。对照更早的发版要加 `--allow-unattested`，或者拿自己重建的清单来比（`--manifest`）。
- **Hub 可能跑着没发过版的网页。** Hub 由运营者手动从选定的提交部署，`/.well-known/droppen-build` 报告的就是这个提交；它不一定是发版标签。如果这个提交里 `web/` 有还没发版的改动，站点的摘要就不在任何发版里，验证会失败。这时按站点报告的提交自己重建。只从发版标签部署 Hub 能彻底避免这种情况。
- **只核对列出的文件。** Hub 可以多放别的路径，但开源代码不会去加载它们，除非外壳或已核对的 JS 引用。外壳只抽查了七个路由；Hub 理论上能在别的路由上给出不同的外壳。
- **只核对检查的主机。** 不给 `--site` 时是 droppen.app 的三个主机。别的主机（如中继）不下发这份网页，不在检查之内。
- **CSP 只检查脚本来源、`frame-ancestors` 和 `object-src`。** 其它指令（`connect-src`、`style-src` 等）和别的响应头不在证明范围内。CSP 是 Hub 在响应头里给的，不在构建产物里：验证脚本只能说明它拿到的那几次响应带着这样的 CSP。浏览器第一次打开、Service Worker 还没接管页面时，页面的 CSP 就是 Hub 当时给的那一个；之后离线外壳的 CSP 由 Worker 自己设（§3）。
- **刚发版、Hub 还没重新部署时，默认检查会失败。** 发版之后到 Hub 换上新构建之前，站点给的是上一次发版的构建，不是最新发版的，默认检查判失败。这时用 `--allow-older`，或 `--release <上一个标签>` 检查它确实是那次发版的构建。
- **信任链的其余部分：** GitHub Actions、npm 注册表（锁文件带完整性哈希）、Node 镜像。可复现构建让人可以不信 CI，自己重建来比。

## 5. 以后

浏览器扩展在每次加载时核对；独立的静态源加版本化的不可变 URL；把各次检查的结果记到公开日志里；本机界面作为默认入口；签名的手机应用。签名并公证的 macOS 桌面应用已经有了（§1）。
