# Droppen：一切都是设备、在本机登录、多台电脑同屏

> 设计稿，2026-10-17（基于 v0.2.102）。只写设计，不改代码。
>
> 前提与协议见 `docs/cross-machine-architecture.md`（下称「架构文档」）和 `docs/relay-and-plans.md`（下称「中继文档」）。相关的另外两件事：SPA 的完整性（`docs/web-integrity.md`），云端宽屏与「需要你」。
>
> 已定：
> - **开源边界**：`droppen`（本机执行器、agent、本机 React 前端）与 `droppen-relay` 开源；只有 Droppen Hub 闭源（§2.1）。
> - **设备与机器合一**：「客户自己用 droppen 在本地启用，那么他既是设备也是机器」；「只是部分机器无法提供启动 agent 的功能」。一切都是**设备**，唯一的内在差别是能不能运行 AI；唯一可以开关的权限是「可管理账号」（§3）。
>
> 路径缩写：`S/` = `server-rs/src/`，`C/` = `server-rs/crates/`，`W/` = `web/src/`。

## 0. 结论

- **三个程序，各管一层。**
  - `droppen-hub`（droppen.app）是闭源的控制面：账号、套餐、设备链、信箱、钥环、加密流、票据。
  - `droppen-relay` 是开源的中继，只转发密文。
  - `droppen` 是开源的本机程序，即今天的 `remote-hand-server`：连接 AI CLI，单机就完整可用；本机不登录时，行为与今天逐字节相同。
- **一切都是设备（链 v3）。** 一次安装、一个浏览器、一部手机，各是一台设备，各有一个身份（一把签名钥、一把 DH 钥）。
  - **每台设备都能远程控制**（开通道、操作会话与终端）。这不是一种能力，不用授予，也不出现在界面上。
  - **可运行 AI** 由设备是什么决定：Droppen 安装能，浏览器和手机不能。它在设备加入时声明并签进链里，用户不能开关；协议保留「停止运行 AI」。
  - **可管理账号**是唯一的真权限，每台设备单独开关。它决定谁能签链条目、谁持用户钥 `UK`、谁能作证。用户亲手登录的设备（浏览器、手机、笔记本上的 Droppen 应用）默认开；无人值守、常开的运行 AI 的设备建议关。
  - 这取代了今天的「设备 / 机器」两套身份和「完整 / 受限」两种设备。
- **在本机设置里登录。**
  - 点「登录 Droppen」会打开 droppen.app 的弹窗。密码、Turnstile、恢复词都只在 droppen.app 上输入。
  - 一台可管理账号的设备确认后，用 `postMessage` 把 RH1 注册码交回本机页面。
  - 本机在进程内完成注册并起链接，不用命令行，不用重启，也从不经手凭据。
- **本机就是控制端（方案 A）。** 本机这台设备用自己唯一的身份，同时运行 AI，并经中继或直连控制账户里其它能运行 AI 的设备。
  - 不再有单独的「聚合器」身份和第二对钥匙。
  - 浏览器和局域网手机经本机网关看，它们不持钥。
- **机器感知的核心：一份开源代码，三种外壳。** SPA 的一切按「哪台运行 AI 的设备」取址。本机未登录、本机已聚合、droppen.app 三种外壳，共用统一会话列表、跨设备「需要你」、跨设备分屏、新建会话选设备、每台设备的设置。
- **顺序。**
  - S1-1 到 S1-3（本机登录）先做。
  - 链 v3（SC）排在 S1-4 之前。通行密钥的链条目（`docs/passkeys.md`）在同一版本里定义，共用一个门槛。
  - S1 的其余部分交付「本机里的统一会话列表」。
  - 开源边界的准备工作（S0）与它们并行。

## 1. 现状（已在代码里核对）

| 方面 | 现状 | 位置 | 对本设计的影响 |
|---|---|---|---|
| 链的格式 | 条目版本字节为 2。动作有 `Genesis`、`AddDevice`、`RemoveDevice`、`AddMachine`、`RemoveMachine`、`RecoveryJoin`、`ApproveDevice`、`ActivateRecovered`、`SetRecoveryKey`、`KeyEpochs`。签名者是 `AK`（只签创世）、当前完整设备、`RSK`（只签两种恢复条目） | `C/rh-proto/src/chain.rs:63-65,184-226` | 链 v3 在此之上加条目（§3.2） |
| 设备与机器 | 两套身份：设备 `DeviceInfo{id, sign_key, dh_key, full}`、机器 `MachineInfo{id, sign_key, dh_key}`，id 在两者之间不重复。完整设备持 `UK`、能签条目、能作证；受限设备都不能，产品里从没用过 | `chain.rs:165-181,1084,1231,1394-1409` | v3 合成一种「设备」 |
| 机器能做什么 | 只能被连：agent 登录、写流、签流头；中继拒绝 agent 发的 `OPEN`；agent 票据的 `peers` 为空；agent 不能订阅流 | `C/rh-relay/src/route.rs:179-181`、`C/rh-proto/src/ticket.rs:8-9`、Hub | 能运行 AI 的设备也要能发起控制 |
| 新鲜度 | 开任何通道都要求「另一台完整设备」12 小时内为 agent 的当前链头作证；只有唯一的完整设备自证才算数 | `chain.rs:1421-1463`、`C/rh-proto/src/handshake.rs:70` | 常开的设备自己不能作证，§3.4 |
| 套餐上限 | `devices.max` 只数当前完整设备；`machines.max` 数当前机器加未完成的注册 | Hub | 合并为「设备数 + 可运行 AI 的设备数」，§3.8 |
| 注册 | 独立进程 `agent enroll <RH1> --hub <wss>`。码是 `RH1-<账户指纹>-<秘密>`，`token = HMAC(秘密)[..16]`；证明是 HMAC，绑定账户、id、两把公钥与 token | `S/agent/enroll.rs:35-145`、`C/rh-proto/src/identity.rs:123-207` | 协议原样复用，只换「码怎么到机器上」 |
| 设备侧确认 | droppen.app「添加机器」：`POST /v1/enrollments` → 显示命令 → 轮询 → 验证明 → 链上写 `[AddMachine, KeyEpochs]` → 写钥环、交 `MK`。需要完整设备的设备会话 | `W/lib/hub/account.ts:1259-1300`、`W/features/cloud/cloud-app.tsx:974-1028`、Hub | 必须有一台可管理账号的设备在场 |
| 链接何时起 | 只在 server 启动时 `spawn_if_enrolled`；注册后要重启 | `S/app.rs:333-345`、`S/agent/link.rs:114-182` | 要能在运行中启停（S1-1） |
| 解除 | 没有 `unenroll` 或 `status`；agent 不理会自己被移除；Hub 在 agent 登录时不查它是否仍在链上；`AddMachine` 不告警 | `S/agent/enroll.rs:36-46`、Hub、`S/agent/link.rs:516` | S1-1 补上 |
| 本机账号 | 每个实例单独，argon2id；用 bearer（localStorage 的 `rh.auth.token`），**没有 cookie**，**没有角色**；没有任何请求因为来源地址被放行：有用户账号时处处登录；没有用户账号（桌面 App 的应用内部账号不算）时，这台设备的主人用 `remote-hand-server open`（本机控制 socket 发的一次性令牌）以 `local:owner` 身份进入，其他设备用一次性设置码建第一个账号（2026-10-19 起，见 README「谁需要登录」） | `S/auth/users.rs`、`S/auth/mod.rs:212-263,494-509`、`W/lib/auth.ts:25` | 登录 Droppen 前必须有本机账号 |
| CSRF | `/api/*` 上有 Origin 检查；没有 Origin 的请求放行 | `S/origin.rs:25-40,106-137` | 新路由沿用，另加 nonce |
| 设备经通道进本机 | 桥接到回环 `/api/*`，身份是 `cloud:<设备>`。任何当前设备都能到达所有 `/api/*`，包括自升级；只有 `/rh/display-name`、`/rh/machine-key` 限完整设备 | `S/agent/bridge.rs:141-171`、`S/auth/mod.rs:295-309`、`S/agent/link.rs:632-759` | 不可管理账号的设备要一份拒绝清单 |
| 设备会话与信箱 | 设备签挑战即可换 30 天的设备会话，不需要 bearer；`POST /v1/devices` 接受任何会话；只有完整设备能发信 | Hub | 本机不需要密码；索钥请求要放开给非管理设备 |
| 索钥 | 完整设备只回应完整设备的索钥请求（用途 4），回的是 `UK` | `W/lib/hub/account.ts:1426-1431` | 加一个只回 `MK` 的分支 |
| Hub 的源 | 没有 CORS；WebSocket 不查 Origin；Turnstile 只能在应用源完成；没有设 COOP | Hub | 登录只能在 droppen.app 上 |
| 无头设备 | `rh-devkit` 已经是一个完整的 Rust 设备端：注册、设备会话、在 Hub 与中继签入、开通道、rpc、读流。钥只在内存里 | `C/rh-devkit/src/lib.rs:103-484,758-870` | 抽成库，给本机发起控制用 |
| SPA 的单机假设 | 全局唯一 `transport()`；`api` 单例；`feedHub`、`GlobalFeedStore` 是单例，行键 `source/id`；查询键、路由、`PaneContent` 都不含机器；换机器靠 `location.reload()` | `W/lib/transport/index.ts:18-34`、`W/api/client.ts:31`、`W/app/global-feed.tsx:25-191`、`W/app/pane-tree.ts:7-33`、`W/features/cloud/context.ts:48-72` | §6 的改造面 |
| 已按机器取址的 | 云端副本的 IndexedDB 键 `stream/<账户>/<机器>/…`；`StatusReaders` 同时读所有机器；`MachineChannel` 可以同时开多条 | `W/lib/hub/mirror.ts:167-169`、`W/features/cloud/machine-name.tsx:215-303`、`W/lib/hub/machine.ts:246-275` | 可以直接复用 |
| 打包 | 资产 `remote-hand-<版本>-macos-aarch64.tar.gz`；自升级是推送式上传；托管副本写死 `<数据>/bin/remote-hand-server`；示例 launchd 标签带个人前缀（`com.<名字>.remote-hand`） | `.github/workflows/release.yml`、`S/selfupdate/store.rs:39`、`S/selfupdate/mod.rs:183-200` | §7 |
| 仓库与依赖方向 | 一个工作区含 `rh-hub`；没有任何开源 crate 依赖 `rh-hub`；但本地端到端脚本从源码构建 Hub；Hub 的路由表只在它的文档注释里 | `server-rs/Cargo.toml` 的工作区成员、`C/rh-devkit/scripts/` 里的本地端到端脚本 | §2.1 |

## 2. 三个程序

![三个程序：Hub 控制面、中继只转发密文、本机 droppen 既运行 AI 又控制其它设备；浏览器与局域网手机经本机网关、不持钥](local-aggregator/programs.svg)

| 程序 | 二进制（今天 → 以后） | 源码 | 跑在哪 | 职责 | 不做 |
|---|---|---|---|---|---|
| Droppen Hub | `rh-hub` → `droppen-hub` | **闭源** | Droppen 运营的云端 | 账号、登录、邮箱、Turnstile、套餐与计量、设备链、信箱、钥环、加密流存储、在线态、票据、中继目录；托管 SPA（开源 `web/` 的构建产物，含新页面 `/link`）：应用在 `my.droppen.app`，公开页面在 `www.droppen.app`，`droppen.app` 本身留给设备与中继连接（`docs/hub-api/README.md`「Hosts」） | 不转发通道；不持任何能解密的钥 |
| 中继 | `droppen-relay`（不变） | 开源 | 运营者部署的各个区域 | 凭票据配对；只转发 `DATA` 密文；计量 | 不存东西；没有钥 |
| 本机 | `remote-hand-server` → `droppen` | 开源 | 用户的电脑；以后也作为 Tauri 应用分发（§7.3） | 连接 AI CLI、本机界面（内嵌 SPA）、本机账号。**登录后**，这台设备用同一个身份做两件事：运行 AI（agent，Noise 响应方），以及控制其它设备（Noise 发起方，经本机网关给本机界面用） | 不登录时不连任何云端 |

**不变量**

1. 本机不登录，就是今天的单机 server：不连云端，不绑定 UDP，不起任何云任务（今天「`state.json` 不在就什么都不做」的规则保持）。
2. 本机的请求从不等云端。云任务都在后台，有超时；Hub 或中继挂了，只影响「其它设备」那部分。
3. 本机聚合时，浏览器（包括局域网手机）**不持任何钥**；钥只在 `droppen` 进程里。
4. droppen.app 上的浏览器照旧直接连每台运行 AI 的设备，不经过别的设备转发。本机网关也不替远端设备转发：`cloud:*` 身份一律不能用网关（§4.5）。
5. 开源代码只通过 `rh-proto` 和公开的 Hub API 与 Hub 打交道（§2.1）。

### 2.1 开源边界与契约

**哪些开源**

| 部分 | 源码 | 说明 |
|---|---|---|
| `droppen`：执行器、agent、本机控制端与本机网关、本机登录 | 开源 | 本机上跑的一切。用户能看到本机到底做了什么：准入、钥怎么存、什么发给云端 |
| `web/`：本机 React 前端，**也包括**云模式（`features/cloud`、`lib/hub`、`lib/e2e`）和 `/link` | 开源 | 只有一份 SPA；droppen.app 上跑的是它的构建产物，由 Hub 托管 |
| `droppen-relay` | 开源 | 用户能核实中继只转发密文 |
| `rh-proto`：帧、Noise、设备链（含 v3）、记录、票据、互通向量 | 开源 | 两边共同的协议 |
| `rh-client`（新，由 `rh-devkit` 抽出）、`rh-devkit` | 开源 | 设备端库与测试工具 |
| Droppen Hub：账号、计费、存储、票据签发、管理工具 | **闭源** | 威胁模型里本来就不被信任的那一方 |

**为什么闭源 Hub 不削弱保证**

架构文档 §7 一直按「Hub 是恶意的」来设计。在这个前提下仍然成立的每一项保证，都由开源的一侧检查：

- 准入：agent 按自己那份链判定，首包不带请求，新鲜度在 agent 一侧；
- 钥：`UK` 与 `MK` 的投递带发送方认证，并对照链上的提交；
- 内容：端到端加密，读者只放行机器签名的流头；
- 本文新增：链 v3 的签名规则、非管理设备的拒绝清单、本机网关的鉴权、注册码交接。

所以 Hub 闭源只影响可用性与计费，不影响机密性，也不影响「谁能操作你的电脑」。唯一的剩余缺口是 droppen.app 上的 JS 由闭源 Hub 投递（`docs/web-integrity.md`）；本机界面和 Tauri 应用绕开了它（§7.3）。

**契约**

- **规则**：开源 crate 与 `web/` 不得引用 `rh-hub` 的代码、表结构、环境变量或未写进文档的行为。开源工作区由 CI 拒绝任何 `rh-hub` 路径依赖。
- **Hub API 文档**放在开源仓库的 `docs/hub-api/`，内容如下：
  - HTTP：`/v1/*` 每条路由的请求与响应、鉴权、限流与长度上限、错误形状 `{error, key?, limit?, used?, message}`；
  - WebSocket：`/v1/client`、`/v1/agent`、中继 `/v1/connect` 的 JSON 控制消息；二进制帧的格式引用 `rh-proto`；
  - 版本规则：`/v1/` 内只做加法；功能靠协商（`agent_info.features`、`/v1/me`）；Hub 兼容开源客户端的前 N−2 个版本；
  - 今天 Hub 的路由表就是初稿。
- **变更规则**：每个改动 Hub API 的变更，都在同一轮里更新 `docs/hub-api/`。闭源 PR 负责实现，开源 PR 负责文档与客户端，两边以同一份 JSON Schema 为准。
- **测试不需要 Hub 源码**：
  - 契约测试：两边用同一份 Schema；
  - 端到端：拉一个固定版本、只含二进制的 Hub 镜像。
- **仓库拆分**：Hub（`rh-hub`）移到私有仓库，照常依赖开源的 `rh-proto` 与 `rh-relay`（闭源依赖开源，方向正确）。

## 3. 一切都是设备（链 v3）

### 3.1 模型

| 属性 | 是什么 | 谁决定 | 改得了吗 |
|---|---|---|---|
| **远程控制** | 开通道到运行 AI 的设备，看、发、打断、回答提问、开终端、读写文件、读离线副本 | 每台设备都有 | 不是权限，不出现在界面上。要撤掉就移除这台设备 |
| **可运行 AI**（链里称 `host`） | 运行 agent，能被连，为自己的流持有机器钥 `MK` 的各轮 | 由设备是什么决定：Droppen 安装能；浏览器、手机不能。加入时声明，由一台可管理账号的设备签进链里 | 用户不能开关。协议保留 `SetHost` 用于停止运行 AI（「断开这台电脑运行 AI」但保留它作控制端）和重新开始 |
| **可管理账号**（链里称 `admin`） | 签链条目（加删设备、开关管理权、轮换、恢复词、`KeyEpochs`）；持用户钥 `UK`；作证；创建注册码；给别的设备投递钥 | 用户，在「我的设备」里逐台开关 | 能开关。不能给自己开；最后一台可管理账号的设备不能被关掉或移除 |

**例子**

| 设备 | 可运行 AI | 可管理账号（默认） |
|---|---|---|
| 浏览器 / 手机上的 droppen.app | 否 | 开（用户亲手登录）；后加入的手机可以选关 |
| 笔记本上的 Droppen 应用（Tauri，钥在钥匙串） | 是 | 开 |
| 今天的命令行 / launchd 安装，经本机设置登录（钥在 0600 文件） | 是 | **关**，开时警告（§3.3） |
| 无人值守的服务器（`droppen agent enroll`） | 是 | 关，建议保持关 |

> **暂定（链 v3 评审，决定 D1）**：可运行 AI 的设备**暂时一律不能管理账号**。agent 从不作证、不签条目，admin host 会让新鲜度失效、也会被算进「最后一台 admin」，把浏览器锁在外面。验证者（Rust 与 TS）拒绝 `Genesis3`/`AddDevice3` 同时带 `host` 与 `admin`、给 host 开 `SetAdmin`、给 admin 开 `SetHost`，以及带 `host` 的 `RecoveryJoin3`（理由：`a device that runs AI cannot manage the account`）；界面不再提供这个选项。上表「笔记本上的 Droppen 应用：开」**推迟到 host 能作证、能签条目（Tauri 应用）之后**再议。

> **界面不再有「可管理账号」（2026-10-20）**：每台设备的权限一样，不再有「可管理的设备」这个概念。每个浏览器、手机都是**完整设备**（链里的 `admin`）：配对时一律写 `admin`，没有选项；「我的设备」没有「可管理账号」一列和开关；本机设置的同意页没有这个开关（本机安装可运行 AI，按 D1 一律 `admin: false`）。D1 不变：可运行 AI 的设备仍由链保证不是 admin，界面不再说起。仍不是 admin 的浏览器、手机（早先关掉开关配对的，以及通行密钥加入的：链要求它们不带 admin，`RULES.passkeyPlain`）在列表里标「受限设备」，由任一台完整设备点「设为完整设备」确认后写 `SetAdmin{admin: true}`（要求链 v3；它不必在线，用户钥在它的信箱里等它；没能寄出时在它那一行说明，用「重发钥匙」补发）。界面不再提供把设备设回受限（`SetAdmin{admin: false}` 仍在协议与 `CloudAccount.setAdmin` 里）。「谁可以控制这台设备」的 `admins` 一项写作「只有你的完整设备」，另选的设备里受限设备标「受限设备」。因为浏览器、手机都是 admin，「近期有另一台 admin 作证」就等于「近期有另一台设备作证」，协议不用改。本节和 §3.7、§4 里关于「可管理账号」开关的描述以这条为准。

**用词**

- 界面与本文正文统一叫「设备」。**界面上一律叫「设备」，可运行 AI 的也是**（zh-TW「裝置」、ja「デバイス」、en "device"）：不说「机器」，也不说「电脑」。要和浏览器、手机分开时说「可运行 AI 的设备」，或者靠行上的「可运行 AI」徽章。说页面所在的这台设备也一样：「这台设备」/「這台裝置」/「このデバイス」/ "this device"（如「断开这台设备」/ "Disconnect this device"），不说「本机」「本機」「このコンピュータ」/ "this computer"；「本地」照用。服务端发给界面原样显示的提示与报错同样遵守。落地页的卖点文案是营销文案，不在此列；下载页是产品界面，在此列（指平台时可以直接说 Mac、Linux 服务器、Windows）。
- 「机器」只留在代码里，指「可运行 AI 的设备」：`MachineChannel`、`MachineMirror` 这些现有名字不改，`MK` 在代码与本文里仍叫机器钥；界面上叫「内容密钥」/ "content key"（zh-TW「內容金鑰」、ja「コンテンツ鍵」）：它加密那台设备的内容。「设备钥」已是每台设备自己的身份钥 `DK`（见 `cross-machine-architecture.md`），所以不用。
- `web/test/ui-wording.test.ts` 守着这条：四张表，以及界面源码里上屏的每一段文字（JSX 文本、`aria-label` / `title` / `placeholder` / `alt`、JSX 里的字符串与模板、`t()` 的 key），按 TypeScript 语法树读，注释和标识符不算；没有例外清单，不查的只有落地页的营销文案文件。
- 链上的 id 保留历史前缀（`d-…`、`m-…`）；v3 之后新加入的一律 `d-…`。代码不得按前缀判断任何事。
- **与通行密钥文档（`docs/passkeys.md`）对照**：
  - 那边写的「端点」= 这里的「设备」；
  - 那边的 caps `{host, control, admin}` 在这里是两个标志 `host`、`admin`；control 人人都有，不再是一种能力；
  - 所以那边的 `caps ⊆ {control}` 在这里就是 `flags = 0`；
  - 那边的占位条目名，以本节的定名为准（下文「通行密钥」一段）。

### 3.2 链 v3 的条目

**编码**

- 新条目的版本字节为 `3`；v2 条目照旧可读。
- 签名标签仍是 `rh/v1 chain-entry`，但版本字节在被签的字节里，所以一条 v3 条目不可能被当成 v2 解析，反之亦然。
- 一条链可以是「v2 前缀 + v3 后续」。

```
DeviceV3   = id str16 | sign_key [32] | dh_key [32] | flags u8      // bit0 = host（可运行 AI），bit1 = admin（可管理账号）；其它位必须为 0
新动作（v3）：
  11 Genesis3        { account_key, device: DeviceV3, recovery_key? }   // device 必须 admin
  12 AddDevice3      DeviceV3
  13 SetAdmin        { id, admin: bool }
  14 SetHost         { id, host: bool }
  15 RemoveDevice3   id                                                   // 不分设备与机器
  16 PasskeySlotSet  { slot, key: PAK, proof, cred_hash, salt_nonce, grants: u8, self_activate: bool }   // admin 签；grants 必须为 0
  17 PasskeySlotRemove slot                                               // admin 签
  18 PasskeyActivate id                                                   // Passkey(slot) 签
新签名者：Passkey(slot)，只能签两种条目：flags = 0 的 AddDevice3（待定），以及 PasskeyActivate
沿用（编码不变，v3 下照常写）：RecoveryJoin（必须 admin，可带 host）、ApproveDevice、ActivateRecovered、SetRecoveryKey、KeyEpochs
```

**通行密钥**

通行密钥的条目与 v3 同一个版本一起定义、一起过门槛，免得 v3 上线后再升一版。它们的语义完全照 `docs/passkeys.md` §4.4，这里只定名字和与本模型的接法：

- **登记槽** `PasskeySlotSet`：由 admin 签。`grants` 在本模型里必须为 0，即槽只能加入普通设备（不可管理账号、不可运行 AI），对应通行密钥文档 §4.10「通行密钥永远不授予 admin」。保留这个字段是为了以后不必再改编码。记下 `set_by` = 签名的 admin。
- **槽签的加入**：签名者为 `Passkey(slot)` 的 `AddDevice3`，`flags` 必须为 0。新设备进入**待定**状态，记 `via_passkey = slot`。同一个槽同时只有一个待定设备，新的加入顶替旧的。
- **槽签的激活** `PasskeyActivate`：签名者为 `Passkey(slot)`，且该设备的 `via_passkey` 就是这个槽。要求槽仍在、`self_activate` 为真、**验证者自己**首次看到那条加入已满 24 小时（`ChainClock`，规则同恢复）。
- **批准时顺手给管理权**：admin 在同一批里写 `[ApproveDevice(id), SetAdmin{id, true}]`。签名者不是被授权的设备，所以不违反「不能给自己加权限」。
- **作废规则**：移除的设备若 `via_passkey == s`，或者是槽 `s` 的 `set_by`，槽 `s` 一并作废（同今天的 `recovery_set_by`）。槽作废后，靠它待定的设备不能再自行激活，但 admin 仍可批准或移除。
- **限制**：同一时刻至多 8 个槽；同一个 `cred_hash` 至多一个当前槽；槽 id 永不复用。
- **激活与套餐**：自行激活不受 `devices.max` 限制；admin 的批准以 `devices.max + 1` 为限（规则同恢复）。
- **通行密钥包封什么**：本模型里 `UK` 只给 admin（§3.3），所以通行密钥包走通行密钥文档 §4.3 的「按机器封 `MK`」那一支，钥环名为 `pk/m/<host>/<MK 轮>/…`。用通行密钥加入的设备能读全部历史，但读不到 UI 文档。

**v2 怎么映射**

验证者按下表把 v2 条目确定性地翻译成 v3 状态。Rust 与 TS 两份实现，有互通向量。

| v2 | v3 状态 |
|---|---|
| `Genesis{device(full)}` | 设备，admin |
| `AddDevice{full: true}` | 设备，admin |
| `AddDevice{full: false}` | 设备，非 admin（产品里没出现过） |
| `AddMachine` | 设备，host，非 admin |
| `RecoveryJoin` | 设备，admin，待定 |
| `RemoveDevice(id)`、`RemoveMachine(id)` | 移除设备 |
| `KeyEpochs.machines` | 对应 host 设备的 `MK` 提交 |

`ChainState` 改为一张 `devices: id → {info, flags, status}` 表。`device()`、`machine()` 两个视图留作兼容：`machine()` = host 为真的设备。

**签名规则**

1. 普通条目只能由**当前、活跃、admin** 的设备签。例外有三类：`Genesis*` 由 `AK` 签；`RecoveryJoin`、`ActivateRecovered` 由 `RSK` 签（不变）；`Passkey(slot)` 只签上面那两种条目，而且只在槽仍有效时。
2. **不能给自己加权限**：`SetAdmin{id: 签名者, true}`、`SetHost{id: 签名者, true}` 无效。给自己减权限可以，但不能让账户没有 admin。
3. 最后一台活跃的 admin 不能被 `SetAdmin false`，也不能被移除（今天「最后一台完整设备」规则的推广）。
4. **只有 host 才有机器钥**：
   - `KeyEpochs` 的 `MK` 提交只能针对 host 设备；
   - 流头、manifest、草稿只认 host 设备的签名钥；
   - agent 只能以 host 设备的身份登录 Hub（`HubLogin` 角色 Agent），中继的 agent 票据也只签给 host 设备。
5. `AddDevice3{host}` 由 admin 签，并且只在该设备完成注册证明之后签（§4.3 第 7 步）。浏览器、手机走配对，配对流程不给 host。
6. `SetHost{false}` 之后，这台设备的流按「被移除的机器」处理：停写，旧副本可以删；`MK` 不再轮换给它。`SetHost{true}` 必须与一条新 `MK` 轮次的 `KeyEpochs` 在同一批里。
7. flags 的未知位、非规范编码、重复 id 一律拒绝。id 永不复用（不变）。
8. 批内按顺序生效：同一批里先把签名者降为非 admin、再由它签的条目无效。

**告警**

| 条目 | 告警对象 | 级别 |
|---|---|---|
| `AddDevice*`、`RecoveryJoin`、`ActivateRecovered`、`ApproveDevice` | 所有设备的界面、所有 host 的日志与推送 | 要求确认「是我做的」（今天的行为，文案写明「可运行 AI」「可管理账号」） |
| `SetAdmin true` | 同上 | 高：「X 现在可以管理账号」 |
| `SetAdmin false`、`SetHost false`、移除 | 同上 | 提示 |
| 单独的 `KeyEpochs`（不在移除后的轮换或注册里） | 同上 | 提示（今天的 N2 规则） |
| 新加入的 host（今天的 `AddMachine` 不告警） | 同上 | 要求确认 |
| `PasskeySlotSet`、`PasskeySlotRemove`、槽签的加入、`PasskeyActivate` | 同上 | 槽签的加入要求确认，任一 admin 可以否决（通行密钥文档 §4.6） |

每台 host 每小时最多 4 条告警，与今天的 N3 规则相同。

**实现备注（SC-1…SC-5）**

实现与上文的差别，以代码为准：

- **「谁可以控制我」提前到 v3**：新增第 19 号条目 `SetAccept { host, from: admins | all | devices[ids] }`，admin 签。每台 host 默认只接受 admin 设备的控制；host 之间、以及任何非 admin 设备的控制，都要 admin 明确放行。agent 在准入（`ChainState::may_control`）和已开通道上执行，Hub 在路由和签票（`peers`）时也执行。v2 链上没有这种条目，所以 host 之间互控要等账户能写 v3。名单要求升序、不重复、非空；与当前策略相同的条目被拒绝（不告警空操作）。
- **新 host 的第一轮 `MK` 必须紧跟**：不只 `SetHost true`，`AddDevice3{host}` 和带 host 的 `Genesis3` 也要求下一条就是该 host 新一轮 `MK` 的 `KeyEpochs`。Hub 拒绝以这种欠账结尾的批。恢复加入的 host 例外：它先是待定的，激活后再由 admin 提交。
- **无操作条目被拒**：`SetAdmin`、`SetHost` 设成与现状相同的值一律无效。所以「给自己开 admin」本来就是无操作，也被拒绝。
- **通行密钥的新加入顶替旧的**：同一个槽的新待定加入会把旧的待定设备直接标为已移除，但不作废这个槽。
- **给非 admin 设备投 `MK` 用信箱用途 5**，不用 2。封装上下文是用途字节 3（`HostKeyContext`，在 `SealContext` 的布局后加 `str16 host`），信件内容是 `str16 host ‖ sealed`。用途 2 仍只表示「host 自己的 `MK`，给它的 agent」，旧 agent 收到用途 5 会直接确认丢弃。
- **新鲜度**：请求方自己是 admin 时，只有它是唯一 admin 才算自证，与 v2 相同。管理类 12 小时的检查不只覆盖 `/rh/machine-key`、`/rh/display-name`，拒绝清单里的每条 admin 路由都要（§5.4）。
- **门槛看批之后的 host**：Hub 判断能否写 v3，看的是这一批生效之后的当前 host。所以新加入的 host 也必须声明了 `chain/3` 和 `policy/1`；被这一批移除或停用的 host 不再卡住账户，否则 agent 退回旧版的 host 在 v3 账户里就永远移除不了。
- **第一条 v3 条目之后不再写 v2**：连两种版本共用的动作（`KeyEpochs` 等）也按 v3 写。在 v3 账户里恢复加入，要用 v3 的 `RecoveryJoin`（浏览器端的写入器以后补）。

### 3.3 钥

**每台设备一个身份：一把 Ed25519 签名钥 + 一把 X25519 DH 钥**

- Droppen 安装沿用今天的机器种子（`agent/machine-key.json`，HKDF salt `rh/v1 machine`）。
- 浏览器、手机沿用 WebCrypto 的钥：用时是不可导出的 `CryptoKey`，存时封在本机钥下；同源代码能解开读出（架构文档第 7 节「网页前端投递」）。
- 不为「运行 AI」和「控制」各发一对钥，理由如下：
  - **没有隔离可言**：同一个安装里，agent 与控制端在同一个进程、同一块磁盘上。拿到一把就拿到另一把。
  - **一个身份 = 一个 id、一个 Noise 静态钥、一次移除**。两对钥会带来两个 id、半撤销状态（撤了控制却留着运行 AI）、两倍的轮换与告警。
  - **用途靠标签分开，不靠钥分开**：Hub 登录的角色（Agent / DeviceSession）、中继的角色（RelayAgent / RelayDevice）、链签名（chain-entry / chain-attest）、流签名（stream-head / manifest）都有各自固定的标签。同一把钥在不同角色下的签名互相不能冒用。
  - **Noise**：一台设备既当发起方又当响应方，用的是同一把静态钥。prologue 绑定（账户、发起方 id、响应方 id），所以反射与拼接都不成立。要拒绝「自己连自己」。IK 的 KCI 论证不变：持有响应方静态钥的人能伪造首包，但完成不了握手，所以发不出请求。
- **真正值得隔离的只有「可管理账号」**：它要 `UK`、能签链条目。处理办法是「默认关 + 存放要求」，而不是第二把钥：
  - 无人值守的 host 建议关。
  - 钥在 0600 文件里的安装（今天的命令行 / launchd 安装）默认关。理由：同一用户的进程（包括本机上跑的 AI CLI）能读这个文件；开了 admin，就等于把「签链条目 + 全部历史」交给了它们。开的时候界面明确警告。
  - 钥在钥匙串（Tauri 签名应用，只给本程序的代码身份）或在浏览器里的设备（钥存在这个源的 IndexedDB，别的网站读不到；同源代码读得出，见架构文档第 7 节「网页前端投递」），默认开。
  - 以后若要「admin 用单独的钥、每次签名要用户在场」，条目版本里可以加 `admin_key` 字段。现在不做。

**机器钥 `MK`**

- 每台 host 设备一把，按轮次在链上提交（`KeyEpochs`，以设备 id 为键），流名和流钥都由它派生。这与今天相同，只是把「机器 id」读作「host 设备 id」。

**用户钥 `UK`：只给可管理账号的设备**

- 不给非 admin 设备，理由：
  1. `UK` 能解开钥环里每台 host 的每一轮 `MK`，还能为别人封钥。分发钥是管理行为。
  2. 非 admin 设备被移除时，只需轮换 `MK`，不用轮换 `UK`。
  3. 以后要按 host 限制控制范围（S4），前提就是非 admin 设备拿不到能解开一切的钥。
- **非 admin 设备的 `MK` 怎么来**：每台设备都能控制，所以都需要 host 们的 `MK`，才能读离线副本。办法如下：
  - 由一台 admin 把每台 host 的每一轮已提交 `MK` 用发送方认证封装给它（信箱用途 2）。时机有三个：设备加入时、新 host 加入时、每次轮换时。
  - 缺钥时，非 admin 设备发用途 4 的索钥请求。这要改 Hub：用途 4 允许非 admin 发，只能发给 admin，客户端照旧每 10 分钟至多一次。
  - 在线的 admin **自动**回复，只回 `MK`、从不回 `UK`（`W/lib/hub/account.ts:1426` 旁加一个分支，带测试）。
  - 收到的每把钥都对照链上的 `KeyEpochs` 提交，对不上就丢弃。
- **UI 文档**（布局、待办，等它们上云时）：非 admin 设备读不了 `UK` 加密的文档。上云时再给它们一把单独的「文档钥」，现在不定。
- **对通行密钥文档的影响**：它按「control 的个人设备持 `UK`」写，并在 §4.3 留了「`UK` 只给 admin 时按机器封 `MK`」的分支。按本节的建议，走后一支（§3.2「通行密钥」一段）。

### 3.4 新鲜度：按操作，不按设备

- **今天的问题**：开任何通道都要求另一台完整设备 12 小时内为 agent 的链头作证。常开的设备（本机控制端、无人值守的 host）自己不能作证，或者作证不算数。只要用户 12 小时没打开别的 admin 设备，它们的实时功能就全部 `Stale`。
- **v3 的规则**：窗口跟着**操作**走。

| 操作 | 窗口 | 谁的证明算数 |
|---|---|---|
| 控制：开通道、会话、终端、文件 | **7 天** | 任何当前 admin 的证明。请求方自己是 admin 时，它自己的证明只在它是唯一 admin 时算（规则同今天） |
| 管理：通道里的 `/rh/machine-key`、`/rh/display-name`、重加密请求 | **12 小时** | 同上 |

- **实现**：`handshake::accept`/`accept_direct` 用 7 天准入；agent 在通道里每开一条管理类子流时，再按 12 小时检查一次。开着的通道也守 7 天：agent 记着它到期的时刻（最近一条算数的证明加 7 天），到点、链每次变化、每开一条子流都再看一次，过期就关掉这条通道（`4412`），连同其中的终端流和直连；之后要重新连上，就得重新准入。开着的通道也认更早链头的证明，所以链头前进、新证明还没到的那一会儿不会断线；但只认 agent 接到下一条条目之前（容许时钟偏差）签的那些：hub 把某台 admin 冻在旧链头、再转发它对旧链头的新证明，续不了命。每台 admin 说过的话分开记着：它后来那条不算数的证明（链头之外的，或落在后面的）收不回之前的话，它不再是 admin 才收回。唯一 admin 自己的证明，按它作证的那个链头上它是不是唯一 admin 来算，所以唯一 admin 配对第二台 admin 时，它开着的通道不会断。新开通道仍要求证明正是当前链头。请求在派发时（收齐请求体之后）再按 7 天和管理类的 12 小时各判一次，扣着请求体不发拖不过期限。实现在 `agent::link`（`ChannelLapse`）。剩下的限制要说清楚：如果 hub 把其它每一台 admin 都冻在本机的链头上，这个上限就取决于那些 admin 自己发现被冻住；新开通道要求证明正是当前链头，也有同样的限制。
- **为什么可以放宽控制的窗口**：新鲜度防的是「Hub 扣住一条移除」。控制类操作在被扣住的移除下最多多活 7 天，而且前提是 Hub 本身也被攻破（诚实的 Hub 一收到移除就吊销会话和中继票据）。管理类操作能改钥，所以保持 12 小时。
- **作证**：只有 admin 设备作证。时机是每次连上、链变化时、开着期间每 3 小时一次（今天的行为）。所以一周内在任一 admin 设备上打开一次 Droppen 就够了。
- **体验**：窗口剩 24 小时时，本机设置里出现提示，并经本机的 Bark / Web Push 提醒一次。过期后降级为只读：读副本不受新鲜度约束，只要求是当前设备。

### 3.5 从链 v2 迁移

**新账户：从创世起就是 v3**（2026-10-21）

- 注册时直接建在链 v3 上，不再先建 v2、再请用户升级（`CloudAccount.createGenesis`）。同一次追加写两条：
  - `Genesis3`：这台浏览器是第一台 admin，不可运行 AI（D1）；恢复词照旧可选；
  - v3 的 `KeyEpochs`：用户钥第 0 轮。
- 这正是下文「把账号升级到 v3」给已有账户写下的第一条 v3 条目（一轮新的用户钥），所以新账户的链从 0 号就是 v3（`v3From = 0`），「我的设备」里没有升级这一步，`upgradePlan()` 为空。
- 只在 Hub 收 v3 条目时建：`/v1/me` 的 `chain_v3.writable`（建账户前没有 host，门槛只看 `DROPPEN_HUB_CHAIN_V3=write`，2026-10-21 起这是 Hub 的默认值，`read`、`off` 只作显式拨下的总闸）。不收时注册停在创世这一步，页面说明是 Hub 这边的原因、请稍后再试；追加时 Hub 才回 409 `chain_v3_not_ready` 的，也一样。**不退回 v2**：新账户只有 v3 一种设计。
- 之后的一切（注册可运行 AI 的设备、配对、恢复词、移除与轮换）都写 v3，只读 v2 的 agent 一开始就加不进来（门槛按名字拒绝它）。
- Hub 仍收 v2 创世（此前建的账户、测试里代表它们的账户与开发工具 `rh-device` 用它），本节其余部分只关乎那些账户。

**已有的账户**

以一个有 1 台浏览器设备（完整设备）和 1 台机器的账户为例（更多机器同理）。

- 切换时**不需要写任何条目**：
  - 浏览器按 v2 映射成为「admin、不可运行 AI」；
  - 机器成为「可运行 AI、非 admin」。
- 切换的是**语义**：v3 的验证者认为这台机器也是设备，因此可以控制别的 host。
- **第一条 v3 条目是单独一步，由用户明确确认**（链 v3 评审，决定 D4）：「我的设备」里的「把账号升级到 v3」，写明旧版应用与 agent 之后跟不上这条链、且不能撤销。在此之前，添加、移除、注册、配对等一切日常操作在 v2 链上照旧写 v2（看链本身 / `chain_v3.in_use`，不看 Hub 的 `writable`）。恢复词在 v3 链上写 `RecoveryJoin3`（admin、绝不 host）。

**互通**

| 组合 | 结果 |
|---|---|
| 旧 agent（只认 v2）+ 只有 v2 的链 | 照常 |
| 旧 agent + 链里有 v3 条目 | 验链失败。agent 报链告警，停止接受通道；本机不受影响。**由门槛防止** |
| 新 agent + 只有 v2 的链 | 照常；能接受其它 host 发起的控制 |
| 旧 agent 收到 host 发起的通道 | 被拒（不是设备，4403）；控制端显示「这台设备的 Droppen 版本过旧」 |
| 旧 SPA 页签 + v3 条目 | 验链失败。版本横幅强制刷新；Service Worker 的预缓存跟版本走 |
| 旧 Hub + v3 条目 | Hub 拒绝追加（409），所以 Hub 先发版 |

**发版顺序**（N = 能读 v3，N+1 = 开始写 v3）

1. **N（读）**：`rh-proto` 的 Rust 与 TS 实现、Hub、SPA、agent 都能读、验、映射 v3，但都不写。agent 在 `agent_info.features` 里声明 `chain/3`。
2. **门槛**：账户里每台当前 host 最近一次签入时都声明过 `chain/3`，Hub 才接受这个账户的 v3 条目，否则回 409 `chain_v3_not_ready` 并列出卡住的 host。客户端从 `/v1/me` 的 `chain_v3`（`mode`、`writable`、`waiting`、`in_use`）读出账户此刻能不能写 v3（`docs/hub-api/http.md`）。
3. **控制先于写入**：门槛一过，host 就可以发起控制（不需要任何新条目）。
4. **N+1（写）**：SPA 在门槛满足时写 v3 条目（添加设备、`SetAdmin`、`SetHost`）；不满足时，仍能用 v2 表达的操作照旧写 v2（加完整或受限设备、`AddMachine`）。

**回滚**

- **第一条 v3 条目写下之前**：任何一步都可以退回旧版本。新账户从创世起就是 v3，没有这段「之前」：Hub 拨回 `read` 或 `off` 时，注册停在创世这一步，直到拨回 `write`。
- **之后**：读者不能退到 N 以下，因为链只追加、带哈希链接，不能删条目。
  - v3 链上不再写 v2。Hub 暂不收 v3 条目时（`chain_v3.mode` 不是 `write`），只拒绝「新加 host / 重新运行 AI」的批次；移除、降权、轮换、恢复、配对设备、接受策略照收，落后的 host 也拦不住。
  - `mode: off` 时，host 的设备角色（设备登录、设备会话、设备 WebSocket、中继票据）一律被拒（`host_devices_off`）。
- **已有账户写下第一条 v3 条目，是一个单独、由用户确认的步骤**（D4）。

### 3.6 安全评审清单（SC-5 的门槛）

由独立评审逐条核对。每条都要有「先失败、后通过」的回归测试。

1. v2 → v3 映射在 Rust 与 TS 中逐字节一致（互通向量覆盖每种 v2 动作）；v2 链在新代码下重放的结果与旧代码完全相同。
2. 版本混淆：v3 条目不能被解析成 v2，反之亦然；flags 的未知位和非规范编码被拒绝。
3. 非 admin 签的任何条目都无效；`RSK` 只签两种条目；`AK` 只签创世（含 `Genesis3`）。
4. 不能给自己加权限（admin、host）；最后一台 admin 不能被关或移除；批内降权之后，同一签名者后续的条目无效。
5. `MK` 提交、流头、manifest、草稿只认 host；`SetHost false` 之后旧钥签不出新流头；`SetHost true` 必须带新的 `MK` 轮次。
6. 非 admin 设备永远拿不到 `UK`：检查信箱里没有给它的用途 1，钥环名字也不让它写。
7. 用途 4 只能由非 admin 发给 admin；回复里只有 `MK`。
8. 新鲜度按操作：控制 7 天、管理 12 小时；只有 admin 能作证；通道里每条管理类子流都按 12 小时重新检查，开着的通道满 7 天没人作证就被关掉。
9. 同一把钥在 Agent、DeviceSession、RelayAgent、RelayDevice 四个角色下的签名，换到别的角色里一律无效。
10. Noise：拒绝自己连自己；prologue 绑定发起方与响应方；在同一把静态钥两种角色都用的前提下，KCI 论证仍然成立。
11. 中继：host 设备的 agent 连接与它的设备连接，是两条票据、两种角色；agent 角色仍不能 `OPEN`。
12. 非 admin 设备经通道调用拒绝清单内的路由一律 403（§5.4），包括 `/api/cloud/*` 与 `/api/agg/*`。
13. 告警：每次加设备、开 admin、host 加入都告警，Hub 扣不住（agent 从自己那份链上告警）。
14. 套餐：`devices.max`、`hosts.max` 在 `AddDevice*`、`SetHost true`、注册时执行；恢复加入不受限制（规则同今天）。
15. 门槛：Hub 在门槛不满足时拒绝 v3 条目；没声明 `chain/3` 的 agent 拿不到 v3 条目（Hub 不推，它也验不过）。
16. 迁移：已有账户的创世与钉住的指纹不变；切换前后各验证者的链头相同。
17. 通行密钥：`Passkey(slot)` 只能签 `flags = 0` 的待定加入，以及它自己那个槽的激活；`grants` 不为 0 的槽被拒绝；作废规则（`via_passkey`、`set_by`）在 Rust 与 TS 中一致；激活的 24 小时按验证者自己的首见时间计算；通行密钥包里只有 `MK`，没有 `UK`。

**评审之后的决定（2026-10，链 v3 两份独立安全评审）**

- **D1 host 不能是 admin**：见 §3.1 的暂定说明。
- **D2 机器钥跟着接受策略走**：一台 host 的 `MK`（用途 5 的 host 钥、用途 4 的自动回复）只给它的接受策略放进来的非 admin 设备；Hub 在链锁下按当时的链检查用途 5 的收件人（`not_accepted`、`bad_host_key`）。收窄策略或把设备移出名单，验证者要求紧接着一条该 host 的 `KeyEpochs`（`HOST_KEY_NEXT`），即轮换它的 `MK`。
- **D3 能控制一台 host = 完全掌控那台电脑，包括它的身份钥**：拒绝清单不是边界。界面在授予控制处明说；能控制它的设备被移除或移出名单后，轮换它的 `MK` 并提示重新注册（换身份钥）。**提示里附上证据，但从不因证据而撤掉**：agent 把每台握手被它接受的设备记在 `state.json`（`controllers`，在回复发出前和重放保护一起保存；还不在记录里的设备存不下就拒绝这次握手（关闭码 4507，页面说明「这台设备存不下谁连接过它」，请检查磁盘），已在记录里的照样放行——磁盘满不能把所有设备都关在外面；至多 256 台），写进加密状态流（`controllers: {from, fromMs, seq, devices: [{id, firstMs}]}`，空也写；`/rh/info` 也有）。`from` 是记录完整的起点：新注册为 0，旧版 agent 升级时为当时的链 seq。「我的设备」照旧对每台失去控制权的设备提示重新登记，并逐台写明证据：记录里有它——「连接过（首次 …），强烈建议重新登记」；记录覆盖了它能控制这台电脑的全部时间（写于失去之后、起点早于它或这台电脑加入账号）却没有它——「自 … 起没有它连接的记录」；其余（旧 agent、记录起点更晚）——照旧的措辞。记录只是证据：握过手的设备拿到过这台电脑的 shell，能改 `state.json` 把自己删掉再重启，之后真实签名的状态流也不会列它。所以只有人能撤掉提示：每条可点「我确定它从没连接过——不再提醒」，确认框写明含义；按（host, 设备, 那次失去的 seq）记在这个浏览器的账号存储里，之后以低调的「已不提醒」列着，可「重新提醒」；同一设备再失去一次、或任何记录列出过它（页面也把列出过的设备记在本地，只记链上有的设备、从不挤掉，记录里塞满编造的 id 也挤不走真实的；读取时不认识的 id 只是不读、不删（各标签页共用，链稍旧的那个不认识刚加的设备），一台设备的记录只在它被移出账号时才清掉，停止运行 AI 不算）都会重新提醒；重新获得控制权的设备（重新列入名单、重新成为 admin）不再算失去，之后再失去是新的一条。记录说连接过的设备不提供这个选项。（另行设计中：自动轮换 host 身份，让重新登记不再必要。）纵深防御：本机 fs 路由对 `cloud:*` 请求拒绝实例数据目录和 `~/.config/remote-hand`（只读放开受管 Claude 根下转录会链接的 `plans/`、`image-cache/`、`projects/<p>/memory/`、`projects/<p>/<s>/tool-results/`，以及 `uploads`）；admin 路由清单改成全量分类，有测试保证每条注册的路由都被分类。围栏跟着配置走：`REMOTE_HAND_CLAUDE_MANAGED_ROOT` 的覆盖会移动它要挡的目标；它只挡本实例自己的目录，`~/.claude/.credentials.json` 等家目录下的凭据**从不**在围栏内——能控制就是能读（D3）。
- **D4 第一条 v3 条目单独确认**：见 §3.5。
- **恢复钥跟着知道它的设备走**（写入闸门）：知道**当前**恢复词的设备——当前恢复词是它设置的，或它正是用当前这组恢复词回来的（记录在 `via_recovery_key`/`viaRecoveryKey`）——被降权（`SetAdmin false`）或按 v3 移除（`RemoveDevice3`）时，恢复钥作废，直到另一台 admin 设新的——否则之后每个新 `UK` 仍按它知道的恢复词封一份，而 `GET /v1/keys` 把恢复包给任何当前设备。它回来之后才设的新恢复词它不知道，保留。v2 的 `RemoveDevice` 语义不变（凡设置过或用恢复词回来过就作废当前恢复钥），旧链重放结果与以前逐字节相同。
- **用户钥**：任何**曾经**持有 `UK` 的设备（曾是 admin，或任何 v2 完整设备）被移除时轮换 `UK`；`SetAdmin false` 之后也记为待轮换；Hub 在降权条目入库的同一事务里删除还在信箱里等它的用途 1。
- **信箱分角色**：host 的 agent 只读、只确认用途 2（自己的 `MK`）；它的设备角色读、确认其余用途，互不吞对方的信。
- **钥环清扫的范围**：移除机器（或 host）只删与它相关的信，不扫钥环；Hub 只在移除非 host 设备或换恢复钥时按链扫钥环（同 v2）。
- **套餐键改名**：`machines.max` 改叫 `hosts.max`；`devices.max` 按新含义（含 host 在内的全部当前设备）计。

### 3.7 界面：「我的设备」

droppen.app 的「设备」页与「机器页」合成一页：**我的设备**，没有筛选。本机设置的「Droppen」一节里也有它的只读版本。自上而下：

- **用量**：套餐、本期流量（经中继转发的，直连不算；用完时写明已限速）、云端存储（「按设备」展开各设备的离线副本）、设备数与可运行 AI 的设备数，各对上限；下面一行写离线副本保留多久。
- **这台设备**：只属于这个浏览器的设置——「允许直连（更快）」（直连更快、不计流量，但对方设备与 STUN 服务商会看到这台设备的 IP 地址），以及折叠的中继选择。以后这台设备自己的设置也放这里。
- **设备**：一张列表，可运行 AI 的设备在其中。

| 列 | 内容 |
|---|---|
| 名称 | 类型图标（浏览器、手机、Droppen）与人认得出的名字：可运行 AI 的设备用它自己的名字；浏览器与手机用 hub 从它自己请求的 User-Agent 归纳的短名（「iPhone · Chrome」，`/v1/me` 的 `ua_label`，只认固定词表），都没有时是短 id；等待批准的设备一律显示 id（用恢复词回来的设备自己选 User-Agent，不能让它冒充熟悉的手机） |
| 徽章 | 「本设备」「可运行 AI」「主设备」「完整设备」或「受限设备」；该重新登记的 host 名字旁有琥珀色「需重新登记」 |
| 状态 | 在线、离线、未知（状态方块，只有 host 与本设备有）；路径与延迟；不在线时「最近在线」；待批准、已移除 |
| 会话 | host：会话数与工作中 / 在问你 / 未读；离线时「离线副本 · 截至 14:02」 |
| 详情 | 末行的 id 就是开关：设备 ID、指纹、路径、主机名、系统、agent 版本、内容密钥代数、谁可以控制它、重新登记的证据与「不再提醒」、最近会话 |
| 操作 | host 有「打开」（离线有副本时「读副本」）；其余都在「⋯」菜单：打开终端、改名、谁可以控制它、停止 / 重新运行 AI、设为完整设备、重发钥匙、移除（勾选「同时轮换内容密钥」，默认勾上；`MK` 在界面上叫内容密钥，见 §3.1 用词）；暂时不能按的项留在菜单里并写明原因（还没升级账户、设备离线、hub 还不收、有激活被暂扣） |

已移除的设备折叠在页面底部（「已移除的设备（n）」）。

**开关的规则**

- 只有 admin 设备能拨；非 admin 设备看到的是只读列表，并写明「这台设备不能管理账号」。
- 自己那一行只能关、不能开。最后一台 admin 的开关置灰，悬停时说明原因。
- 给可运行 AI 的设备开管理权时，确认框写明三件事：它会持有用户钥、能加删设备；无人值守的电脑建议保持关闭；若它的钥在普通文件里（命令行安装），同一台电脑上的程序都能读到。

**添加设备**只有一个按钮「添加设备」，先问加哪一种，再进各自的流程：

- **「添加浏览器或手机」**：配对（扫码、口令、文本），新设备「可管理账号」默认开，可以先关掉再确认。
- **「添加可运行 AI 的设备」**：首选在那台设备的本机设置里「登录 Droppen」（§4）；备选扫码或命令行注册码。「可运行 AI」由它自己声明。「可管理账号」的默认值：Tauri 应用开，命令行 / launchd 安装与无人值守服务器关。

**「添加可运行 AI 的设备」是一个不用人带的引导**（`web/src/features/cloud/add-computer.tsx`）：

1. 在那台设备上安装 Droppen App（macOS，`getdroppen/droppen` 发版里的 `Droppen-<版本>.dmg`；打开即启动，没有登录页），与分发仓库的约定都在 `web/src/features/landing/get-droppen.ts`。第一个 App 发布前（`APP_AVAILABLE` 关），这一步是安装命令 `curl -fsSL https://droppen.app/install.sh | sh`（后台运行）并写明「App 即将推出」，不链接不存在的文件。
2. 在 App 里（或后台安装时它自己打开的本机页面上）点「登录 Droppen」（S1-3 的 `/link` 弹窗）：在 droppen.app 窗口里登录这个账户、核对两边的指纹、点「加入这台设备」；那个浏览器还不是账户里的设备时，先在引导里「添加浏览器或手机」配对它。
3. 「正在等新设备上线…」：只看 shell 的机器列表（链跟随）和在线状态，不拨号；新设备出现并在线后，账户里还有别的可运行 AI 的设备，就显示「现在可以把两台设备放进同一个屏幕」，按钮在主设备上用屏幕 API 建一个左右分屏（每格写自己的 `PaneContent.machine`）并打开：每格直接打开那台设备最近的会话（来自它的索引副本），没有会话就在那台设备上开一个终端，连不上才退回到那台设备的选择器。

「服务器或后台运行？」是第二条路：下载页的 Mac 后台运行 / Linux 服务器两个标签，以及一次性注册码（命令行注册；`--code` 无浏览器注册随 Linux 支持一起来，`LINUX_AVAILABLE`）。下载页 `droppen.app/download` 按访问的设备自动选中标签（`detect.ts`：userAgentData 与 UA，iPadOS 自称 Mac 时按触屏判为平板，Mac 不猜架构），标「推荐」，五个标签（Mac · App、Mac · 后台运行、Linux 服务器、Windows、手机 / 平板）一键可切，地址 `?os=` 可分享，手动选择只记在本次会话，地址优先。只有一台可运行 AI 的设备时，仪表盘、设备切换和窗格选择器各有一行「再加一台设备，就能在一个屏里同时看」；有两台以上后，顶栏下方一次性出现一行提示「每个格子都可以放不同设备的内容」（单独占一行，不压住任何窗格），带「新屏幕」。两者都可关掉，按浏览器和账户记住。本机页面从未登录过 Droppen 时，会话页正中有一张首次运行卡片（「登录 Droppen」），本机鉴权不变。

原来的「机器页即仪表盘」（「我的设备」按「可运行 AI」筛选）已并进这一页：每台 host 的行上有会话计数与「打开」，跨设备「需要你」台账在列表下方、有内容时才出现（§6.4）。

### 3.8 套餐：设备数与可运行 AI 的设备数

| 键 | 含义 |
|---|---|
| `devices.max` | 链上当前设备的总数，不分是否 admin、是否 host |
| `hosts.max` | 其中可运行 AI 的设备数 |

> 各套餐的具体数值是商业内容，不在公开仓库；客户端只读 `/v1/me` 给出的上限与用量（`docs/hub-api/`）。

**理由**

- 可运行 AI 的设备才产生存储（加密流）和大部分中继流量，所以单独限制。
- 浏览器、手机、非 admin 设备都只算进 `devices.max`。

**在哪里执行**

| 动作 | 检查 |
|---|---|
| `AddDevice*` | 两个上限都查（host 才查 `hosts.max`） |
| `SetHost true` | `hosts.max` |
| `POST /v1/enrollments` | 当前 host 数 + 未完成的注册 < `hosts.max` |
| 恢复加入 | 不受限制（规则同今天） |
| `ApproveDevice` | 可到 `devices.max + 1`（规则同今天） |

**降级**：已有的设备照常可用，只是不能再加（中继文档 §3.7 的规则不变）。

**其它**：`machines.max` 保留为 `hosts.max` 的别名一个版本。界面文案见 §3.7。

## 4. 在本机界面登录 Droppen

### 4.1 用户看到什么

设置里新增一节「Droppen」，只在本机模式出现。

| 状态 | 显示 | 操作 |
|---|---|---|
| 未登录 | 「登录后可以在手机上用 droppen.app 访问这台电脑，也能在这里一起看你的其它电脑。不登录也一样能用。」 | 主按钮「登录 Droppen」。高级选项：Hub 地址（默认 `https://droppen.app`） |
| 本机没有账号 | 「先在『安全』里为这台电脑建一个本机账号：登录 Droppen 后其它电脑会经这里可达，不能对任何人敞开。」 | 跳到「安全」 |
| 进行中 | 「在弹出的 droppen.app 窗口里继续」，下面是这台设备的指纹（8 组 4 位 hex） | 「重新打开窗口」「取消」 |
| 已加入 | 账户邮箱与指纹；这台设备的名字、id；「可运行 AI」；「可管理账号：关」（只读，在 droppen.app 上改）；链接状态（「已连接 · 经东京中继」或「断开 · 12:03 起」）；能看到几台其它设备；最近一次作证（「手机 · 2 天前」）；哪些本机账号能看其它设备 | 「在本机界面里显示其它设备」开关；「断开这台电脑」 |
| 已被移出 | 「这台电脑已在 droppen.app 上被移出账户，本机照常可用。」 | 「重新加入」 |
| 出错 | 链告警、钥文件权限不对等 | 说明原因；「断开这台电脑」 |

### 4.2 凭据去哪：只去 droppen.app

**规则**

- 密码、邮箱验证、Turnstile、恢复词、配对口令，都只在 droppen.app 的源上输入。
- 本机页面和本机 `droppen` 永远拿不到：密码、Hub 的登录会话、任何 admin 设备的设备会话。

**理由**

1. 本机页面常常是 `http://192.168.x.x:8700` 这样的明文源，在这里输密码，密码会在局域网上明文传一趟。
2. Hub 没有 CORS，Turnstile 只能在应用源完成。为本机放开它们，等于让任何网页都能代用户登录。
3. 本机 server 只需要一个结果：把自己加进账户。
4. 以后上 passkey，RP ID 是 droppen.app。
5. 不教用户在别的页面上输入 Droppen 密码。
6. 本机程序是开源的，它不经手凭据这一点，任何人读代码就能确认。

**本机拿到什么**

- 一个 RH1 注册码：只能注册一台设备，10 分钟有效，要出示证明，与命令行用的码相同。
- 之后，本机用**自己唯一的设备钥**签挑战，换取设备会话（`POST /v1/session`），不需要任何密码。

### 4.3 流程

![在本机设置里登录：本机备好公钥 → 弹窗在 droppen.app 登录并同意 → 注册码经 postMessage 交回本机 → 进程内注册 → 弹窗上链、交钥 → 本机不重启即上线](local-aggregator/signin.svg)

参与者有四个：本机页面 P（已用本机账号 U 登录）、弹窗 W（`<hubWeb>/link`，即应用的源上的 `/link`，例如 `https://my.droppen.app/link`；一台 admin 设备）、本机 `droppen` S、Hub H。

`hubWeb` 是应用的源（S 读 Hub 的 `/.well-known/droppen-hub`，见 §4.9 与 `docs/hub-api/link.md`）。旧版本机（v0.2.103–v0.2.128）不读它，打开的是 `https://droppen.app/link?v=1`，它只接受来自 `https://droppen.app` 的应答；应用搬到 `my.droppen.app` 后，那里是搬迁提示（`web/src/features/hosts/legacy.tsx`）：这个浏览器在那里有设备时先提议搬移，再请用户关掉窗口、在 Droppen 里选「改在浏览器标签页里继续」（§4.9，v0.2.125 起才有，交回不绑 Hub 的主机）；v0.2.103–v0.2.124 没有这一项，要先更新 Droppen，或用一次性注册码加入。

**1. 发起**

P 在点击事件里同步调用 `window.open("<hubWeb>/link?v=1", "droppen-link", "popup,width=480,height=760")`，URL 不带数据。同时 P 调用 `POST /api/cloud/link {hub?}`。

S 依次做：

- 检查前提：本机鉴权开着；U 是云端所有者，或者还没有所有者；实例没注册（已注册的不必再走这一流程，见第 7 步后的说明）。
- 备好机器种子：没有就生成，存在 `agent/machine-key.json`。这是这台设备**唯一**的身份。
- 开一个 attempt：`{id, nonce: 16 字节随机, owner: U, hub, expires: 15 分钟}`，只放在内存。
- 回复 `{attempt, nonce, hubWeb, device: {hostname, sign_key, dh_key, host: true}}`。

**2. 握手**

- W 要求 `window.opener` 存在，向它发 `{t:"rh-link/ready", v:1}`（不含秘密，`targetOrigin: "*"`）。
- P 只接受同时满足两个条件的消息：`event.source` 是它打开的窗口，`event.origin === hubWeb`。
- P 回 `{t:"rh-link/request", v:1, nonce, device}`，`targetOrigin` 写死 `hubWeb`。
- W 记下 `cb = event.origin`。

**3. 登录**

W 未登录时，先登录或注册。W 所在的浏览器还不是 admin 设备时，先在弹窗里完成以下现成流程之一：

- 新账户：创世（链 v3 的 `Genesis3`，§3.5）+ 12 个恢复词。这台浏览器成为第一台 admin。
- 已有账户：扫码或口令配对，由**已有的 admin 设备确认**。配对时新浏览器的「可管理账号」默认开。
- 只剩恢复词：恢复加入后是**待定**设备，要另一台 admin 批准，或 24 小时后自行激活。
  - 这期间 W 显示「等待批准，最早 明天 14:02」；attempt 过期后，P 显示「批准后再点一次登录」。

**4. 同意**

同意页有中、繁、日、英四种语言，内容：

- 「把这台电脑加入 Droppen 账户 <邮箱>」；
- 来源 `cb`，等宽大字；
- 主机名（仅作参考）；
- 设备指纹（公钥 SHA-256 的前 16 字节，8 组 4 位 hex，与 P 上显示的是同一串）；
- 「可运行 AI」（只读，由这台电脑声明）；
- 「可管理账号」开关，默认**关**，旁边说明理由（§3.3）。

`cb` 要过 §4.4 的规则。设备数或可运行 AI 的设备数已满时，直接说明并给出套餐文案。

**5. 注册**

- W 生成秘密，调用 `POST /v1/enrollments {device, token}`，组出 RH1 码。
- W 调用 `postMessage({t:"rh-link/code", v:1, nonce, code}, cb)`。注册码不进任何 URL，也不经过 Hub。

**6. 交给本机**

- P 调用 `POST /api/cloud/link/finish {attempt, nonce, code}`。
- S 校验：attempt 没过期，nonce 相符，调用者是 owner。
- S 在进程内跑 `agent enroll` 的同一段代码，用第 1 步备好的种子：`/v1/agent` → `enroll {token}` → `enroll_assigned` → `enroll_proof`。
- S 回 202。

**7. 上链与交钥**

W 轮询到 `proved` 后检查两件事：证明成立；**并且**证明里的公钥等于 request 里的公钥。

检查通过后，W 写入以下条目：

| 链 | W 写入的条目 |
|---|---|
| v3 之前（S1-3 今天的实现） | `[AddMachine(A), KeyEpochs(MK_A 第 0 轮)]` |
| v3 之后 | `[AddDevice3{A, host, admin: 用户的选择}, KeyEpochs(MK_A 第 0 轮)]` |

然后 W 依次做：

1. 写钥环 `mk/A/0/<UK 轮>/<W>`；把 `MK_A` 交给 S 的 agent（与今天相同）。
2. 若 A **非 admin**：把每台其它 host 的每一轮 `MK` 封给 A（信箱用途 2），A 才能控制它们、读它们的副本。若 A **是 admin**：改为把 `UK` 各轮封给 A（用途 1，与配对相同）。
3. 为新链头签 `ATTEST`。
4. 调用 `postMessage({t:"rh-link/done", v:1, nonce, account, email, device: "A"}, cb)`，2 秒后关窗。

**已注册的电脑（用命令行注册过的）**

不用再走弹窗。链 v3 的读者上线、过了门槛之后（§3.5），它自动就能控制其它设备。缺的 `MK` 由它发索钥请求，在线的 admin 自动回复（§3.3）。账户不一致时，弹窗拒绝：「这台电脑已加入另一个账户」。

**8. 本机起来**

- S 的注册任务在链上看到自己后：写 `state.json`，**在进程内启动链接**（S1-1，不重启）。
- 控制端随之启动，用的是同一个身份：换设备会话 → `/v1/client` 签入 → 取信箱里的 `MK`（逐个对照链上提交）→ 订阅各 host 的流（§6.4）。

**9. 收尾**

- P 收到 `done`，或 `GET /api/cloud` 变成 `linked`，就刷新这一节。
- 弹窗被提前关掉也不影响：attempt 过期后清理即可，种子留着下次用。

**目标**：用户已经是 admin 设备时，从点按钮到在线，10 秒以内（不含用户操作的时间）。

**与 S1-1 到 S1-3 的关系**：它们只涉及注册与 v2 的 `AddMachine`，在新模型下完全有效。为单独「聚合器」准备的字段（S1-2 的 `aggregate` 与 `aggregator` 公钥、S1-3 的 `POST /v1/devices` 的 `kind/host`），在新模型里不再需要。已实现的留着无害，SC-2 时一并清理。

### 4.4 弹窗握手的安全规则

**消息**

- 六种，都带 `v:1`：`rh-link/ready`、`request`、`code`、`done`、`cancel`、`error {code, message}`。
- 只处理同时满足三个条件的消息：来自已知窗口、已知源、nonce 相符。

**`cb`**

- `cb` 只取 `event.origin`，不接受 URL 参数里的回调地址。
- 不警告的源（端口不限）只有这台电脑自己：`localhost`、`*.localhost`、`127.0.0.0/8`、`[::1]`。
- 其它源，**包括局域网与 Tailscale**：显示红色警告「这个地址不像是你自己电脑上的 Droppen」。用户要多勾一项「我确认 <主机> 是我自己的电脑」才能继续，并比对指纹。
  - 私网 IPv4（10/8、172.16/12、192.168/16）、`fd00::/8`、`*.local`、`100.64/10`：都是「浏览器所在网络上谁应答就是谁」，同一个咖啡馆 Wi-Fi 上的人就能伪装。
  - `*.ts.net`：Tailscale Funnel 免费给任何人一个公网 `https://x.<tailnet>.ts.net`。（实现评审发现：起初这几类都不警告，一个假设置页点一下就能拿走注册码。）
- Droppen Hub 自己的主机名（任何协议与端口）永远拒绝；Hub 在 `/.well-known/droppen-hub` 里列出主机时，它的源、应用与站点主机，以及源之下的任何名字（`*.droppen.app`），一样拒绝（`web/src/features/link/handoff.ts` `callbackTier`）。
- **已知、暂缓**：
  - 同一台电脑回环上另一个端口的程序也算「本机」，不警告；
  - 同意页开着时，opener 可以再发一个 `request` 把它换掉（同源）。

**两类攻击**

- **恶意网页打开 `/link`**，收走注册码，注册它自己的电脑。
  - 主要靠同意页挡：来源、指纹、用户的主动点击。
  - 漏过时还有两道防线：所有设备对新 host 告警（§3.2）；`hosts.max` 限制。
- **登录 CSRF**：把攻击者账户的注册码塞进受害者的本机。
  - 挡法：`finish` 只接受本机自己发出的 nonce，且调用者必须是同一个本机用户；nonce 从不进 URL。

**其它**

- 弹窗页与本机页都不能设 `Cross-Origin-Opener-Policy: same-origin`，加一条测试守住。
- 弹窗被拦截时，P 显示一个由这次点击触发的链接。

### 4.5 本机 API、鉴权与 CSRF

新路由放在 `S/api/cloud.rs`。

| 路由 | 作用 | 谁能调 |
|---|---|---|
| `GET /api/cloud` | 状态，不含机密 | 任何本机用户 |
| `POST /api/cloud/link {hub?}` | 开 attempt | 所有者；还没有所有者时，任何本机用户 |
| `POST /api/cloud/link/finish {attempt, nonce, code}` | 交码，进程内注册 | 发起该 attempt 的用户 |
| `POST /api/cloud/link/cancel {attempt}` | 取消 | 同上 |
| `POST /api/cloud/aggregator {enabled}` | 是否在本机界面里显示并控制其它设备（本机的「聚合」功能）。关掉后：关闭网关，删掉本机存的其它设备的 `MK` 与副本 | 所有者 |
| `POST /api/cloud/unlink {deleteCopies}` | 断开这台电脑（§4.7） | 所有者 |
| `PUT /api/cloud/access {user, remote: bool}` | 哪些本机账号能看、能操作其它设备 | 所有者 |
| `GET /api/agg/ws?ticket=` | 本机网关（§6.3） | `remote` 为真的本机用户 |

**规则**

- **都走现有的两层**：先过 `auth::guard`（bearer），再过 `origin::layer`。
- **没有 CSRF 面**：没有 cookie，bearer 在 localStorage 里，跨站请求带不上。
- **本机鉴权必须开着**：没有本机账号时回 409 `local_auth_required`；账号删光后，网关立即关闭。
- **`cloud:*` 身份一律回 403 `not_from_cloud`**：经通道进来的设备不能改本机的云端链接，也不能借网关碰别的设备。
- **存储**：所有者与远端权限存在 `<实例>/cloud.json`（0600）。

### 4.6 本机多用户与多实例

- **多实例**（例如同一台电脑上端口 8700、8701 的两个实例）：每个实例是一台独立的设备，有自己的种子与注册，可以登录各自的账户。两个实例登录同一个账户时，就是两台设备，默认名字带上实例名。
- **一个实例里的多个本机账号**：今天没有角色，共享同一份数据。本机这台的会话照旧对所有本机账号可见。**其它设备是另一个信任域**，所以：
  - 发起登录的本机账号成为**云端所有者**，只有它能断开、关闭聚合、改远端权限；所有者账号被删时，剩下的任一账号都可以断开，免得锁死。
  - 其它本机账号默认看不到其它设备，由所有者逐个打开。
  - 网关按本机账号执行这一点，并在本机记审计（本机用户、设备、管道类型、路径）。

### 4.7 退出与解除

| 动作 | 在哪 | 本机做什么 | 云端怎么收尾 | 之后 |
|---|---|---|---|---|
| 关闭聚合 | 本机设置 | 关网关；删掉 `aggregator/`（其它设备的 `MK`、副本库） | 无需链上动作：它仍是账户里的设备，只是本机不再发起控制 | 这台电脑仍在运行 AI，手机照样能用它 |
| 停止在这台电脑上运行 AI，但保留它作控制端 | droppen.app「我的设备」（admin 操作） | agent 看到 `SetHost false`：停写流，不再接受通道 | 写 `SetHost false`；它的流按被移除的机器处理 | 本机仍能看、能控制其它设备 |
| 断开这台电脑 | 本机设置，或 `droppen agent unenroll` | 可选「同时删除云端副本」（逐条 `stream_delete`）；发 `retire`；删 `agent/state.json`、`machine-keys.json`、`streams.db`、`machine-key.json` 和 `aggregator/` | Hub 拒绝它再登录；任一 admin 下次上线时自动写移除，并轮换 `MK` | 本机回到「未登录」 |
| 在 droppen.app 移除这台电脑 | 「我的设备」 | agent 看到自己被移除：停链接、删控制端数据，本机数据不动；设置显示「已被移出」 | 现成的移除与轮换 | 「重新加入」= 清掉 agent 状态后重走 §4.3 |

退役（`retire`）只是请 admin 尽快移除的信号，签不了链。

### 4.8 云端不可达时

| 情形 | 本机会话、终端、设置 | 其它设备 | 「Droppen」一节 |
|---|---|---|---|
| 从没登录 | 照旧 | — | 「登录 Droppen」 |
| 登录时 Hub 不可达 | 照旧 | — | 弹窗报错；`finish` 回 502 `hub_unreachable`；种子留着 |
| 已加入，Hub 断开 | 照旧 | 已开的中继通道照常（票据 15 分钟，中继另有 2 小时宽限）；新通道要等 Hub；列表显示副本库的最后状态，标「截至 12:03」 | 「Droppen 断开 · 12:03 起」，自动重连 |
| 中继全挂 | 照旧 | 只读副本；同一局域网多半还能直连（前提是通道曾经建起来过） | 「没有可用的中继」 |
| 超过 7 天没有 admin 作证（§3.4） | 照旧 | 只读副本 | 「需要在一台可管理账号的设备上打开一次 Droppen」 |

### 4.9 没有弹窗时：经系统浏览器交回（桌面应用、拦截弹窗的浏览器）

弹窗与 `postMessage` 要求本机页面打开应用的源（`hubWeb`）、并留着 `window.opener`。有三种场合做不到：

- 桌面应用（§7.3）：窗口里的新窗口一律交给系统浏览器，`window.opener` 不存在；
- 浏览器拦截了弹窗（Safari 默认就会拦异步打开的窗口）；
- 只有手机是 admin 设备（扫码，见本节末尾，未实现）。

前两种已实现（2026-10-19，「浏览器交回」）。不改 Hub：droppen.app 的同意页把注册码**封装**后，用一次顶层导航交回本机页面自己的源。

**1. 发起**（`POST /api/cloud/link`，与弹窗共用一个 attempt）

- 请求带 `Origin` 时（本机页面发起的都带），S 除了 nonce 之外再为这个 attempt 生成一把一次性 X25519 钥 `E` 和 16 字节 `state`，只在内存里，随 attempt 一起过期、用掉或取消。
- `hubWeb` 是浏览器登录 Hub 的地址，弹窗也在这里打开：S 问 Hub 的 `/.well-known/droppen-hub`，取其中的 `app`（如 `https://my.droppen.app`，须是 Hub 自己的主机或它的子域）；没有、或不合规则，就用 Hub 自己的源（`https://droppen.app`）。S 自己仍连 `wss://<hub>`。规则与各版本的行为见 `docs/hub-api/link.md`「Which host the popup and the hand-off open」（`server-rs/src/api/hub_web.rs`）。
- 回复多一个 `handoffUrl`：`<hubWeb>/link#droppen-link=<base64url(JSON)>`。JSON 里只有：attempt id、`E` 的公钥、`state`、页面的源（取自 `Origin`）、过期时间、主机名与设备两把公钥。全部在片段里，不进查询串；S 不记日志。
- 桌面应用：页面 `window.open(handoffUrl, "_blank", "noopener")`，应用把它交给系统浏览器。应用往外交地址时一向去掉片段（本机地址的片段可能是登录令牌）；只有形如 `/link#droppen-link=…`、又不是应用自己服务器的地址保留片段（`desktop/core/src/sign_in.rs`）。
- 浏览器：弹窗被拦或被关后，「改在浏览器标签页里继续」用同一个 attempt 打开这个地址（点击里打开，不会被拦）。

**2. 同意**（应用的源上的 `/link`；旧版本机给的 `https://droppen.app/link#droppen-link=…` 经搬迁提示原样转到 `my.droppen.app`，片段一起带过去）

- 页面最先运行的模块（`fragment-boot.ts`）把片段从地址栏拿掉，存进这个标签页的 `sessionStorage`（登录、注册途中刷新还在），答完或取消就删。
- 没有 opener 时用这份请求：`cb` = 请求里写的源，§4.4 的三档规则照旧（本机回环不警告；局域网、Tailscale、公网要勾「这是我自己的设备」；Hub 自己的主机拒绝，Hub 在 `/.well-known/droppen-hub` 里列出的各个主机名——源、应用、站点——以及源之下的任何名字（`*.droppen.app`）也拒绝）。同意页同样显示来源、主机名、指纹，并提示「核对那台设备上的 Droppen 显示的是同一串指纹」。
- 过期（按 S 的时钟，留 2 分钟误差）就不再发码，提示回设备上重来。

**3. 交回**

- 点「加入这台设备」时，**在这次点击里**先开一个空白标签页（弹窗拦截器只放行点击里同步打开的窗口），再申请注册码。
- 码用 `rh_proto::link_return`（TS 侧 `lib/e2e/link-return.ts`，两边有逐字节相同的测试向量）封装：`w ‖ nonce ‖ AES-256-GCM`，`kek = HKDF-SHA256(salt = "droppen/v1 link-return", ikm = X25519(w, E) ‖ state, info = w ‖ E ‖ context)`，`context` 与 AAD = attempt id、页面的源、设备两把公钥。明文是 `{code, email}`。
- 空白页去掉 opener 后跳到 `<cb>/link/return#droppen-return=<attempt>.<密文>`；没开成就显示一个链接让用户点。droppen.app 这个标签页留着，照常等证明、上链、交钥（§4.3 第 7 步），完成后说「回到那台设备上的 Droppen」。码封好之前的任何出口（套餐上限、链 v3 还写不了、出错、取消）都把空白页关掉；连点两次只发一个码。
- 设备拒收（已取消、已在账户里、连不上 Hub、发起人不再管理）只在它自己的标签页里说，droppen.app 听不到：等证明时同意页留着「取消」，并提示「设备的标签页说没收下就在这里取消，回设备上重来」。让 droppen.app 直接知道，留作后续。

**4. 本机收下**（`/link/return` 与 `POST /api/cloud/link/return {attempt, sealed}`）

- `/link/return` 是本机 SPA 里唯一不经路由和登录的页面：系统浏览器在本机没有登录。它把片段从地址里拿掉，原样 POST 给自己的服务器，显示结果，2.5 秒后自己关掉（droppen.app 开的标签页可以关）。
- 这条路由不要登录（`auth::exempt` 里第五项）：答复本身就是凭据。打不开（别的 attempt、别的 state、改过）回 403 `bad_reply`，不消耗 attempt；attempt 不在 404、过期 410、没有交回材料 409 `no_handoff`；`cloud:*` 403、本机鉴权关着 409，与其它路由相同。这台设备已在账户里，要等答复打开之后才说（409 `already_enrolled`，attempt 用掉）：在那之前，来问的可能是任何连得上这个端口的人。
- 打开后在锁里**用掉** attempt（与 `finish` 抢同一个，谁先谁赢），再按 attempt 的发起人检查仍可管理，之后与 `finish` 完全相同：进程内注册，发起人成为所有者。
- 结果记在内存里（成功：机器 id；失败：错误码与原因），`GET /api/cloud` 的 `handoff` 只给发起人看，保留到 attempt 的寿命。

**5. 本机页面与应用**

- 设置 → Droppen 显示「在浏览器里继续」：三步说明、指纹、链接剩余时间、重新打开浏览器、复制链接（旁边写明不要外传）、取消（`browser-sign-in.tsx`，引导页以后复用）。它每秒问 `GET /api/cloud`：`handoff` 成功 → 「等待 droppen.app 把这台设备加入账户」；失败 → 显示原因；attempt 没了也没有结果 → 「已超时」；`linked` → 完成。
- 应用看到自己交出了这样一个地址，就用窗口的登录每 2 秒问一次 `GET /api/cloud`，最多 26 分钟（attempt 15 分钟加注册码 10 分钟）；状态从别的变成 `linked` 时把窗口拉到最前。
- 设置里的 Droppen 一节和会话列表上的首次登录卡片在应用里重新出现（密码修改、退出登录仍隐藏）。

**为什么不经 Hub 会合**：会合需要 Hub 新开一个不登录就能取的信箱；顶层导航回本机源就够了，Hub 不变，也看不到任何东西。代价是 droppen.app 的标签页必须在**同一台设备**上（回环地址只在这台设备上有意义）——扫码用手机登录的场景仍需 Hub 会合，留到以后：载荷与封装原样复用，只把「跳回本机」换成「放进 Hub 的会合点」。

**已知、接受**

- 发出的地址（含 `state`）和交回的地址（只有密文）会进系统浏览器的历史。前者只对这次 attempt 有效，后者没有 `E` 的私钥打不开，`E` 随 attempt 消失；能读浏览器历史的已是本机上的恶意程序。
- 任何网页都能伪造一个 `#droppen-link=`，把码封给它自己的钥、送去任意源：挡法与「恶意网页打开 `/link`」相同（来源分档、指纹、用户点击），回环上要有它自己的程序在听才拿得到——与 §4.4「回环上另一个端口的程序也算本机」同一个已知风险。
- 用户在另一台设备的浏览器里打开复制的链接时，交回的回环地址到不了这台设备；droppen.app 会一直等证明直到超时。界面提示「粘贴到这台设备上的浏览器里」。
- 复制出来的链接在 attempt 有效期内（15 分钟）就是凭据：谁拿到它，谁就能用**自己的**账户发码、封给这台设备——对听在局域网或 Tailscale 上的安装直接 POST 交回，或者骗主人在这台设备上打开交回链接——这台设备就进了别人的账户。界面在「复制链接」旁明说：谁打开这个链接谁就能把设备加进自己的账户，不要发给任何人，只在这台设备上打开。更稳的做法是注册前在本机页面上显示并确认账户（邮箱或账户指纹），留作后续：答复里的邮箱是发码方自己说的，可信的账户身份要等 Hub 分配之后才有。
- 升级顺序：droppen.app 必须先部署。旧的 droppen.app 收到这样的地址只会显示「请从设备的设置里打开」。

## 5. 本机作为控制端：A 还是 B

### 5.1 两个方案

- **A**：本机 `droppen` 这台设备用**自己的身份**，出站开通道到其它可运行 AI 的设备（经中继或直连），并读它们的离线副本；本机的 SPA 经本机网关使用。浏览器不持钥。
- **B**：本机源上的浏览器（例如 `http://127.0.0.1:8700`）作为另一台设备，同一份云模式代码在本机源上运行，由浏览器自己连 Hub 与中继。

### 5.2 对比

| 维度 | A：本机这台设备自己控制 | B：本机源上的浏览器作设备 |
|---|---|---|
| 局域网手机（`http://<局域网 IP>`） | **能**：手机只连本机 | **不能**：非安全上下文没有 `crypto.subtle` |
| 身份 | 一台电脑一个身份（运行 AI 与控制合一，§3） | 电脑一个，每个浏览器每个源再各一个 |
| 权限 | 默认非 admin：不持 `UK`、不签链、不作证 | 读离线副本得有 `MK`，管理得 admin；实际上会是 admin |
| 占设备数 | 不额外占 | 每个浏览器乘以每个源（`127.0.0.1` 与 `localhost` 不同） |
| 后台 | 常驻：副本常温，「需要你」随时算好 | 只有页面开着时才有 |
| 新鲜度 | 控制类 7 天（§3.4） | 同左；若是 admin，管理类 12 小时 |
| Hub 改动 | 小 | 要开 CORS；Turnstile 在 localhost 上做不了；CSP 要放行所有中继 |
| 代码复用 | 浏览器复用 `tunnelTransport`、`offlineTransport`、fold；Rust 复用 `rh-devkit` 的设备端 | 整套云模式搬到本机源上跑 |
| 前端投递（`docs/web-integrity.md`） | 浏览器里没有钥；SPA 由本机二进制提供 | SPA 由本机提供 |
| 主机被攻破 | 拿到设备钥：在被移除前能控制其它 host；不是 admin 就签不了链、拿不到 `UK` | 同一台主机上的浏览器被控：多半是 admin，能读全部历史、签链 |

### 5.3 推荐 A

1. 局域网手机只有 A 能做到；而用户要的正是在家里用手机打开本机，看所有电脑。
2. A 正是「既是设备也是机器」：一台电脑一个身份，不多占设备数，没有第二对钥。
3. 权限更小：默认非 admin。
4. 常驻：副本常温，跨设备「需要你」随时算好。
5. Hub 不用为本机源放开 CORS、Turnstile、CSP。
6. 信任落在开源的一侧：持钥、开通道、执行策略的都是开源的本机程序，钥从不进入闭源 Hub 投递的 JS。

B 留给以后的 Tauri 桌面版。在那里，B 和 A 是同一台设备：有安全上下文，有钥匙串，可以开「可管理账号」（§7.3）。

### 5.4 非 admin 设备经通道能做什么

agent 在桥接（`S/agent/bridge.rs`）里按请求方是否 admin 选策略，并在 `agent_info.features` 里声明 `policy/1`。

| 能（每台设备） | 只有 admin 能 |
|---|---|
| 会话：看、发、打断、steer、撤回、回答提问、新建 | `/rh/machine-key`、`/rh/display-name`、重加密请求（同时要求 12 小时新鲜度） |
| 终端；文件读写；待办；定时任务；读设置 | `POST /api/server/self-upgrade` |
| 直连信令 `/rh/direct`；`GET /rh/info` | `/api/auth/*`（本机账号与会话） |
| — | 账号的增删与凭据写入（`/api/accounts*` 的非 GET 请求） |
| — | `/api/notify/config` 的写入；`/api/push/*` |
| 任何设备都不能 | `/api/cloud/*`、`/api/agg/*`（`cloud:*` 一律拒绝） |

- 不该给的请求回 403 `limited_device`。划线的原则：「用」这台电脑的事，每台设备都能做；改它的安全姿态（谁能登录、跑什么二进制、凭据），只有 admin 能做。
- **硬门槛**：旧 agent 会放行所有设备的全部 API（包括自升级），所以 v3 的读者版本要带 `policy/1`，并且门槛（§3.5）同时要求它。

### 5.5 钥匙放哪、被攻破时

**存放**

- 设备身份只有一个：`agent/machine-key.json`（0600，绑定实例 uuid，`SeedStore`）。
- 控制一侧的数据放在 `<实例>/aggregator/`（目录 0700，文件 0600）：
  - `keys.json`：其它 host 各轮的 `MK`；
  - `copies.db`：已验签、已解密的副本与流游标。
- 链状态与 agent 共用同一份已验证的链。

**威胁**

| 攻击者 | 能做 | 不能做 | 缓解 |
|---|---|---|---|
| 本机同一用户的进程（包括本机跑的 AI CLI）读到 `machine-key.json` | 在被移除前：冒充这台电脑（今天就如此）；**并且**控制账户里其它 host，读它们的离线副本 | 不是 admin 时：签链条目、加设备、开管理权、拿 `UK`、在其它 host 上自升级或改鉴权（拒绝清单）、被移除后继续 | 默认非 admin；拒绝清单；移除即断并轮换 `MK`；告警；S4 的「接受谁控制」；以后改放钥匙串 |
| 同上，但这台设备开了 admin | 加设备、开管理权、拿 `UK`、读全部历史：等于接管账户 | 移除前可以做任何事 | 钥在普通文件里的安装默认关 admin，开时警告（§3.3） |
| 偷到 `copies.db` | 读到其它设备最近的会话明文 | — | 与 `keys.json` 同盘同权限；如实说明；关闭聚合即删 |
| 读到这台电脑的备份或快照（Time Machine、APFS 本地快照、其它备份工具、换下的磁盘） | 读到备份那一刻 `copies.db`、`keys.json` 里的明文与钥，**包括之后被收窄、移除、关闭时删掉的** | — | `aggregator/` 打上 Time Machine 排除标记（`com_apple_backup_excludeItem`，与 `tmutil addexclusion` 相同）；删除前覆写（收窄时 `secure_delete` 加截断 WAL；关闭、断开、移除时整个目录先改名移走、再把每个文件写零后删除）只在文件层面尽力而为，碰不到快照、备份与固态盘的磨损均衡；其它备份工具要自己排除这个目录。以后：每个安装一把放在钥匙串里的静态钥，每台 host 一把数据钥，删钥即抹（加密擦除），不在 S1-5 |
| 无人值守 host 被攻破 | 同第一行：**能控制其它 host**。这是「一切都是设备」的代价，v2 里机器只能冒充自己 | 不是 admin 就不能改账户 | 建议它保持非 admin；S4 让每台 host 可以只接受 admin 设备的控制；每条通道在被连的 host 上记日志 |
| 局域网嗅探 `http` | 看到经网关发给手机的内容 | — | 今天本机内容已是如此；建议 Tailscale；以后 HTTPS 或 Tauri |
| 其它本机账号 | 看本机会话（今天就能） | 看或操作其它设备，除非所有者打开 | §4.6 |
| 远端设备经通道进本机 | 用本机（今天就能） | 借网关碰别的设备；改本机的云端链接 | `cloud:*` 拒绝 |
| 恶意 Hub | 扣住对一台设备的移除，控制类最多 7 天（开着的通道到期也被 agent 关掉）、管理类最多 12 小时；扣住副本的更新（副本停在旧处，状态里带「截至」） | 伪造条目、塞钥、读内容；伪造副本（只放行机器签名的流头与 manifest，manifest 先验签再取页，每页边取边验、不超过 `MAX_PAGE_LEN`）；**把副本退回旧版本**（拒绝：每份副本记着它见过的最新流头作下限，重读时也带着；在结果变得可见的地方检查——追赶结束（`stream_synced`）时与之后的每个流头——比下限旧（时间更早且序号不超过它）就拒收、本机留着原副本；退到已放行位置之前的 `GONE` 一律拒绝。追赶途中不查：诚实 Hub 的最新 checkpoint 通常比它最新的记录旧） | 按操作的新鲜度；Hub 与中继的吊销；副本的签名与下限 |

### 5.6 局域网手机经本机看

- 手机打开 `http://<局域网 IP>:8700`，用本机账号登录；账号有远端权限时，就能看到其它设备。
- 手机 ⇄ 本机：一条网关 WebSocket；本机 ⇄ 其它设备：端到端通道。
- 手机不持钥，不需要 WebCrypto，不占设备数，不需要配对。
- 网关单连接多路复用，可开 deflate（本机模式今天就开着；CRIME 式泄漏的风险在 Hub 和中继那一段，网关不经过它们）。
- **推送**：第一版不经本机转推其它设备的提醒，免得和 droppen.app 的推送重复；以后可以加开关。

### 5.7 用户怎么看见、怎么吊销

- **「我的设备」**（§3.7）：这台电脑一行，带「可运行 AI」徽章；「可管理账号」默认关；有移除操作。
- **告警**：加入时所有设备与 host 告警（§3.2）。
- **本机设置**：设备 id 与指纹、状态、能看几台、钥覆盖（例如「7/8 台有钥：m-x 缺第 3 轮，已索取」）、新鲜度。
- **被连的 host**：每条来自其它设备的通道、每个被拒绝清单挡掉的请求，都记日志。
- **吊销**：
  - 在 droppen.app 移除：立即断开，并轮换 `MK`；这台设备不是 admin 时不轮换 `UK`。
  - 只撤控制：本机关闭聚合（删钥与副本）。

## 6. 机器感知的应用核心

本节的一切都在开源仓库。droppen.app 只是托管同一份 SPA 的构建产物，`HubShell` 用到的 Hub 接口都在 `docs/hub-api/` 里。

**用词**：本节的「机器」指「可运行 AI 的设备」，代码名沿用 `Machine*`。界面上一律叫「设备」，也不叫「电脑」（§3.1）。

![数据怎么走：浏览器经一条网关 WS 到本机 droppen，本机用同一个身份经中继或直连控制其它设备、从 Hub 读加密流；下方是同一份 SPA 的三种外壳](local-aggregator/gateway.svg)

### 6.1 概念

```ts
// W/lib/machines/key.ts
type MachineKey = "local" | string;          // "local" 只在本机外壳里出现，指本机；其余是可运行 AI 的设备 id
interface SessionRef { machine: MachineKey; source: "claude" | "codex"; id: string }
const sessionKey = (r: SessionRef) => `${r.machine}|${r.source}/${r.id}`;
```

- 本机外壳里，本机自己的设备 id 规范化为 `local`。
- droppen.app 里没有 `local`。
- 删掉 `SessionSummary.server?`（今天没人读）。

```ts
// W/lib/machines/runtime.ts
interface MachineInfo {
  key: MachineKey; name: string;
  state: "online" | "offline" | "unknown"; since: number;
  path?: { kind: "direct" | "relay"; relay?: string; rttMs: number };
  copyAsOf?: number; agentVersion?: string; os?: string;
  canManage: boolean;            // 本设备对它的操作是否带管理权（拒绝清单）
  live: "live" | "copy" | "connecting" | "stale";
}
interface MachineRuntime {
  readonly key: MachineKey;
  info(): Store<MachineInfo>;
  transport: Transport;          // 今天的接口原样（W/lib/transport/types.ts:57-94）
  api: ApiClient;                // openapi-fetch，fetch 绑定到 transport
  feeds: FeedHub;                // 这台的 /api/events 多路复用
  copies?: MachineMirror;        // 远端才有
  ensureLive(): void;            // 按需开通道
  dispose(): void;
}
interface AppShell {
  kind: "local" | "gateway" | "hub";
  machines(): Store<MachineInfo[]>;
  runtime(key: MachineKey): MachineRuntime;
  home: MachineKey;              // 存布局、待办的那台
}
```

### 6.2 一份代码，三种外壳

| 外壳 | 何时 | 机器从哪来 | 每台的实时传输 | 每台的副本 | 钥在哪 |
|---|---|---|---|---|---|
| `LocalShell` | 本机模式；未登录，或该本机账号没有远端权限 | 只有 `local` | `directTransport`（今天的） | 无 | 无 |
| `GatewayShell` | 本机模式；已登录、聚合开、账号有远端权限 | 网关 `dir` | `local` 用 direct；其它用 `tunnelTransport(GatewayPipes(machine))` | `MachineMirror(GatewayCopySource)`，不落 IndexedDB | 本机 `droppen` |
| `HubShell` | droppen.app | `/v1/me`、在线态、状态流 | `tunnelTransport(MachineChannel)`（今天的） | `MachineMirror(HubCopySource)`，落 IndexedDB | 浏览器 |

三种外壳之上共用：`SwitchingTransport` 与 `ModeController`（每台各一）、`offlineTransport`、`offline-search`、三个 fold、FeedHub、会话列表、注意力、分屏、新建会话、设置、待办。

**两个接缝**

- **`PipeOpener`**（`W/lib/transport/tunnel.ts:47-84`）：`MachineChannel` 已实现它；新增 `GatewayPipes` 也实现它。
- **`CopySource`**（新抽出）：`MachineMirror` 今天直接拿 `{mk, client}`（`W/lib/hub/mirror.ts:497-512`），改为拿一个按逻辑名订阅的来源：

  ```ts
  type CopyName = { kind: "index" } | { kind: "status" } | { kind: "session"; source: string; id: string };
  interface CopySource {
    // 事件：reset | bodies[] + headSeq + headTimeMs | error
    follow(name: CopyName, fromSeq: number, on: (e: CopyEvent) => void): () => void;
  }
  ```

  - `HubCopySource` = 今天的流名推导 + `StreamClient` + 解密（在浏览器里）；
  - `GatewayCopySource` = 网关的 `sub` / `copy`（本机 `droppen` 已验签解密）。

### 6.3 本机网关 `/api/agg/ws`

**连接**

- 每页一条 WebSocket，用今天的 WS ticket 鉴权；调用者必须是有远端权限的本机账号；`cloud:*` 拒绝。
- 文本帧是 JSON（`t` 标记类型）；二进制帧是 `[管道号 u32][桥接消息]`，桥接消息与 E2E 通道里的相同（`W/lib/e2e/bridge.ts`、`C/rh-proto/src/bridge.rs`）。

| 方向 | 消息 |
|---|---|
| S → 浏览器 | `dir {machines: [MachineInfo…]}`，之后 `dir_delta` |
| 浏览器 → S | `open {p, machine, kind: rpc\|mux\|term\|blob, path, method?, headers?}` |
| S → 浏览器 | `opened {p}`，或 `refused {p, code: offline\|stale\|forbidden\|no_relay\|limited_device, message}`；之后是双向二进制帧；最后 `close {p, code?}` |
| 双向 | `credit {p, bytes}`：每管道每方向 512 KiB 窗口 |
| 浏览器 → S | `sub {s, machine, name: CopyName, from}`、`unsub {s}` |
| S → 浏览器 | `copy {s, reset?, bodies, headSeq, headTimeMs}`、`copy_error {s, code}` |

**S 一侧**

- 每台机器一条按需建立的端到端通道，多个浏览器共用；空闲 5 分钟关闭。
- 常驻跟随每台机器的索引流与状态流；会话流只跟随打开过的（最近 20 个）和每台最新的 5 个；都写进 `copies.db`。
- 优先级：控制 > 终端 > rpc 与 mux > blob。

### 6.4 统一会话列表与「需要你」

- **行从哪来**：机器在线时来自它的 FeedHub；离线时来自副本索引（带 `asOf`）。合并键是 `sessionKey`。
- **列表靠流就够快**：agent 每 3 秒比较一次列表和 transcript，所以列表不必常开通道；打开会话、终端、发消息时才调用 `ensureLive()`。
- **排序与分组**：默认跨设备混排；分组可选「按设备」「按项目」。顶部是设备筛选条，离线的写「离线副本 · 截至 14:02」。
- **需要你**：`groupAttention`（`W/features/sessions/session-attention.ts:147`）跨设备计算，「在问你」置顶，其次未读，再次工作中。标题栏与 app badge 的未读数跨设备求和。
- **设备仪表盘**：「需要你」台账与设备面板是同一组 selector 与组件（`NeedsYouLedger`、`MachinePanel`），droppen.app 与本机共用。状态流加 `os`、`agentVersion`（S1-5）。
- **规模**：可运行 AI 的设备数以套餐上限（`hosts.max`）为界；按 2,000 行验收。

### 6.5 分屏、新建会话、每台设备的设置、待办

**分屏**

- `PaneContent` 的 `session`、`chat`、`terminal`、`files` 加 `machine?`，缺省为 `home`，旧屏幕 JSON 照常可读。
- 每个叶子由 `MachineProvider` 提供自己的 runtime；同一屏里不止一台时，叶子标题栏显示设备名。

**布局与待办存哪**

- 本机外壳：存本机。
- droppen.app：存用户选定的「主设备」（默认第一台可运行 AI 的设备）。主设备是账户的，存在 Hub 上（`/v1/main-device`），每个浏览器、手机看到同一组屏幕；布局本身仍存在主设备上，等上云后再迁。

**新建会话**

- 加一个设备选择，默认顺序：当前叶子的设备 → 上次用的 → `home`。离线或 `stale` 的置灰并写原因。
- 模型、账号、目录都走所选设备的 `api`。新建终端同理。

**设置**分三类：

- **本设备**：主题、字号、语言、本浏览器的通知订阅。
- **每台可运行 AI 的设备**：通用、账号、定时任务、通知。顶部有设备选择；拒绝清单内的写入项写明「需要可管理账号的设备，或在那台电脑本机修改」。
- **只在某处有**：本机的「安全」与「Droppen」；droppen.app 的「我的设备」、中继、套餐。

**待办**：待办来自插件，不在本文；插件怎么存数据见 `docs/plugins/storage-protocol.md`。

### 6.6 要改的模块

**前端（`W/`）**

| 模块 | 今天 | 改成 |
|---|---|---|
| `W/lib/machines/*`（新） | — | `key.ts`、`registry.ts`、`runtime.ts`、`context.tsx`、`shells/{local,gateway,hub}.ts`、`query.ts`（`mq(machine, …)`） |
| `W/lib/transport/index.ts`、`W/api/client.ts` | 全局 `current`；单例 `api` | 每台 runtime 各有 `transport` 与 `api`；全局版本留作 `home` 的别名，标为 deprecated |
| `W/app/global-feed.tsx`、`feed-hub.ts` | 单例，行键 `source/id` | 每台一个 FeedHub 与行集；`UnifiedSessions` 合并 |
| `W/features/sessions/session-detail.tsx`、`session-feed-pool.ts`、`session-attention.ts`、`use-seen.ts`、`sessions-layout.tsx` | 单机 | 用 `sessionKey`；跨设备的注意力；统一列表 |
| `W/features/sessions/new-session-view.tsx`（原 `new-session-dialog.tsx`）、`use-create-conversation.ts`、`new-session-flow.ts` | 隐含本机 | 设备选择 |
| `W/features/terminals/*` | 全局传输 | 按 runtime |
| `W/app/pane-tree.ts`、`panes.tsx`、`pane-tree-view.tsx`、`secondary-pane.tsx`、`screens.tsx`、`screens-rail.tsx` | 叶子不含机器 | 叶子带 `machine?`；屏幕存 `home` |
| 全部 react-query 键（约 30 个） | 全局 | 改为 `mq(machine, …)`，并按机器失效 |
| `W/lib/transport/blob.ts`、`W/components/source-image.tsx`、`W/lib/fs-url.ts`、`W/components/host-file.tsx` | 键是路由串 | 键带上机器 |
| `W/features/sessions/feed-persist.ts`、`W/lib/local-cache.ts`、localStorage 的 `rh.draft.*` 等 | 不含机器 | 按机器存，附一次性迁移 |
| `W/app/router.tsx`、`public/sw.js`、`W/lib/push.ts` | 不含机器 | 加 `/m/$machine/…`；推送 URL 带机器 |
| `W/features/cloud/context.ts`、`cloud-app.tsx`、`cloud-bar.tsx` | 只有一台机器；`reload()` 换机器 | `HubShell`，所有机器同时在；「我的设备」与仪表盘 |
| `W/features/cloud/devices.tsx`、`machine-name.tsx` | 设备页与机器列表分开 | 合成「我的设备」（§3.7）；「可管理账号」开关；非 admin 下只读 |
| `W/lib/hub/account.ts` | 只按完整设备处理；配对总是给完整设备；索钥只回 `UK` | v3 条目、`SetAdmin`、`SetHost`；配对与注册带开关；给非 admin 设备投 `MK`；只回 `MK` 的索钥分支 |
| `W/lib/e2e/chain.ts` 等（TS 版链） | v2 | v3 的读、写、映射，与 Rust 互通 |
| `W/lib/hub/mirror.ts`、`streams.ts` | 吃 `{mk, client}` | 吃 `CopySource` |
| `W/features/settings/settings-view.tsx` | 单机 | 三类分组 + 设备选择 + 「Droppen」 |
| `W/features/link/*`（新） | — | `link-popup.tsx`、`link-settings.tsx`、`handoff.ts` |

**后端（`S/` 与 `C/`）**

| 模块 | 今天 | 改成 |
|---|---|---|
| `C/rh-proto/src/chain.rs`、`handshake.rs`、互通向量 | v2；只有一个新鲜度窗口 | v3 的条目、映射、签名规则、`host`/`admin` 检查；按操作的新鲜度 |
| `S/api/cloud.rs`（新） | — | §4.5 |
| `S/agent/{enroll,link,mod}.rs` | 独立进程注册；启动时起链接；只当响应方 | 注册改为库函数；运行中启停；对移除做出反应；`status`、`unenroll`；声明 `chain/3`、`policy/1` |
| `S/agent/bridge.rs` | 所有设备都能用全部 API | 非 admin 的拒绝清单 |
| `S/aggregator/*`（新） | — | 用同一身份做控制：`hub.rs`（设备会话、`/v1/client`、信箱）、`keys.rs`、`copies.rs`、`channels.rs`（票据、中继、Noise 发起、直连）、`gateway.rs`、`access.rs` |
| `C/rh-client`（新） | 设备端在 `rh-devkit` 里 | 抽出来，`rh-devkit` 与本机共用 |
| Hub（闭源） | 设备与机器分开；`devices.max` 只数完整设备 | 应用 v3；`devices.max`、`hosts.max`；`chain/3` 门槛；host 设备可同时以设备身份登录、领设备票据；用途 4 放开给非 admin；`retire`；拒绝已移除设备的 agent 登录（对外的部分见 `docs/hub-api/`） |
| `docs/hub-api/`（新，开源） | 只在 Hub 的注释里 | 完整的公开 API，附 JSON Schema |
| `C/rh-devkit/scripts/` 里的本地端到端脚本与 `cloud-dev.sh` | 从源码构建 Hub | 拉固定版本的 Hub 镜像 |

### 6.7 渐进迁移，不破坏单机本地模式

**统一验收**（每一步都要过）：

- 现有测试全部通过；
- 界面一致性扫描（内部工具，不在公开仓库）前后截图一致；
- 本机走一遍：新建会话、终端、分屏、屏幕持久化、离线缓存冷启动；
- 旧 URL 与旧屏幕 JSON 照常打开。

**步骤**

1. **M-0 取址**：`MachineKey`、`LocalShell`、`useMachine()`、`apiFor`、`mq()`。全局版本留作别名；加 lint，防止新代码用全局版本。
2. **M-1 行与存储**：用 `SessionRef` 贯穿各处；存储键按机器。
3. **M-2 路由与叶子**：加 `/m/$machine/…`、`PaneContent.machine?`；`home` 的旧路由不变。
4. **M-3 droppen.app**：`HubShell` 接管，所有机器同时在，不再 `reload()`。
5. **M-4 本机聚合**：`GatewayShell`，只在已聚合、且当前账号有远端权限时启用。

**M-2 计划**（S2-3 的 droppen.app 一半；本机 `GatewayShell` 一半与插件待办不在内）：

- **地址**：`/m/<设备>/` 下有 `sessions`（含 `$source/$id`）、`chat`（含三种会话）、`terminals`、`files`、`settings`、`stats`，与旧路由用同一组组件（`useParams`/`useSearch` 改为非 strict）。旧路由永远指 `home`；`/m/<home>/…` 换成旧地址（每页只有一个地址）；不在本页的设备显示「这台设备不在本页」，不报错。屏幕、应用、插件只在 `home`，没有 `/m/` 形式。
- **叶子**：`session`、`chat`、`terminal`、`files` 加 `machine?`。只在不等于 `home` 时写进 JSON，所以本机与只用一台设备的屏幕 JSON 一字不变；缺省就是 `home`，也就是存这块屏幕的设备。每个叶子包一层 `MachineView`（`MachineProvider` + 那台的列表 feed + 占用），选择器、会话、终端、文件都走叶子自己的设备。一棵树里不止一台设备、或叶子不是 `home` 时，叶子标题栏显示设备名。叶子的设备不在本页时（本机网页读到在 droppen.app 上排的屏幕、设备离开了账户），这个叶子只显示「这台设备不在本页」和切换、关闭，不碰那台设备；选择器指着不在本页的设备时，退回路由所在的设备。每个叶子外面有一层错误边界，一个叶子画不出来不会拖垮整页；路由的主视图例外，它的边界只包住 `Outlet`，按地址重置（`RoutedViewBoundary`），换一个地址就重画，叠在上面的屏幕和应用不受影响。
- **列表 feed 按设备计数**：`GlobalFeedProvider` 改为取 `machineFeed(machine)`（`perMachine`），第一个使用者 `start()`，最后一个离开才 `stop()`；store 跟着设备留着，重新挂上时行立刻在。窗格里有 B 的会话，就一定有 B 的列表在跑，满足 S3-1 遗留的「列表停过之后 feed 只排队」。
- **占用**：`MachineRuntime.hold()` 返回释放函数，`useEnsureLive` 改为占用。窗格内容（选择器除外）、`/m/<设备>` 路由、新建会话对话框里选中的设备都占用它。第一步里没有 `hold` 的驱动退回 `ensureLive()`，行为与今天相同；第二步 `HubMachine` 计数：被占用时不放通道，最后一个占用离开、又不在看，才开始 5 分钟计时；被占用的设备在副本里跟随本页打开过的会话（`setFollowSessions(在看 || 被占用)`），照旧受 `StreamBudget`（192）约束。屏幕的叶子是常驻的（访问过就一直挂着，直到删掉屏幕），所以它们的设备一直算占用——与它们的终端 socket 本来就占着通道一致。
- **`home` 的含义**：改为「主设备」，只管存储与默认——布局、屏幕、插件文档、旧地址。栏上切换设备改为导航到 `/m/<设备>/<同一栏>`（`home` 用旧地址），导航栏、屏幕、窗格都不动、不重建。`RootLayout` 仍以 `home` 为 key：只有主设备真的换了（用户改选，或它离开账户）才重建，那时旧地址下的每个视图、每块屏幕确实都是另一台的了。初始主设备取这个浏览器上次在看的那台（`rh.hub.machine.<账户>`），升级后旧地址与已有屏幕仍在原来的设备上打开；都没有时取账户链上第一台可运行 AI 的设备；切换器里加「设为主设备」（先确认）。`HubShell.focus` 改为「路由所在的设备」（从路由同步），「在看」= 路由设备 ∪ 被占用的设备。
- **导航栏**：会话、即时聊天、终端、文件、设置、用量跟着路由所在的设备走；屏幕、应用、插件、新建菜单仍是 `home` 的；健康点读路由设备的 feed。本机只有 `local`（就是 `home`），所以一切不变，界面一致性扫描的截图前后一致。
- **新建会话**：两台以上设备时对话框顶部有设备选择，默认顺序：当前叶子或路由的设备 → 上次用的（`rh.hub.new-session-device`）→ `home`；离线、`stale` 的置灰并写原因。选中即占用（拨号），模型、账号、目录、最近目录都走所选设备的 `api`；建好后在那台打开（`/m/<设备>/sessions/…`，或叶子带上 `machine`）。窗格里的新终端、文件同理走叶子的设备。
- **设置**：`/m/<设备>/settings`；两台以上设备时设置页标题行有设备选择。字号与语言只由主设备的偏好决定（`PrefsSync`），所以只在主设备的设置里出现；其它设备的设置里换成一行说明和去主设备设置的链接。
- **不串号**：查询、feed、草稿、发件箱、回执检查、排队提示、终端、上传本来就按 `useMachine()` 与 `sessionKey` 取址，换成叶子或路由的 provider 即可；新键都在 `rh.hub.` 下；`check:machines` 保持通过。
- **服务端也要认 `machine`**：屏幕的清洗（`server-rs/src/screens`）按已知字段重建每个叶子，读和写都过它，原来会把 `machine` 丢掉。现在它在 `session`、`chat`、`terminal`、`files`、`picker` 上保留一个能用的 `machine`（非空、不超过 200 字、无控制字符，与 id 同样只存不解），退化成选择器的内容不带。droppen.app 上屏幕存在主设备的 agent 里，所以主设备的 agent 要升级到这一版，跨设备的屏幕才存得住；旧 agent 存回来少了设备时，网页在那块屏幕上提示（`machinesDropped`），窗格回到主设备。
- **回滚**：旧版本（网页或 agent）读到带 `machine` 的叶子会在 `home` 上找那个 id：会话「找不到」、终端「已退出」、文件落在 `home` 的同一路径，都不崩；旧 agent 读时就丢掉 `machine`，下一次保存把它写没。`/m/…` 地址在旧版本里是未知路由。除了可选的 `machine`，存储格式不变。
- **拆成两步**：第一步是路由、叶子与屏幕 JSON（含服务端清洗）、`MachineView`、按设备计数的列表 feed、`hold()` 接口、导航栏、新建会话的设备选择、设置的设备选择、一致性预览的 `uiDevices=1`、测试；第二步是 `hub-shell.ts`（占用计数、focus 从路由来、主设备固定与「设为主设备」、`select` 改为导航）、`cloud-bar.tsx`、`cloud-app.tsx`、仪表盘 `openAt` 改用 `/m/` 地址、`sw.js` 的离线壳路由加 `m/`、`ui-wording` 测试收进新文案表、浏览器验收与云端 e2e。

**M-0、M-1 落地时的取舍**（S1-6）：

- 代码在 `W/lib/machines/`：`key.ts`、`registry.ts`（`MachineRuntime` 只有 `transport` 与 `api`；FeedHub、会话 feed 池等按机器的单例用 `perMachine` 造，`feedHubFor(machine)`）、`context.tsx`、`query.ts`、`storage.ts`、`shells/{local,hub}.ts`。`info()`、`copies`、`ensureLive()` 留给 S3-1 与 S1-7。
- 存储键：`local` 保留今天的键（`rh.draft.<source>.<id>` 等、IndexedDB 的 `sessions`、`feed:<source>/<id>`），所以本机不用迁移、回滚到旧版本也照常读；其它机器是 `rh.m/<机器>/…`（IndexedDB 是 `m/<机器>/…`）。droppen.app 上没有 `local`，今天的键是当时打开的那台机器写的：每个键由第一个来读它的机器接管（移走一次），会话的草稿由那个会话的机器接管。
- lint 是 `scripts/check-machine-keys.mjs`（`pnpm check:machines`，CI 里跑）：仓库没有 ESLint，规则是按名字的正则检查。
- `SessionSummary.server?` 没删：它属于服务端的 API（Rust 与 openapi），不在这次纯前端的改造里。
- `features/cloud/context.ts` 仍描述 droppen.app 打开的那一台机器（即 `home`），由 S3-1 的 `HubShell` 取代。

**M-3 落地时的取舍**（S3-1）：

- `HubShell` 在 `W/features/cloud/hub-shell.ts`（它由云端模块拼成，`lib/` 不反向依赖 `features/`）；`W/lib/machines/shells/hub.ts` 只剩登记那一半（`enterHubShell`、`openHubMachine`）。`CloudContext` 删了：账户、Hub 链接、中继和每台机器的通道、副本都从 `hubShell()` 读。
- 账户链上每台在用的 host 一出现就开一份 runtime（`HubMachine`：副本、通道、`SwitchingTransport`、`ModeController`、`MachineInfo` store），链上去掉就 `closeMachine`（最后一台是 home 时也关：它的 runtime 留作 retired，给还没卸下的视图用，`isOpen` 为假）。不在看的机器只显示副本，不拨通道：`ModeController` 多了 `wanted`，只有 `ensureLive()`（切到它、打开会话、开终端）之后才拨；切走 5 分钟（`IDLE_MS`，与 §6.3 网关一致）后放掉通道，立刻回到副本，再要它时马上重拨；通道上还有没走完的管道（下载、上传）就先不放，过一会再看；没人要却开着的通道（迟到的请求拨起来的）同样到点放掉。关掉的机器，`SwitchingTransport` 拒绝之后的请求，不会被迟到的重试重新拨通。
- `home` 就是眼前那台机器，切换时移动 home（`setHome`）；`RootLayout` 以 home 为 key，整棵树在新机器上重建，不 `reload()`，其它机器的通道、副本、查询缓存原样留着。没有 M-2 时屏幕叶子不带机器，所以布局与待办仍跟着眼前那台（与旧行为一致）；§6.5 的固定「主设备」留给 S2-3。切到另一台时先离开旧机器的页面，回到同一栏（`switchPath`：会话、终端、文件……）。
- 按机器失效：每台的 `ModeController.onSwitch` 只 `invalidateMachine` 自己。
- `CopySource` 在 `W/lib/hub/copy-source.ts`：`MachineMirror` 与 `MachineStatusReader` 按名字（索引、状态、会话）订阅，`HubCopySource` 负责按密钥纪元推导流名、跟随更新的纪元、用机器签名校验存下的位置。账户的密钥晚到时（刚注册的机器）副本那时才开始，不用刷新。副本在 IndexedDB 里的键照旧是 `stream/<账户>/<机器>/<流>`（本来就按机器分），没有改成 `m/<机器>/…`：改了会让每台设备上的副本作废，回滚也读不到；`m/<机器>/…` 仍是会话列表和 feed 窗口的键（S1-6）。
- `MachineRuntime` 有了 `info()` 与 `ensureLive()`；`copies` 与 `feeds` 没挂在 runtime 上（`lib/machines` 不依赖 `lib/hub` 与 `app/`），分别是 `hubShell().machine(key).mirror` 与 `feedHubFor(key)`。
- 订阅有上限：Hub 每条连接最多 256 个流订阅（`docs/hub-api/websocket.md` 的 `stream_refused`），超过的被拒，被拒的是谁看先后。不在看的机器只跟随索引与状态；在看的那台另加最暖的 5 个会话和本页打开过的最近 8 个；整页一个 `StreamBudget`（192），超了只拒会话流，索引与状态永远给，并且按实际订阅的名字数计（多个密钥纪元也只算真正订着的那几个）。被拒的会话不等待，直接用设备上已有的；有了空位再要一次。
- 设备上的读：会话保存时的 `getAllKeys` 只读这台机器的键（IndexedDB 键区间），扫除只读 `stream/` 与 `streammeta/`；扫除每个 shell 启动时一次，不是每份副本一次。留哪些副本由索引（最暖的会话）与打开列表决定，不看内存里还有没有那个槽：切走时内存放掉了暖会话的槽，紧接着落地的一次保存也不会把它们的副本扫掉。副本关掉后，计时器排上的保存不再写入（`flush` 要求的照写）。
- 机器列表、「我的设备」、套餐里读机器名，shell 已有那台的副本就读副本的状态，不再另开 `StatusReader`。
- 每台的 feed socket：树卸下时 React 先清理父（`GlobalFeedProvider` 停掉列表），再清理子（会话的 feed 池），会话 feed 发现频道断了、还有人在看，就会重连，把已停的 socket 又拉起来、再没人关。现在 `FeedHub` 记着列表是否停过：停过之后 feed 要频道只排队，等列表再 `start()`（所以 M-2 把别的机器的会话放进窗格时，必须启动那台的列表，或由 `ensureLive()` 启动，否则那些 feed 会一直等）；一次连接里没人听时攒下的列表帧超过上限，`start()` 就换一条新连接拿新快照，不回放过期的。
- 插件激活按（机器，插件）缓存：home 换了，插件的存储跟着换到新机器；机器关掉时它的激活一起丢掉。下载进度条标出不在看的机器的下载。
- 遗留问题：`GlobalFeedProvider` 的 store 按机器建；`sessionFeedsFor` 有了 `dispose`，`closeMachine` 会关掉那台的会话 feed；别的机器的服务端重启不再作废页面的下载票据，别的机器的 socket 失败也不再去查页面登录（`holdsPageLogin`）。下载票据本身仍是页面登录那台的：droppen.app 上每台都走通道、不用票据，要到 `GatewayShell`（本机直连 + 远端）才需要按机器分。

**S3-2 落地时的取舍**（设备仪表盘与跨设备「需要你」）：

- 仪表盘就是机器页（当时是 `devices-dashboard.tsx`，`MachinesScreen` 是它的别名；后来由 `W/features/cloud/my-devices.tsx` 取代，见下）：「我的设备 · 可运行 AI」。≥1680 三栏（左栏摘要、筛选、添加电脑；中间面板网格与「需要你」台账；右栏选中设备的会话与设备信息），1280–1679 摘要变成顶部横条、右栏仍在，更窄时只剩横条与面板（窄于 1280 时点面板直接打开设备）。面板网格 `auto-fill, minmax(340px, 1fr)`，1440 / 1920 / 2560 下是 2 / 3 / 4 列。
- 与本机共用的部分在 `W/features/machines/`：选择器 `needs-you.ts`（`needsYou`、`unreadAcross`、`ledgerKind`，建在 `session-attention.ts` 的 `groupAttention` / `unreadCountOf` 之上）、每台的会话列表 `device-sessions.ts`（`useDeviceSessions`：live 的那台来自它的 feed，其余来自副本索引）与 `copy-sessions.ts`、组件 `NeedsYouLedger`、`MachinePanel`、`DeviceDetail`、`UnreadSignals`。本机只有 `local`，这次只用到选择器：标题栏与 app badge 的数字走 `unreadAcross([local])`，与以前的 `totalUnread` 相同，截图前后一致；台账与面板等 S1-7 的 `GatewayShell` 有了多台设备再挂到本机页上。
- 不拨号：仪表盘的一切来自 `info()`、状态副本与索引副本。`HubMachine.sessions`（`copySessions(mirror)`）把索引变成一个 store，索引变了才换。索引行就是完整的 `SessionSummary`（`asking`、`activity`、`resident`、`unreadCount`、`attentionAt`、`scope` 都在），所以台账不需要任何会话流，`StreamBudget` 不变：浏览器实测 4 台在线 = 8 个订阅（各自的索引与状态），四条通道都是 `idle`；后台那台的「在问你」1.9 秒出现在台账顶部（agent 每 3 秒列一次）。不在线的设备照应用显示副本的样子「静止」（`restingRow`）：它的会话不算工作中、不算在问你，未读照算——停在问题上掉线的机器不会一直顶在台账上；实测停掉那台，它的问题立刻离开台账，重新上线 0.6 秒后回来。
- 索引缺的只有一样：问题本身的文字。右栏与台账只能显示会话标题；最小的改动是索引行加一个截断的 `askingText`（问题的前 160 字，随 `asking` 一起清空），不必订阅会话流。`os`、`agentVersion` 取自状态流里已有的 `settings.platform`、`settings.serverVersion`（`darwin` 显示为 macOS），没有版本号与架构；S1-5 给状态流加字段后再细化，没有时那一行不显示。
- 打开：台账的一行、面板的最近会话、右栏的「去回答」都调用 `HubShell.openAt(machine, path)`。从仪表盘（应用不在页上）先写地址（`/sessions/<source>/<id>`、`/terminals`）再 `select`；应用在页上时先 `select` 再写地址，路由立刻跟着地址走，走到的必须是新机器的页面。两种都是把 home 切到那台（M-2 之前窗格不带机器，所以不在原地打开别的机器的会话，§6.7 的 M-2 注意事项不触发）；会话页的 `ensureLive()` 照旧，只有那台被拨号。
- 标题栏与 app badge：`UnreadSignals` 在应用里由 `RootLayout` 挂（home 用 feed，其余用副本），在仪表盘上由仪表盘挂（全部用副本）。home 的 feed 还没有第一份快照时（刚切到它，`RootLayout` 以 home 为 key 重建）照旧用它的副本；仪表盘与应用交接时（前一个卸下、后一个等路由加载完才挂上）旧数字保留 1.5 秒，没有下一个来接才清掉——实测仪表盘 → 应用、机器 → 机器、应用 → 仪表盘，标题逐帧都没有掉过。app badge 从「有事发生」的标记改为页面打开时的未读数（与标题一样封顶 99），回到前台时重新写入；推送到达而页面没开时仍由 service worker 打标记。
- 只有在线的设备算进标题与 badge（`unreadAcross`）：离线、未知、副本过期的设备（`away`）的未读照旧留在它的面板与「需要你」台账里，但不进标题——在这页上读不掉它，一台合上放进抽屉的笔记本不该让 badge 永远挂着 `(N)`。所以设备离线时，左栏的「未读」可能大于标题的数字。
- 副本过期：设备说自己在线，但它的索引副本超过 15 分钟没动（agent 每 5 分钟给每条流签一次心跳头，`HEARTBEAT_EVERY`；三次没到），就和离线一样「静止」显示，面板写「副本 · 截至 …」——流写入卡住、被拒、本设备缺某个密钥纪元时，旧的问题不会一直顶在台账上。
- 已知的出入：副本索引只含最近 30 天、自己的会话（`RH_AGENT_SYNC_DAYS`，`agent/streams/content.rs` 的 `admits`），feed 没有这个窗口，所以 home 在 feed 与副本之间切换时，总数可能因为 30 天前的未读而变一下；读掉一条后，别的设备上的数字要等那台 agent 下一次列表（每 3 秒，`LIST_EVERY`）再经 Hub 才更新。两者都接受，不改。
- 12 台设备的预览：界面一致性预览的 `?uiCloud=machines-many`（另有 `machines`、`machines-empty`）；扫描默认视图加了 1920×1080 与 2560×1440。
- 没做：设备页右栏的密钥状态与设备链时间线；768–1279 用面板网格代替设计稿里的机器台账表；「可更新」标记；按最近活动排序（面板保持链上的顺序，不会随会话跳动）。

**「我的设备」落地时的取舍**（§3.7，设备页与机器页合成一页）：

- 一页，地址 `/devices`（`W/features/cloud/devices-route.ts`、`my-devices.tsx`）。没有设备在看（`HubShell.focus` 为 `null`）时就是它；顶栏只剩一个入口「设备」，图标用网格（`LayoutGrid`）：默认打开的是可运行 AI 的设备面板网格，钥匙图标留给真正讲钥匙的提示（旧钥匙、恢复词），设备行里的类型图标（浏览器、手机、Droppen）不拿来当导航。
- 旧的两个入口都没有过自己的地址：机器页是「没有机器在看」时停在应用上次的地址，设备页是盖在应用上的一层。它们的去处都改成这一页：顶栏「设备」、无副本遮罩的按钮、链上去掉了正在看的那台、「换中继…」（打开「全部」并滚到中继一节）。`HubShell.select(null)` 把地址换成 `/devices` 并记下应用的地址；从这页打开一台设备时回到那个地址（换了一台就只保留同一栏，`switchPath`），所以没有记住设备时打开的深链接（`/sessions/…`，先落在这页）选了设备后照常打开；记住了设备时照旧直接进应用。`/devices` 加进了 service worker 的离线壳路由；不加 `/machines` 之类从没存在过的别名。
- 筛选：「全部」是原设备页（`DevicesPanel`，现在只画正文，页框、提醒条、标题数字由 `MyDevicesFrame` 画一次，切换筛选时不重来）；「可运行 AI」是 S3-2 的仪表盘（`DevicesDashboard`，同样只画正文）。没记住选择时，账户有可运行 AI 的设备就默认「可运行 AI」，否则「全部」；选择按浏览器、按账户记在 `localStorage`，地址带 `?view=all|ai`，链接能直接打开某个筛选。
- 「全部」宽于 1280px 时按 §3.7 的列对齐成表，有列头：设备（类型图标、名称、「可运行 AI」徽章，小字 id 与指纹）· 状态 · 最近在线 · 操作（原有的「可管理账号」一列已去掉，见 §3.1 里 D1 之后的说明）；更窄时同一行照旧换行。开关规则、改名、移除（「同时轮换内容密钥」）都是原来的组件。添加只有两个入口：「添加浏览器或手机」与「添加可运行 AI 的设备」；仪表盘上的添加按钮打开的是同一个选择（先在那台设备的本机设置里登录，其次一次性注册码），不再直接发码。
- 账户层面的提示画在页框里、两个筛选上方都有（`AccountNotices`）：这台设备不能管理账号、账户还在链 v2 或 hub 还不收 v3、钥匙该轮换了、本设备缺最新的用户钥、有设备该重新登记（「可运行 AI」上给出「显示全部设备」）。钥匙该轮换时，那一屏的墨色主按钮是轮换，添加按钮退为次要。
- 在「我的设备」上按浏览器的后退：地址变了但页面没变（应用的路由没挂着），所以 shell 听 `popstate`，回到上次看的那台、停在后退到的地址；还没看过任何一台时，留在 `/devices`。再前进回 `/devices` 时（有设备在看），回到「我的设备」，而不是应用路由的 Not Found；从这里再打开那台，回到后退时应用所在的地址。地址在这页下面被别的方式改过时，打开另一台设备只保留同一栏；从仪表盘直接打开某个会话（`openAt`）的地址照旧。
- 通道、中继和注册的错误：`MachineChannel` 抛出的、记在中继上的失败原因、`AccountError` 的 proof 与 notInChain 等消息改成「device」的英文，并且就是 i18n key，`errorText` / `knownText` 按读者的语言显示（带名字的 `AccountError` 用 `{{id}}` 插值）；中继或 hub 拒绝通道时给的理由（服务器的原话）放进句子里显示（`ChannelRefused`：「中继拒绝了通道：{{why}}」）；中继失败里其他原样的技术原因照旧显示。
- 不拨号：「全部」读 `info()`、副本（状态里的名字，`useMachineStatuses` 走 shell 已有的副本）、链与 `/v1/me`；中继一节的「到那台设备」用应用上次看的那台（`HubShell.lastShown`，离开 5 分钟内它的通道还在）。`StreamBudget` 不变。页面右上角有「← 设备名」回到上次看的那台。
- 用词：见 §3.1。英文 key 本身就是英文文案，所以改的是 key（连同三张表和 `t()` 调用），没用到的旧 key 删掉了；代码名（`Machine*`、`mq`、`{{machine}}` 插值名）不动。日文的「端末」与「コンピューター」统一成「デバイス」。
- 没做，留着：
  - 本机设置「Droppen」一节里的只读「我的设备」：本机网页没有账户链，要本机 API 先给出设备列表（Rust），留给 S1-7。
  - 「最近」一列里 admin 设备的「最近一次作证」：`/v1/me` 不给每台设备的作证时间，要 hub 先加字段。
  - 「添加可运行 AI 的设备」的扫码备选：只有本机设置登录与命令行注册码两条路；要可运行 AI 的设备先能出示二维码。
  - 浏览器与手机的状态（在线、离线、未知）：hub 只对可运行 AI 的设备发在线态；它们那一格空着，「最近在线」照常显示。

**「我的设备」改成一页时的取舍**（去掉「全部 · 可运行 AI」筛选，`feat/devices-page-redesign`）：

- 一页：页头是标题与唯一的主操作「添加设备」（先选「浏览器或手机」或「可运行 AI 的设备」，再进原来的两个流程；账户还没有可运行 AI 的设备时直接进后者），下面是用量（`UsageSummary`，`plan.tsx`）、「这台设备」（`ThisDeviceSection`）、设备列表（`device-row.tsx`）、有内容时的「需要你」台账、钥匙与恢复词，最后是折叠的已移除设备。`?view=` 的旧链接照样打开这一页，地址里的 `view` 被去掉；「换中继…」打开这一页并展开「这台设备」里的中继。`devices-dashboard.tsx` 删了；它的面板、摘要六个数、状态筛选、按名称或系统筛选和右栏选中设备没有搬过来——面板上的信息（系统、agent 版本、路径、最近会话、打开、终端、改名、旧钥匙 / 链告警 / 副本保留告警）都在设备行或它的详情与菜单里。
- 名字：浏览器与手机按 hub 归纳的 `ua_label` 显示，页面只认词表里的词（`lib/hub/ua-label.ts` 的 `uaLabelOf`），hub 送来别的就当没有；本设备在 hub 说之前用本页自己的 User-Agent 按同样的规则归纳。名字只用来认设备，确认框里照旧写 id，核对靠指纹。手动改名（存在账户加密的地方、hub 读不到）留作后续。
- 顶栏右上角显示账户的邮箱（`/v1/me` 的 `email.address`），没读到之前显示账户 id，id 留作悬停提示。
- 「不直连」改成正面说法「允许直连（更快）」，是方形开关；存的仍是原来的「hub only」开关（取反），立即生效。

**M-2 落地时的取舍**（S2-3 的 droppen.app 一半，两个 PR：路由与叶子、`HubShell` 一侧）：

- 计划（上面的「M-2 计划」）照做。与计划不同或计划没写到的：
  - **服务端也改了**：屏幕的清洗在读写时都按已知字段重建叶子，原来会丢掉 `machine`。`server-rs/src/screens` 现在保留它。所以 droppen.app 上主设备的 agent 要到这一版，跨设备屏幕才存得住；旧 agent 存回来少了设备时，网页在那块屏幕上提示。
  - **主设备**：存在浏览器里，按账户（`rh.hub.home.<账户>`）。只有「设为主设备」（切换器底部）或它离开账户才会换，换了才重建 `RootLayout`。换的时候眼前的页面不变：旧主设备的普通地址改写成 `/m/<旧>/…`，新主设备的 `/m/<新>/…` 变成普通地址；屏幕、应用、插件页（只有主设备有）回到会话。
  - **在看的设备就是地址所在的设备**：栏（`CloudBar`）显示它，并用 `HubShell.follow` 让 focus 跟上路由。切换器用路由导航到那台的同一栏（是一条历史记录，后退能回来），不再移动 home。`select` 只在「我的设备」与应用之间换地址，从「我的设备」打开某台时：应用的地址就是那台的，就回到那个地址，否则去那台的同一栏（`addressOn`）。`openAt` 用那台自己的地址。
  - **主设备是这个浏览器记住的**：没有记住的浏览器，默认是上次在看的那台（升级时），否则是账户链上第一台可运行 AI 的设备——每个浏览器都一样，不看先打开哪台。「设为主设备」之前要确认（「屏幕布局存在主设备上：换成 B 后，A 上的屏幕会暂时看不到，换回 A 即可恢复」）。主设备离开账户时，换成正在看的那台，没有就换成链上剩下的第一台。换了主设备以后，历史里更早的普通地址（后退、前进到的）都按新的主设备打开。
  - **以后**：主设备按账户同步（存进账户，而不是浏览器），让每个浏览器看到同一组屏幕。这次没做，所以两个浏览器可能各选各的主设备。（已做，见下面「主设备按账户落地时的取舍」。）
  - **路由重新挂上时的陈旧状态**：「我的设备」显示时应用的路由卸下，shell 在它下面改地址；路由再挂上时，在 `load()` 之前会先按上次的地址渲染一帧。那一帧如果是另一台设备的会话，会把那台拨起来。所以 `RootLayout` 在路由跟上地址栏之前什么都不渲染（`useRouterCaughtUp`，只在挂上时检查一次）。比较用路由 `href` 里浏览器写法（百分号编码）的路径，不用解码后的 `pathname`，否则带转义字符的地址永远对不上、整页空白。`follow` 不拿 `location` 判断：TanStack 的历史写入是排队的，地址栏会比路由慢一点。
  - **占用**：`HubMachine.hold()` 计数。被占用时 live、跟随会话，`release()` 不放。最后一个占用离开、又不在看时，才开始 5 分钟计时。窗格、会话、终端、`/m/` 页面、新建会话对话框（打开时，以及等新会话出现期间）都占用。
- **浏览器验收**（`cloud-dev.sh`，3 个一次性 agent，界面里重命名为 device-A/B/C；验证时 `idleMs` 调成 15 秒）：
  - 一块屏幕放 A 的会话、B 的终端、C 的会话，然后停掉 C：C 的窗格变成只读副本（红方块，输入框写「设备离线 — 这是只读副本」）。
  - A 的 agent 里存的布局带 `machine`；刷新后三个窗格原样恢复，B 的终端重新接上。
  - 等过 idle 时间 40 秒，被窗格占用的 B 一直 `open`。
  - 没人要的设备一直 `idle`：列表从副本读，仪表盘和「需要你」也不拨号；从「我的设备」打开 B，不会把上次看的 A 拨起来。
  - 栏切换、「设为主设备」、「我的设备」⇄ `/m/B/…` ⇄ 普通地址的前进后退，都不刷新页面（页面上的标记一直在）。
  - 两台设备上同 id 的会话各显示各的内容：只追加到 B 的金丝雀，在 A 上看不到。
  - 新建会话选 B：请求到了 B（只有 B 的 home 里有的目录被接受），B 去启动 CLI；验收里的 agent 故意没有 claude 在 PATH 上，以免用到本机钥匙串里的凭据，所以停在启动那一步。
  - 本地端到端通过。
- **没做**：本机 `GatewayShell` 一侧（S1-7、S2-2 之后）、插件待办跨设备、推送 URL 带设备（droppen.app 没有推送）、`/m/` 下的应用与插件页。

**主设备按账户落地时的取舍**（M-2 的后续；`docs/hub-api/http.md` `/v1/main-device`）：

- **存在哪**：Hub 按账户存一个值（主设备与设置时间）。账户里任何一台在链上的设备都能设（`PUT /v1/main-device`），值只能是链上在用的 host；已移除、不再运行 AI、别的账户的、或不运行 AI 的设备一律 `409 not_a_host`。检查与写入都在账户的链锁里。读的时候再按链过滤一次：存着的值不是在用的 host 就当没有，所以崩溃、或旧版 Hub 追加的移除都不会让人读到过期的值。
- **离开账户**：host 被移除或 `SetHost false` 后，Hub 把它清掉并推 `main_device {machine: null}`；Hub 不替人挑下一台。网页按原来的规则挑（正在看的那台，否则链上第一台可运行 AI 的设备），用 `if_unset` 认领：几个页面同时认领时第一个算数，其余的拿到回答就跟着它。
- **推送**：`/v1/client` 的 `main_device {machine, set_ms}`，登录后（`welcome` 之后）一次，之后每次变化一次。快照在连接登记之后才读，所以中间的变化可能先到；网页按 `set_ms` 只收更新的。没有新的轮询，也不需要回到前台刷新：每次重连都有快照。
- **网页**：开始时先用这个浏览器的副本（`rh.hub.home.<账户>`，离线或 Hub 还没回答时用），Hub 说了就跟账户的（只认自己验过的链上有的 host；链上还没有的先等链）。真换了才重建 `RootLayout`，眼前的页面不变（与本机「设为主设备」相同的改写）。「设为主设备」先在本页生效，再写给 Hub；写不出去（离线）就留在本浏览器，链接重新连上时再写；写的过程中到达的旧快照不采用。确认文案改为「这一设置对你的所有设备生效」。
- **升级**：Hub 还没有值时，第一个加载的浏览器用它的主设备认领（`if_unset`）。只是本页推出来的默认（这个浏览器什么都没记住、也没选过）不认领，免得一台新手机替已经选过的浏览器做决定。
- **回滚**：旧版网页不读不写这个值，照旧按浏览器记；旧版 Hub 没有这个值，路由是 404、也不发 `main_device`，新版网页就退回按浏览器记。再升级回来时 Hub 上的值还在（离开账户的会在读的时候被过滤掉）。

## 7. 命名与打包

### 7.1 名字

| 程序 | 今天 | 以后 |
|---|---|---|
| 本机 | `remote-hand-server` | `droppen` |
| Hub | `rh-hub` | `droppen-hub` |
| 中继 | `droppen-relay` | 不变 |

crate 名不改。本机包名暂不改，只把 `[[bin]]` 改名为 `droppen`。

### 7.2 兼容计划

原则：**对外的名字改成 Droppen，内部已经落盘的东西一个不搬。** 任何自动改写 plist 或自动搬目录的方案都不做。

| 项 | 做法 |
|---|---|
| 发版资产 | 新增 `droppen-<版本>-<os>-<架构>.tar.gz`（macOS 三个、Linux 两个），内含 `droppen`、`remote-hand-server`（同一个文件的硬链接）、`INSTALL.txt`。旧资产继续发至少 3 个月（到 2027-01-31），`SHA256SUMS` 两份都列（S5-1 已做，见 §7.4） |
| 两个名字 | 同一个二进制，行为不看 `argv[0]`；只有说法看：用 `droppen` 运行时帮助、用法和错误里建议的命令都写 `droppen`，帮助注明旧名仍可用；用旧名运行时一字不改 |
| `<数据>/bin` 下的托管副本 | **不改名**：仍是 `bin/remote-hand-server` 与 `.prev`、`.bad`、`.staging-*` 阶梯，旁边加一个 `bin/droppen` 相对符号链接（安装脚本加）。长驻的 `up` 监督进程只认旧路径 |
| 自升级 | 推送式上传，与文件名无关，不改 |
| 数据与配置目录 | 已有安装永远用旧目录。解析顺序：`DROPPEN_DATA_DIR` > `REMOTE_HAND_DATA_DIR` > 旧目录存在就用旧目录 > `~/.local/share/droppen`。从不自动搬 |
| 环境变量 | `RH_*`、`APP_*` 不改；`REMOTE_HAND_*` 加 `DROPPEN_*` 别名 |
| launchd | 已有的标签与 plist 不动；以后 `droppen service install` 只给新安装写 `app.droppen.<实例>.plist` |
| 协议标识 | 不改：`rh/v1 …` 标签、`x-rh-*` 头、`"agent":"remote-hand"`、`rh-*` 存储名。三种码读的时候已经同时认 `DR1-`/`DRP1-`/`DRR1-`（和 `RH1-`/`RHP1-`/`RHR1-` 是同一个版本，大小写规则照旧），写出来的仍是 `RH1-`/`RHP1-`/`RHR1-`；什么时候改写新前缀见下面的切换顺序 |
| 「添加能运行 AI 的电脑」的命令 | `droppen agent enroll …`，作为备用路径 |
| Hub | 改名在 Hub 一侧完成，开源程序不受影响 |
| 默认 Hub | 内置 `https://droppen.app`，用 `RH_HUB_DEFAULT` 可以改 |

**码前缀的切换顺序**

读的一侧已经两种前缀都认。写的一侧一项一项改，每一项都要等读它的程序普遍升级以后再改。两种前缀后面的部分完全一样，所以哪里被旧程序拒绝，把前缀改回旧的就能用。

1. 先改安装命令：「添加设备」对话框里那条 `DROPPEN_CODE=… sh` 命令，等认 `DR1-` 的版本成了 getdroppen/droppen 上的最新版，就可以写 `DR1-`，因为安装脚本总是下载最新版。例外是用 `DROPPEN_VERSION` 钉住旧版本的安装，它会拒绝这个码。
2. 过一两个版本再改配对文本（`RHP1-` → `DRP1-`）。读它的是另一台设备上的页面，可能是缓存的旧页面或旧的 PWA，会说「不是配对码」；所以同时加一句提示，让对方刷新页面。
3. 本机「登录 Droppen」弹窗交回本机页面的码，只在知道打开它的本机服务（agent）版本够新时才写 `DR1-`。旧的本机页面会把这条消息当成无效，直接忽略。
4. 「用别的方式装的」那条从标准输入读码的命令（对方服务的版本不知道）放到最后，也可以一直不改。
5. 中继令牌：Hub 先认（已经认了），中继都升级以后 Hub 才开始发 `DRR1-`。旧中继拿到 `DRR1-` 令牌时要另外设 `RH_RELAY_ID`。只影响运营者。
6. 永远不去掉对 `RH1-`/`RHP1-`/`RHR1-` 的接受。

### 7.3 Tauri 应用：高保障路径（以后）

**是什么**

开源的 `droppen`（本机服务加内嵌界面）打包成签名、公证的 Tauri 应用。按 §3 的模型，它是一台**可运行 AI 的设备**；用户亲手登录，所以「可管理账号」默认开。

- **进程**：Tauri 外壳 + sidecar `droppen`。由应用注册成登录项或 launchd 代理，只对新安装生效。
- **界面**：webview 加载 **App 包里**的 SPA 资产，经 IPC 访问本机 API 与网关；本机账号由应用经 IPC 代为登录。
- **上手**：首次打开的引导就是「登录 Droppen」。应用用系统浏览器打开 `https://droppen.app/link#droppen-link=…`，走 §4.9 的浏览器交回。用户在 droppen.app 上登录或注册并确认；注册码封装后经系统浏览器交回应用自己的服务器，在进程内注册。也可以跳过，只在这台设备上使用。
- **更新**：更新器校验发布者签名。Hub 最多通告「有新版本」，不能下发二进制。

**为什么是高保障路径**

| 问题 | droppen.app 上用浏览器 | 本机浏览器打开本机界面 | Tauri 应用 |
|---|---|---|---|
| 运行的 JS 从哪来 | 每次加载都由**闭源 Hub** 投递 | 本机二进制内嵌（开源构建） | 签名 App 包内（开源构建，经公证） |
| 能否给某一个用户下发恶意代码 | 能：Hub 可以只给某个用户发一份不同的 JS，它能使用、也能读出那里的钥 | 不能经 Hub 做到 | 不能，除非伪造发布者签名 |
| 钥在哪 | IndexedDB，封在本机钥下（同源代码读得出） | 本机 `droppen` 的 0600 文件 | 钥匙串，只给本 App 的代码身份 |
| 怎么核实 | 比对公开的哈希 | 开源，可以自己编译 | 开源 + 可复现构建 + 签名 |

droppen.app 网页适合随手打开；要高保障的人用本机界面或 Tauri 应用。在这两条路上 Hub 不投递代码，它闭不闭源都不影响。

**签名还带来**

- 钥进钥匙串。0600 文件挡不住同一用户的进程（§5.5），这一条代价因此消除；这也是 Tauri 应用可以默认开「可管理账号」的前提。
- 「我的设备」等账户管理可以直接在应用里做。

**手机**：Tauri 移动版是一台不可运行 AI 的设备，可管理账号默认开；钥在 Secure Enclave 或 Android Keystore，用 `HubShell` 直连。

**现在就要守住**：

- 外壳抽象（以后加 `TauriShell`）；
- 钥存储走 `KeyStore` 接口；
- 不用 cookie，不靠 `reload()` 切换状态；
- 路由不假设「源 = 服务器」；
- 网关协议与传输方式无关；
- 上手流程不依赖 `window.opener`；
- 只用 `docs/hub-api/` 里写明的接口。

**现在不做**：任何 Tauri 代码。

### 7.4 两个名字落地时的取舍（S5-1）

- **一个二进制**：cargo 的 `[[bin]]` 仍叫 `remote-hand-server`（supervisor、自升级、测试和桌面 sidecar 都按这个名字找）。`droppen` 在发版包里是硬链接，在 `<数据>/bin` 里是符号链接。
- **`argv[0]` 只管说法**（`server-rs/src/program.rs`）：最后一段是 `droppen` 时，帮助的第一行、用法、错误里建议的命令说 `droppen`，stderr 前缀是 `droppen:`；其他名字（旧名、`up` 拉起的子进程、测试）一字不改，`remote-hand:` 前缀照旧，grep 它的脚本不受影响。只改写命令，不改写路径和文件名（`<数据>/bin/remote-hand-server`、`remote-hand-server.prev` 原样）。`[rh-up]` 日志前缀不变。
- **发版**（`scripts/release-public.sh`）：每个平台两个包。新包 `droppen-…` 里 `droppen` 是实体、`remote-hand-server` 是它的硬链接（tar 里只存一份数据；单独解出任何一个都是完整文件，符号链接单独解出来是悬空的）；旧包 `remote-hand-…` 和以前一样只有 `remote-hand-server`。`SHA256SUMS` 列全部。每个包都解开再查一遍：成员不多不少、每个程序与前面检查和冒烟过的二进制逐字节相同、标记/glibc/`$HOME` 路径等检查再跑一遍；新包里的 `droppen` 再冒烟一次（以新名字运行）。旧名资产至少发到 2027-01-31（`OLD_ASSETS_UNTIL`）。
- **安装脚本**：先读 `SHA256SUMS`，列了新包就下新包（取 `droppen` 成员），否则（或下载失败）下旧包（取 `remote-hand-server`）。装出来的仍是 `<数据>/bin/remote-hand-server`，与以前完全相同；plist、systemd unit、数据目录一律不动。旁边建 `droppen -> remote-hand-server`（相对链接，自升级换文件后照样指向新的；已有一个不是链接的 `droppen` 就不动它，并提示）。打印给人敲的命令（改密码、最后的 `Command:`）用 `droppen`；手动启动的 `up -- serve` 命令仍用旧路径，脚本靠这个路径认出手动启动的监督进程。卸载时连这个链接一起删。只认旧资产名的旧版脚本照常能装，所以旧资产要继续发。
- **网页里让人敲的命令**：`GET /api/auth/status` 加 `droppenPath`——运行中二进制旁边的 `droppen`（是同一个文件才算），否则二进制本身；家目录写成 `~`，因为这个接口不登录也能读，不把账户名放出去。登录页的提示与「设置 → 安全」里的 `user add` 用它（需要时加引号）；旧服务器没有这个字段，网页说 `remote-hand-server`。
- **droppen.app「已经用别的方式装了」的 stdin 命令**：对面服务器的版本未知（还没进账户），所以仍是 `~/.local/share/remote-hand/bin/remote-hand-server`；等 Hub 报告的账户内 agent 最低版本到了这一版再换成 `droppen`（`web/src/features/landing/get-droppen.ts` 里有 TODO）。
- **会话里的通知命令**：`PATH` 上有 `droppen-notify` 和 `rh-notify` 两个名字（同一个脚本；脚本用 `DROPPEN_NOTIFY_AS` 把自己的名字告诉二进制，用法和错误说被敲的那个名字）。技能改名 `droppen-notify`，每次启动会话时删掉旧的 `skills/rh-notify`，CLI 只列一个。降级到旧版时旧版不认识新目录，两个会同时列出，直到再升级。
- **没做**：桌面应用的 sidecar 改名；设置页的 `setup-code`、`open` 也改用 `droppenPath`；`[[bin]]` 改名、数据目录、`DROPPEN_*` 环境变量别名、`droppen service install` 留给以后。

## 8. 里程碑（按 PR 拆）

> 本节原是内部的 PR 计划（排期、测试环境、上线步骤与验收记录），不在公开仓库。代码注释里的里程碑编号（`S1-4`、`SC-3` 等）在下面各小节里各有一句说明；S1-4、S1-5、S1-7 的实现设计与评审结论讲的是开源代码，保留在原处。

### S0 开源边界（与其它工作并行）

- S0-1：`docs/hub-api/`，开源客户端用到的 Hub 接口（§2.1）。
- S0-2：云端 e2e 改为对固定版本的 Hub 镜像运行，不需要 Hub 的源码。
- S0-3：Hub 移到单独的仓库；开源 CI 拒绝 `rh-hub` 依赖。

### S1-1 到 S1-3 本机登录

- S1-1：注册改为库函数；链接在运行中启停；对移除做出反应；`status`、`unenroll`。
- S1-2：本机 API 里的 Droppen 链接（§4.5）。
- S1-3：在本机设置里登录 Droppen（§4.3、§4.4）。

### SC 链 v3：一切都是设备（S1-4 之前）

- SC-1：`rh-proto` 与 TS 读 v3 条目（§3.2）、按操作的新鲜度（§3.4）、互通向量；只读，不写。
- SC-2：Hub 应用 v3；按 `devices.max`、`hosts.max` 计数（§3.8）；`chain/3` 门槛（§3.5）。
- SC-3：agent 与 SPA 验 v3；每台设备都能控制；非 admin 的拒绝清单（§5.4）。
- SC-4：「我的设备」（§3.7）与 v3 写入。
- SC-5：§3.6 全表的独立安全评审，之后 droppen.app 开始写 v3。

### S1-4 到 S1-7 本机里的统一会话列表（SC-3 之后）

- S1-4：本机安装用自己的身份控制其它设备。
- S1-5：本机跟随每台设备的副本（`C/rh-client`、`S/aggregator/`、`copies.db`）。
- S1-6：web 按机器区分传输、查询、行与存储（§6.7 的 M-0、M-1）。
- S1-7：本机列表里出现其它设备的会话（本机网关）。

**S1-4 实现计划**（先写计划再实现；D1–D4 照旧）

- **身份**：本机这台设备就是它注册的那台 host，一个 id、一对钥：`agent/machine-key.json` 的种子展开的 Ed25519 + X25519（`DK`，§3.3）。设备角色用 `HubLogin{DeviceSession}` 换设备会话（SC-2 已允许 host 以设备身份登录；Hub 第一次时给它建 `kind: host` 的设备行）。agent 角色与设备角色靠签名标签分开，不再生成第二对钥；旧版留下的 `aggregator/device-key.json` 照旧删掉。
- **交钥：谁封、何时、Hub 存什么**
  - 本机控制端（`S/aggregator/`，随链接一起起停）在三件事都成立时运行：链上它是当前设备、链接在跑、所有者没关掉聚合（命令行注册、从没在本机设置里登录过的安装默认开）。它换设备会话 → 读信箱（`GET /v1/mailbox`；Hub 只把用途 2 以外的信给设备角色）→ 用途 5 逐封用 `open_host_key` 打开：发信者必须是当前 admin、收件者是自己、钥对得上链上那一轮的提交，并且那台 host 的接受策略放它进来（D2），否则丢弃；成立的存进 `aggregator/keys.json`（目录 0700，文件 0600），再确认（`DELETE`）。链还没追上的信（提交或发信者未知）留在信箱里，链前进后再试。其它用途一律确认丢弃（它不是 admin，收不到 `UK`）。
  - **自动索钥**：接受它的某台 host 的最新一轮它没有，且这个缺口持续 20 秒（留给 admin 主动投递的时间）时，向每台当前 admin 发用途 4（无秘密，epoch 0），10 分钟至多一次；时间记在 `keys.json` 里，重启不重发。链变化时立刻再看一次信箱；有缺口时每 5 秒看一次，没有时每分钟一次（S1-5 换成 `/v1/client` 的 `mail` 推送）。
  - **admin 一侧**（`W/lib/hub/account.ts`）：用途 5 的收件者把非 admin 的 host 也算进去（D1 下本机安装都是非 admin host）；收到 host 的用途 4 也自动回复，只回接受它的 host 的已提交 `MK`（每次至多 96 封，最新的先），从不回 `UK`。host 还没以设备身份登录过（旧 agent、聚合从一开始就关着）时 Hub 回 400（只认 Hub 那句「a computer gets it once it signs in as a device」），界面记为「它登录后自己会要」，不算失败。对同一台设备的完整回复 10 分钟至多一次；其间只补比已封给它的更新的轮次——注册时弹窗封过的、轮换时主动封过的都算（评审 S4、N7）。
  - **接到注册流程**：弹窗写完 `[AddDevice3{host, admin: false}, KeyEpochs]`、把它自己的 `MK` 交出去之后，最多等 20 秒，等它的设备角色出现（它在链上看到自己后 1 秒内签入），再把接受它的 host 的各轮 `MK` 封给它（`sendHostKeys`）；等不到也不算失败，由它自己的用途 4 补上。「添加可运行 AI 的设备」的命令行注册在后台做同样的事。已注册的安装升级后自己发用途 4。
  - **Hub 存什么**：信箱里的密文（用途 5，收件者确认即删）与不含秘密的用途 4。
  - **不需要新条目**：注册那一批之后链序号不再变，它就能拿到其它 host 的 `MK`——前提是这些 host 的接受策略放它进来；默认只放 admin，那时它一把也拿不到（D2），要 admin 先放行。
- **门槛 `policy/1`**：旧 agent 会把全部 API（包括自升级）放给任何连得上它的设备（§5.4）。
  - Hub：签中继票据（`peers`）与 Hub 自己路由 `OPEN` 时，请求方不是 admin，就只给声明过 `policy/1` 的 host（以 agent 最近一次签入时的声明为准，退回旧版的 agent 下次签入即失去它）；admin 不受限。这是纵深防御，真正的线是 agent 自己的准入与拒绝清单。
  - 界面：「谁可以控制这台设备」在那台 host 没声明 `policy/1` 时不许放进 admin 以外的设备，写明先在那台设备上更新 Droppen。
  - 本机控制端的状态里每台 host 带 `policy`（是否声明），S2 开通道前照此检查。
- **移除**：admin 在「我的设备」移除它，v3 链写 `RemoveDevice3`；它能控制某台 host 时（或者勾着默认的「同时轮换内容密钥」），同一批里每台剩下的 host 的 `MK` 加一轮（`rotate(…, {userKey: false})`；D1 下它从没持有 `UK`，`UK` 不轮换），新钥只封给剩下的设备。Hub 在同一事务里删它的信、吊销它的设备会话与票据。本机 agent 从自己的链上看到移除：停链接与控制端，删掉整个 `aggregator/`。
- **自升级**：SC-3 的拒绝清单（`S/agent/bridge.rs`）按请求方是否 admin 判；本机安装在 D1 下永远不是 admin，它经通道调其它 host 的 `POST /api/server/self-upgrade` 回 403 `limited_device`；Hub 的 `policy/1` 门槛让它到不了没有拒绝清单的旧 agent。S1-4 不实现本机开通道（S2-1）；验收用 devkit 以本机安装的同一身份（同一颗种子）开通道来证明。
- **兼容与回滚**：
  - 旧 agent（v0.2.118）从不以设备身份登录。它若从没以新版登录过，Hub 就还不认识这台设备，给它的用途 5 被 Hub 400 拒掉，界面不当失败；若新版登录过、再回滚，Hub 仍认识它，用途 5 会照收、堆在它的设备角色信箱里，直到那个角色的上限（256 封、1 MiB、同一发件者 128 封）。Hub 的信箱上限按角色分开算（评审 S3），所以它堆满也挡不住 host 自己的 `MK`（用途 2）；旧 agent 不发用途 4。
  - 旧网页不给 host 封钥，也不回 host 的用途 4（确认后忽略）：新 agent 每 10 分钟再要，新网页上线后补上。
  - agent 回滚到 v0.2.118：`aggregator/keys.json` 留在盘上没人读。断开（`unlink`）时 v0.2.118 照样整目录删；**被移除时它不删**（它的移除处理不认识这个目录）。重新升级后，链接一启动、看到状态已被移除就删（`startable`）；新版运行中，Hub 说「已被移除」（离线期间被移除，链上的移除它收不到）时也立刻删（评审 S2）。`cloud.json` 新加的字段旧版忽略；Hub 回滚只是少了 `policy/1` 过滤与按角色的信箱上限。
  - 链：无新条目、无新动作、无格式变化。
- **安全评审之后（S1–S5、N1–N7）**：
  - `keys.json` 的每一次写入（新钥、记下索钥、丢掉不再接受它的 host）都在目录锁下、只在「聚合开着、链接没停」时写；这个句柄读到或写过的文件若已被删（关闭、断开、被移除），一律不写回，并丢掉内存里的钥。链接停时控制端 10 秒内不停就直接结束。
  - `cloud.json` 读不出或解析不了时按「关」处理，只记一次日志。
  - 流量捕获（`RH_HUB_API_TRAFFIC`）里的设备会话 token 抹掉；`Verdict` 的 `Debug` 不打印钥；内存里的钥十六进制串在释放时清零。
  - 「谁可以控制这台设备」在还不知道那台设备是否声明了 `policy/1`（`/v1/me` 还在加载）时，也不许放进 admin 以外的设备。
- **复核之后（2026-10-19）**：
  - admin 回复的节流窗口只在 `0 ≤ 现在 − 开始时间 < 10 分钟` 时算数：本机时钟回拨、记录坏了，都按「没有窗口」完整回复。最坏情况下，丢了信的设备约 20 分钟后才拿到全部钥（它 10 分钟问一次，窗口也是 10 分钟）。
  - 控制端索钥按缺口退避：同一个缺口（`host/轮次` 的集合）第一次问后 10 分钟、第二次后 1 小时、之后每 6 小时；缺口合上或变了（某台 host 的最新轮次动了、host 增减）就从头来，但离上次索钥至少 10 分钟。admin 长期不在线时，一台安装一天约 6 封，而不是 144 封。
  - 一封存不下的钥：同一轮次已有另一把钥（确定冲突）立刻确认丢弃；磁盘写不进去，试 5 轮后确认丢弃，之后它会再要。
  - 链接重新启动前，先把已经自己结束的旧链接（及其控制端、写流任务）停掉。
  - `cloud.json` 里只有 `{"aggregate": false}`、没有 `linkedAt` 仍算开：文件是整份写的，改访问权限等任何更新都会写出默认的 `aggregate: false`；只有 `aggregatorOff`，或从本机设置登录且没要聚合，才是所有者的「关」。手改 `cloud.json` 只让控制端停，不删 `aggregator/keys.json`；本机设置里关闭（`POST /api/cloud/aggregator`）才删。
  - **发版顺序：先 Hub、后网页。** 新网页会给 host 的设备角色封用途 5；旧 Hub 的信箱上限不分角色，那些信可能挤掉 host 自己的 `MK`（用途 2）。
- **D1 的调和**：同意页的「可管理账号」做成开关，默认关；请求来自可运行 AI 的安装（今天的本机安装都是）时开关置灰，旁边写「可运行 AI 的设备暂时不能管理账号」。批里永远是 `admin: false`；`joinAction` 与两份验证者是后两道。D1 解除（Tauri 应用能作证、签条目）时只需去掉置灰。

**S1-5 实现计划**（跟随每台设备的副本；先写计划再实现）

- **先把 `rh-client` 从 `rh-devkit` 里抽出来**（只搬家，不改行为），再在它之上跟随副本。
- **`C/rh-client` 的边界**（开源，只依赖 `rh-proto`，不依赖 `rh-hub`）：
  - 搬进去：`HubUrl`；设备会话（挑战 + `POST /v1/session`）；信箱的 HTTP（读、确认、发索钥）；取页（`GET /v1/streams/{stream}/pages/{hash}`）；`/v1/client` 的 `Link`（签入，按 `ch` 分发帧，把带 `ch` 的 `stream_synced`/`stream_refused` 送到对应订阅，其余控制消息给调用者，连接关闭的通知）；流读者：`SUB` → `GONE`/`CHECKPOINT`（页经 HTTP 取，按 manifest 校验）→ `RECS` → `HEAD` 才放行 → `stream_synced`。读者有两种：一次读完的，和常驻的、能从游标续订的跟随器。三种 fold（索引、状态、会话）也搬进去。HTTP 与 WebSocket 流量捕获留一个钩子，devkit 和 agent 各挂各的。
  - 留在 devkit：注册、配对、恢复、开通道、e2e 与压力测试。devkit 改为依赖 rh-client，并把搬走的名字原样再导出，对外 API 不变。
  - agent 的 `aggregator/hub.rs` 变薄，改调 rh-client。
  - 以后：S2-1 把开通道（Noise 发起方、中继票据）也搬进来；桌面应用的 Rust 一侧直接用 rh-client。网页仍用 TS 版（`W/lib/hub/streams.ts`），两边以 `docs/hub-api/` 和互通向量为准。
- **跟随谁（`S/aggregator/access.rs`）**：
  - 只跟随「接受本机控制、且本机持有其钥」的 host，也就是控制端保留下来的钥（D2：`hosts_admitting`；收窄或移除时钥先被删）。不跟随自己。
  - 每台 host 跟随三样：
    - 索引流、状态流：按钥的轮次各有一个名字（`index_stream(MK_e)`）。订阅最新的至多 3 个轮次的名字，以「存在且最新」的那个为准；定下以后退订更旧的。更新的名字一直订着，因为轮换后 agent 会搬过去。
    - 会话流：只跟随索引里 `updatedAt` 最新的 5 个（§6.3）。名字按索引行的 `se`，没有就用最早的轮次。「打开过的 20 个」留给 S1-7 的网关。
  - 订阅上限：每条连接 192（Hub 上限 256 的四分之三）。索引、状态必给；会话流有空位才订。
- **怎么跟（`S/aggregator/copies.rs`，独立任务，随链接起停）**：
  - **连接**：用自己的设备会话开一条 `/v1/client` 长连接（设备角色签入，SC-2 已允许 host）。每 20 秒发 `PING`，60 秒没有回音就断开重连。
    - 重连指数退避：0.5 秒起，30 秒封顶，带抖动。
    - 401 或 4401：重新换会话。
    - `device_removed`：同 S1-4，删掉 `aggregator/`，静默退避。
    - `host_devices_off`：10 分钟后再试。
  - **游标**：每条流的游标是它最后放行的、机器签名的流头。
    - 重启时先用 host 的签名钥验这个流头，再用 `LogReader::from_head` 接着读，`SUB from_seq = head.next_seq`。
    - Hub 回 `GONE`（尾巴已压实）时，从 `CHECKPOINT` 的 manifest 与页重建。
  - **放行**：只放行签名流头覆盖的记录。记录打不开、头对不上、会话条目断档时，这条流从零重建，退避；重建期间旧状态留着。
  - **同一条连接上的推送**：
    - `mail` 推送立刻叫醒控制端读信箱；S1-4 的 5 秒轮询降为兜底。
    - `presence`：记下每台 host 在线与否，写进状态（S1-7 要用）。
  - **钥**：控制端每轮把「host → 签名钥、各轮 `MK`」交给副本任务（只在内存里，`watch`）。新轮次的钥到了就订新名字；缺钥的名字不订，等 S1-4 的索钥补上。
- **`copies.db` 的数据模型**：
  - **位置与权限**：`<实例>/aggregator/copies.db`。目录 0700；文件先以 0600 建好再交给 SQLite，WAL 与 SHM 随之也是 0600。`synchronous = NORMAL`，因为它是可重建的缓存。
  - **表 `copies(host, kind, name, stream, epoch, head, state, updated_ms)`**，主键 `(host, kind, name)`：
    - `head` 是机器签名的流头（编码后）；
    - `state` 是 fold 后的**明文** JSON：索引行、整份状态、会话条目。
    - 为什么不再加一层加密：它与 `keys.json` 同盘同权限，拿到 `keys.json` 也能读到同样的内容。§5.5 的威胁表如实写明。
  - **表 `meta`**：绑定账户与本设备 id。对不上（另一次注册留下的）就整库清掉重来。
  - **上限**：每台 host 1 份索引、1 份状态、至多 5 份会话。单份会话副本的明文超过 8 MiB 就不再存，只记「太大」。索引与状态不另设上限，它们本身受 agent 的体积预算约束。
  - **留存**：
    - 会话一旦不在「最新 5 个」里，它的行当场删除。
    - host 不再接受本机（收窄）、不再是 host、被移除：它的所有行当场删除。
    - 关闭聚合、断开、本机被移除：整个 `aggregator/`（含 `copies.db`）删除。
  - **写入守卫**：同 `keys.json`（S1-4 评审 S1）。每次写都在目录锁下，只在「聚合开着、链接没停、库文件还在」时写。库文件被删（关闭、断开、移除）后，副本任务关掉连接、丢掉内存里的状态，不再写。删除在 1 秒内完成：`on_removed` 当场删目录，副本任务随链接的令牌停下。
  - **日志**：不记任何内容或钥，只记 host id、流的种类、序号、错误种类。
- **状态流加字段**（agent 写）：
  - `os`：`{name: "macos"|"linux"|…, version, arch}`。macOS 读 `SystemVersion.plist`，Linux 读 `/etc/os-release`，启动时算一次。
  - `agentVersion`：本程序的版本。
  - 旧网页忽略未知字段；网页仍以 `settings.platform`、`settings.serverVersion` 兜底。
- **本机 API**：`GET /api/cloud` 的 `aggregator.hosts[]` 加 `copy: {indexRows, indexAsOfMs, statusAtMs, sessions, online}`，只有数量与时间，没有内容。内容本身留给 S1-7 的网关。
- **兼容与回滚**：
  - 不改 Hub、不改协议：只用已有的 `/v1/client` 订阅与取页接口；Hub 没有迁移。
  - 旧网页、旧 agent 不受影响：状态流的新字段是加法。
  - agent 回滚到 v0.2.121（S1-4）：`copies.db` 留在盘上没人读；断开或被移除时，那个版本照旧整目录删。只有「收窄」不会删回滚期间那台 host 的旧副本；重新升级后的第一轮就删。
  - 发版：只发 agent，与网页、Hub 无先后。
- **验收与测试**：
  - rh-client：fold 的单测随代码搬家。跟随器用 `rh-proto` 的写方造记录与签名头，覆盖四种情形：只放行签名头、`GONE` + `CHECKPOINT` 重建、从游标续订、坏记录重建。
  - agent：
    - `copies.db` 的权限、绑定、按 host 清除、续订；
    - 跟随计划：最新 5 个会话、轮次名字的取舍、订阅预算；
    - 守卫：关闭、停止、删除之后都不写回。
  - Hub（自己起的容器）：host 的设备角色经 `/v1/client` 订阅另一台 host 的流并取页；被移除后订阅被拒。
  - 本地端到端加一段，3 台 host（真实 agent，其中 1 台离线）加本机安装：
    - 5 秒内拿到三份索引；
    - 新会话 5 秒内进副本（索引与会话流）；
    - Hub 重启后自己续上；
    - 移除后 1 秒内删钥删库；
    - 金丝雀命中 0：副本里的明文只在本机文件里。
- **实现时补上的**：
  - **收窄与移除时在文件层面尽力抹掉明文**：`copies.db` 开 `secure_delete`，删掉的行就地清零；删一台 host 的行之后再 `wal_checkpoint(TRUNCATE)`，WAL 里残留的旧页也一并清空。关闭聚合、断开、本机被移除时，在目录锁下把整个 `aggregator/` 改名移走（从这一刻起谁也找不到它，写入守卫一律拒绝），再在单独的线程里把每个文件（`copies.db` 及其 `-wal`、`-shm`，`keys.json`）逐个写零、同步、立即删除；服务正常停止时等它最多 5 秒；没做完的（进程中途停了，比如关闭后紧接着自升级重启）在下次启动时补做——不论聚合开着与否——每次删除之前、打开副本库时也会补做（安全评审 F7、复核 N4、M1）。SQLite 连接设了「关闭时不回写日志」，仍开着的连接不会把明文写回正在清零的文件；日志在 checkpoint 后缩回 1 MiB 以内，副本库关闭时（不在删除中）截断为空，旧帧不会跨重启留着（M2）。单测在文件字节里找被删 host 的金丝雀，去掉这几步就失败；e2e 收窄一台 host 后同样在 `copies.db` 及其 `-wal`、`-shm` 里找。**这只在文件层面成立**：碰不到 APFS 本地快照、Time Machine 与其它备份、固态盘的磨损均衡；`aggregator/` 因此打上 Time Machine 排除标记（它本就是可以从 Hub 重读的缓存），§5.5 的威胁表如实写明。以后的做法是加密擦除：每个安装一把放在钥匙串里的静态钥、每台 host 一把数据钥，删钥即抹；不在 S1-5。
  - **控制端每轮先删钥**：不再接受本机的 host，在本轮任何 Hub 请求之前就从 `keys.json` 删掉（Hub 连不上时也删），再把钥交给副本任务；副本任务自己也盯着链，收窄当场删行、停订阅。
  - **rh-client 的接口**：`follow::Follower`（无 IO，副本任务与 Hub 的库测都用它）；`link::connect_device` 区分 Hub 的拒绝码（`denied`、`removed`、`host_devices_off`）与网络失败；`Link::silent_ms` 判断 60 秒没回音；fold 改成逐条 `apply`。
  - **流量捕获**：本机设备角色经 rh-client 的请求与 `/v1/client` 控制消息都记成 `"src": "agent"`，设备会话 token（应答的 `token`、签入的 `session`）抹掉。
  - **状态流**：Linux 的 `os` 另带 `distro`（`os-release` 的 `ID`）。网页暂不读新字段，仍用 `settings.platform`、`settings.serverVersion`（S1-7/S3-2 再换）。
- **安全评审之后（S1-5，2026-10-19）**：
  - **F1 恶意 Hub 撑爆内存**：`CHECKPOINT` 一到就验 manifest 的机器签名，验不过不取任何一页；每页取到就对 manifest 验哈希，不等全部取完；取页的应答体超过 `MAX_PAGE_LEN` 即拒（先看 `Content-Length`，再边读边数）；会话副本取页累计超过 8 MiB 即记为「太大」。单测：伪造的 checkpoint、超长的页（有长度与无长度两种）。
  - **F2 回滚**：跟随器带下限（`follow::Floor`：见过的最新流头的序号与机器时间，从库里的游标起算，重读时也带着）。比下限旧（时间更早且序号不超过它）的结果拒收；`GONE` 只接受已放行位置之后的。机器删掉一条流又从 0 重写时，新流头的时间更晚，照常读。下限在哪里查，见下「复核之后」N1。
  - **F3 性能**：会话一边 fold 一边按条目计字节，过 8 MiB 立即停；只有心跳的流头（不放行新内容）不再写库；会话行在同一条连接里至多 10 秒写一次（第一次立即写，断线时全部写出）；「太大」从库里带到重启之后，不再重取。
  - **F4**：迟到的「太大」与内容更新一样，只对当前还在跟随的流与 host 生效；写库时丢掉不再跟随的 host 的行。单测：被清掉的 host 的迟到更新不留下任何行。
  - **F5**：取页时也听停止信号；每条连接在第一次订阅之前先按当时的链与钥重算一次要跟随的 host，连接期间被收窄的 host 不会带着它的钥被订阅。
  - **F6**：开 WebSocket（TCP、TLS、升级）最多等 20 秒；副本任务在登录、连接时也听链接的停止信号。`Link` 被丢掉时它的读任务立刻结束。
  - **F7**：见上「在文件层面尽力抹掉明文」。
  - **F8**：`GET /api/cloud` 的 `copy`（其它设备的副本数、时间、在线与否）只给能管理链接的本机账号，以及所有者打开了远端权限的本机账号；本机鉴权关着时谁也不给（§4.6）。
  - **F9**：流量捕获里，设备角色的 `known_client`（`GET /v1/me` 的 `kc1.…`）与账户邮箱也一并抹掉，不论在应答的哪一层。
- **复核之后（S1-5，2026-10-19）**：
  - **N1 诚实的重读被下限拒掉**：从 0 重读时 Hub 先发最新的 manifest（压实时签的，往往比最近的记录旧），再发记录、最新的流头。原先在 checkpoint 上就按下限拒收，正在写的流一出错就冻住，直到对方再压实（空闲 5 分钟、忙时 30 分钟）或离线的 host 回来。现在追赶途中不查下限，只在结果变得可见时查：追赶结束（`stream_synced`）时，读到的最新流头仍低于下限，说明 Hub 扣着后来的内容或在回滚，按出错处理、本机留着原副本；之后的每个流头也查。出错后先从库里的游标续读（一次），再不行才从 0 重读。单测：「落后于下限的 checkpoint，然后记录，然后最新流头」读通；追赶结束时仍低于下限则拒收。e2e 加了一段：在 Hub 的存储里删掉一条正在写的会话的尾巴，本机的副本出错、重读，之后的两行照常进副本，期间原有内容一直在。
  - **N2 心跳**：只有心跳的流头不重写副本，但每分钟至多一次把「截至」时间与游标往前挪（只改流头一列），安静但在线的设备不再显示几小时前。只有库里流头的位置与心跳相同时才挪（在同一个事务里先读后写）：某次写入失败时，游标不会越过副本缺的记录（M3）。
  - **N3 时钟超前**：比本机时钟超前一天以上的流头，下限的时间部分按本机现在算；时钟曾超前（比如到 2100 年）的设备改好时钟、从 0 重写一条流后照常读。
  - **N4**：删除时的覆写改为直接把文件写零（不再开第二个 SQLite 连接走 `secure_delete`），并移出目录锁与异步工作线程，见上。
  - 会话的整份副本在跟随任务里至多每 10 秒序列化一次（追赶结束时立即）；替换下来的旧会话行即使删除失败也不再写入。

**S1-7 实现计划**（本机列表里出现其它设备的会话；先写计划再实现）

- **两半**：本机服务一侧是网关的 `dir` 与 `sub`/`copy`、内存里的副本、按需跟随打开的会话、`GET /api/cloud` 的新字段；页面一侧是 `GatewayShell`（只有副本）、统一列表、设备筛选、「需要你」、远端会话只读打开、远端权限开关。
- **网关 `GET /api/agg/ws?ticket=`**（`S/aggregator/gateway.rs`，§6.3 的一部分；管道 `open` 留给 S2-2）：
  - 升级前就拒：`cloud:*` → 403 `not_from_cloud`；本机鉴权关着 → 409 `local_auth_required`；调用者看不到其它设备 → 403 `no_remote_access`；登录已退出 → 401。票据同 `/api/events`（`scope=ws`，一次性），`auth::is_websocket_path` 加上这条路径；桥接的拒绝清单把它列为 `Never`（前缀规则本来就挡）。
  - 文本帧 JSON，`t` 标类型：

    | 方向 | 消息 |
    |---|---|
    | S → 浏览器 | `dir {machines: [{id, name, state: online\|offline\|unknown, sinceMs, copyAsOfMs, os, agentVersion}]}`：连上时一次，之后每变一次发整份（它很小，不做 `dir_delta`） |
    | 浏览器 → S | `sub {s, machine, name: {kind: index\|status} \| {kind: session, source, id}, from?, fromEpoch?}`、`unsub {s}`、`ping` |
    | S → 浏览器 | `copy {s, reset?, bodies, headSeq, epoch, headTimeMs}`：`reset` 时 `bodies` 从零重建副本，否则叠在已有的副本上；`bodies` 为空是心跳（「截至」往前走）。`missing {s}`：那台设备（暂时）没有这份副本，不终止订阅。`copy_error {s, code}`：`no_device`、`bad_name`、`too_large`、`gone`（设备离开了名单）、`overrun`（浏览器读得太慢，重订即可）、`limit`、`busy`（订得太快，稍后再订），终止这个 `s`（服务端也同时放下它，页面不必再 `unsub`） |
    | S → 浏览器 | `open` 一律回 `refused {p, code: unsupported}`；权限没了回 `bye {code: no_remote_access\|local_auth_required\|unauthorized}` 后关闭（4403/4409/4401）；`pong` |

  - `reset` 的 `bodies` 由内存里的 fold 合成，浏览器用同一组 TS fold（`indexFold`、`statusFold`、`feedFold`）得到同一份状态：索引是一条 `{reset, upserts: 全部行, removed: [], asOf}`；状态是最后一条状态体；会话是（裁剪过时）一条窗口页加一条 replace-from 的 `{fromIndex, items, endIndex, fp, prefixFp}`。Rust 的 `FeedCopy` 为此记下 `fp`、`prefixFp`（旧库里没有的按无，离线传输照常重发整窗）。之后的 `bodies` 就是 agent 写下、跟随器放行的原文，原样转发。`(fromEpoch, from)` 等于当前的（纪元、序号）时不发 `reset`，只发一条空的 `copy`；只带序号不带纪元的一律重发快照（换了纪元的流序号会重来）。
  - 上限：每条连接 512 个订阅；`sub` 限速（一次至多 256 个，之后每秒 32 个，超了回 `busy`）；发送队列 256 条且至多 32 MiB，放不下的订阅记为 `overrun`（只有一条也超过 32 MiB 时，队列空着就照发）；每发一帧至多等 10 秒，读不动的页面直接断开，不耽误重查权限。会话的整份快照至多 8 MiB（与副本上限相同），只在那份副本的槽锁下生成，不占登记表的锁。
  - 锁的顺序是「槽 → 登记表」；登记表放开锁之后才发新的 `dir`（watch 的写锁），网关先把 `dir` 从 watch 里取出来再放下订阅。否则「网关读着 `dir` 放订阅」与「登记表拿着锁发 `dir`」会互相等死（第二轮评审，有回归测试）。放开锁再发，就可能后做的先发出去：每份 `dir` 在登记表的锁里编号，比已发出的旧的就不发，刚离开的设备不会被一份旧名单带回来（第三轮评审）。
  - 同样的「登录是否还在」也加到了 `/api/events`：由某个登录打开的 feed 连接每 5 秒查一次，登录没了就以 4401 关闭。
- **数据从内存来**（`S/aggregator/live.rs`，`LiveCopies`，挂在 `agent::control::Control` 上，链接重启也不换）：
  - 副本任务的每条流把 fold 放进一个共享的槽：追赶结束（`stream_synced`，下限已查）时发布，之后每批放行的 `bodies` 在同一把锁下先 fold、再立刻发给订阅者。所以会话的 10 秒节流只管写 `copies.db`，不管网关。新订阅在同一把锁下登记并取快照，快照与之后的增量不会重叠也不会漏。流重读（出错、换钥、换纪元）时，新的 fold 追赶完才顶替旧的，期间页面看到的是旧的；索引与状态只接受不比已发布的更旧的纪元（与写库的规则相同）。
  - 副本库打开时把库里的行读进内存（Hub 连不上时也有「截至」的最后状态），之后请求路径上从不读盘。
  - 收窄、移除、不再运行 AI：那台的槽与名单项当场去掉，订阅者收到 `gone`；关闭聚合、断开、本机被移除：整个 `LiveCopies` 清空（与 `copies.db` 同时）。
  - **打开过的会话**：网关的会话订阅登记兴趣，副本任务据此跟随（§6.3「打开过的最近 20 个」）。排在索引、状态之后、每台最新的 5 个之前，受 192 的订阅预算约束；本来就在最新 5 个里的不占这 20 个名额；最后一个订阅离开后再跟 5 分钟；在跟随期间它的行也留在 `copies.db`。那台设备的索引里没有的会话不登记兴趣，立刻回 `missing`（不能拿随便编的名字让本机去跟）；兴趣表至多 64 项，满了让最早放下的那项让位，都还有人订着就不再登记。名单只在内存里，重启后从头来（之后再打开即可）。
  - 一开始没有、后来才建出来的流（先 `Missing`，没有 `Synced`），追上之后照样发布给网关；一批 body 折不进 fold（半途出错）的副本撤下，不再给页面看，下次重读再发布。
  - `dir` 的每一项：副本任务跟随的 host（接受本机控制、本机持钥，不含自己）；名字取状态里的 `displayName`，没有就用 id；在线与否取副本连接收到的 `presence`（连接断着时为 unknown）；`copyAsOfMs` 是索引最新的机器签名时间；`os`、`agentVersion` 取状态流的新字段（S1-5），没有时退回 `settings.platform`、`settings.serverVersion`。
- **权限（F8 的同一个谓词）**：`sees_other_devices(cloud.json, 所有者账号还在, 本机鉴权开着, 用户)` 只写一处，`GET /api/cloud` 的 `copy`、新字段 `seesOtherDevices` 与网关都调它。规则：
  - 默认只有**所有者**（在这台设备上登录 Droppen 的那个本机账号）看得到其它设备；其它本机账号要所有者在设置里打开才看得到。
  - 没有记录所有者的设备（命令行注册的）谁也看不到，直到能管理链接的人在 设置 → Droppen 点「这是我加入的」（`POST /api/cloud/claim`，记成所有者）。所有者账号被删了也一样：谁也看不到，直到有人重新认领。认领只在没有所有者时可以（否则 409 `has_owner`）。
  - 「谁也不会被锁在外面」的退路只用于**管理**链接（没有所有者或所有者已删时，任何本机账号都能管理、能认领），**不**用于查看。
  - 只有**所有者**能给别的账号开远端权限（`PUT /api/cloud/access`，否则 403 `not_owner`）：没有所有者时谁也不能开，免得开出来的权限在别人认领后生效。认领成功时清空之前的所有授权，谁还能看由新所有者决定；从本机设置登录、所有者换了人时也一样。两个认领同时来时，「还没有所有者」在 `cloud.json` 的文件锁里再判一次，输的那个得 409 `has_owner`（第三轮评审）。
  - 所有者与授权绑在账号上，不只看名字：`cloud.json` 的 `accountsSince` 记下给出时那个账号的 `createdAt`，删掉后同名重建的账号什么也不继承（所有者算没有，授权作废）。早先版本登录写下的所有者没有这条记录时，只认登录（`linkedAt`）之前就有的那个账号；早先的授权一律作废，由所有者重开。
  - `cloud.json` 读不出来或解析不了：拒绝查看（不按默认值猜），认领也拒绝（409 `unreadable`，不能盖掉所有者的设置）。
  - **本机鉴权关着时谁也不给**，网关回 409（本机网页一律要本机账号，没有回环例外）。理由：没有本机账号时，能连到端口的任何人（局域网上的人、DNS rebinding 的网页）在本机看来都是「所有者」；其它设备是另一个信任域（§4.6），不能因此向局域网敞开。只有自己用的电脑也一样：先在「安全」里建一个本机账号（「登录 Droppen」本来就要求它）。
  - 连接期间重查谓词、本机账号是否还在、鉴权是否还开、**开这条连接的登录是否还在**（票据记下签发它的登录；退出登录、被撤销或过期就 `bye unauthorized` 关闭，Bearer 调用者同样；说不出是哪个登录的调用者一律 401，不放行）：升级后立刻一次，之后每 2 秒，以及 `cloud.json` 每次改动（远端权限、认领、聚合开关、断开）时；`cloud.json` 的改动在升级前的检查之前就开始听，中间的改动不会漏。不再满足就发 `bye` 并关闭。
  - 只读就是只读：网关没有任何写路径，`open` 一律拒绝；页面上远端设备的传输是离线传输（写请求一律 503）。
  - `GET /api/cloud` 加 `seesOtherDevices`（调用者自己）、`canClaim`（没有所有者、调用者能管理链接时为真）与 `accounts`（只给能管理链接的人：每个本机账号、它是否能看、是否是所有者），给远端权限开关与「这是我加入的」用。
  - 日志：连接的建立与关闭（本机用户、订阅数），订阅只记设备 id 与种类；不记内容、会话 id 与标题。Hub 看到的只多了本机对已有会话流 id 的订阅，与 droppen.app 打开同一个会话时相同：没有新字段、新消息或新的明文元数据。
- **外壳的边界**（§6.2）：
  - `LocalShell` 不变：未登录、聚合关着、或这个本机账号看不到其它设备时，页面就是今天的单机页面，DOM 与请求都不变（只多一次 `GET /api/cloud`，与设置页共用缓存）。
  - `GatewayShell`（`W/features/gateway/`、`W/lib/gateway/`、`W/lib/machines/shells/gateway.ts`）：本机页面里由 `GatewayBoot` 按 `GET /api/cloud`（`seesOtherDevices` 且聚合在跑；每分钟重读一次，另外设置页一改就失效重读）开网关、关网关；网关回 `bye` 后也是先关掉，再按下一次 `GET /api/cloud` 决定开不开。`local` 照旧是 home、直连；`dir` 里的每台设备一份 runtime：`MachineMirror(GatewayCopySource)` 只在内存里（`persist: false`：不落 IndexedDB，也不用 WebCrypto，局域网 IP 打开的页面不是安全上下文）、`copySessions`、带只读文案的离线传输；`hold()` 只让它跟随打开的会话，不拨通道（S2）。`ShellKind` 多一个 `gateway`。名单清空、权限被收回、聚合关掉时关掉远端 runtime，回到 `local`。本机的会话列表与 feed 窗口副本（`feed-persist`）也只为 `local` 落盘。
  - `HubShell` 不变。共用：`W/features/machines/`（`needs-you.ts`、`device-sessions.ts` 改为读任一外壳的设备）、离线传输、`copySessions`、`MachineMirror`。
- **「在问你」与「截至」**：
  - 在问你：索引行的 `asking`（agent 每 3 秒列一次，`LIST_EVERY`），副本任务一放行就经网关到页面；列表行的标记、「需要你」台账（在问你置顶，其次未读，再次工作中）、设备筛选里的计数都读它。离线、或副本超过 15 分钟没动的设备按「静止」显示（`restingRow`），不算在问你。验收：5 秒内出现。
  - 截至：`copyAsOf` = 那台设备索引最新的机器签名时间（在线时心跳至多 5 分钟一次，`HEARTBEAT_EVERY`）。设备离线（或副本过期）时，筛选条里写「离线副本 · 截至 14:02」，只看这台时列表顶上也写一行。
- **各屏的改动**（只在页面上有两台以上设备时出现；单机模式一个像素都不变）：
  1. 会话列表（桌面侧栏、手机列表页）：过滤框下多一行设备筛选（26px 分段、可横向滚动）：「需要你 n」（有才出现）、「全部」、「这台设备」、每台远端设备（状态方块、名字；离线的写「离线副本 · 截至 …」）。选择按浏览器记住。「全部」按列表原来的顺序（工作中、未读、最新）混排，远端行的第二行以设备名开头；按项目分组时每台设备的项目各一组，组头写设备名。远端行的右键菜单只有「复制会话 id」。
  2. 会话页的空白处（桌面，没打开会话时）：「需要你 · 所有设备」台账（`NeedsYouLedger`），下面是原来的提示。
  3. 远端会话（`/m/<设备>/sessions/<source>/<id>`）：从副本读的只读会话，输入框的位置只剩一行「只读：这个会话在另一台设备上」（模型、思考强度、权限与附件都不显示）；页头有设备名；左栏其它入口仍是这台设备的。「需要你」里远端在问你的行按钮写「打开」（不是「去回答」），并注明要在那台设备上回答（S2 之前）。
  4. 标题栏与 app badge：在线设备的未读之和（`unreadAcross`）。
  5. 设置 → Droppen：「谁能在这里看到其它设备」：每个本机账号一行开关（所有者固定为开）；不能管理的账号只看到自己的状态；没有所有者时一个「这是我加入的」按钮（认领）；局域网上不加密的 `http://` 打开时，开关旁一行提醒（局域网上的人能看到传输的内容）；本机鉴权关着时仍是「先建一个本机账号」。
  6. 新建会话：远端设备列出但置灰，写「只读：在这里还不能在这台设备上新建会话」。窗格可以只读地显示远端会话；远端的终端、文件写明「需要那台设备本身」。
- **用词**：新文案收进 `W/i18n/cross-device.ts`（中、繁、日），`web/test/ui-wording.test.ts` 照查；服务端回给页面的提示（`bye`、`copy_error` 的 `message`）同样只说「设备」。
- **测试**：
  - Rust 单测：`LiveCopies` 的发布与投递顺序（快照与增量不重不漏）、三种快照与 TS fold 等价（含窗口页与 `fp`）、纪元不倒退、收窄与清空发 `gone`、兴趣计数与 5 分钟余留、兴趣表上限、按字节的发送队列、「读 `dir` 时放订阅」不死锁（带超时的回归测试）；`access::plan` 把打开的会话排在最新 5 个之前、受预算约束、最新的不占名额、行随之保留；网关的准入（`cloud:*`、鉴权关、没权限、所有者、被授权、收回后 `bye`、登录退出后 `bye`、没有所有者与 `cloud.json` 读不出时谁也不给）、认领、协议解析、上限与限速、索引里没有的会话；`FeedCopy` 的 `fp`；票据记下签发的登录、路径与桥接分类。
  - Web 单测与渲染测试：`GatewayCopySource`（reset、增量、`missing`、`overrun` 后重订、重连后带 `from` 重订）、`GatewayShell`（开关 runtime、`ShellKind`、不写 IndexedDB、不碰 `crypto.subtle`）、`feed-persist` 只为 `local` 落盘、列表模型（混排顺序、分组键）、多设备列表（筛选条、远端行的设备名、离线的「截至」、在问你）、单机列表与改前逐字相同、Droppen 的权限开关、只读输入框。
  - 本地端到端（只在本地 Hub，接在 S1-5 第 1 段之后）：所有者经网关拿到三台 host 的 `dir`（离线的那台 `offline` 且带 `copyAsOfMs`）与三份索引；在 s15one 上的一个会话里问一个问题，网关上的索引行在 5 秒内变成 `asking`（计时）；以局域网 IP 作 Host/Origin 连同一个网关；第二个本机账号没有远端权限时 `seesOtherDevices` 为假、网关 403，授权后能连，收回后 1 秒内收到 `bye`；订阅远端会话读到金丝雀，本机日志里没有。
  - 浏览器：一致性预览加 `?uiGateway=1`（假的 `/api/agg/ws`：两台远端，一台在问你、一台离线），截单机与多设备、中文与英文、1440 与 390；再用局域网 IP 打开预览页验证不依赖安全上下文。

**S1-7 落地时的取舍**（网关一侧、页面一侧）

- 计划照做。与计划不同或计划没写到的：
  - **设备筛选条会换行，不横向滚动**：一排连在一起的分段在 300px 宽的侧栏里放不下三台以上设备，离线那台的「截至」会被藏到右边。现在每一项是独立的方框（26px，`border-strong`，选中时墨底反白），放不下就换到下一行。方框里只写短的「截至 14:02」（今天只写时间，否则带日期），完整的「离线副本 · 截至 10月18日 14:02」是它的提示文字；只看这台设备时，筛选条下面那一行也写完整的。时间按应用的语言写（不按浏览器的）：中文、日文「10月18日 14:02」，英文 "Oct 18, 14:02"，24 小时制。
  - 「需要你 n」是筛选条的第一项（有才出现），手机上就靠它；桌面上没打开会话时，右边是「需要你 · 所有设备」台账（`NeedsYouLedger`），下面仍是原来的「从左边选一个」。
  - 「全部」按列表原来的规则（工作中、未读、最新）把几台设备的行排成一个列表；按项目分组时，每台设备的项目各是一组，组头写设备名；另一台设备的组记在 `owned:<设备>|<目录>` 下，只在内存里（不写进 localStorage：其它设备的路径不落在这台设备上），这台设备的仍是原来的键、照旧记住。筛选条选中另一台设备也只记在本页，记下的仍是「全部」。没有另做「按设备分组」：筛选条的每台设备一项已经是它。
  - 列表的偏好、折叠、feed 都是这台设备（home）的，哪怕右边打开的是另一台设备的会话（`SessionsLayout` 把列表包在 home 的 `MachineView` 里，右边的 `Outlet` 仍是路由的设备）。
  - **只读**：另一台设备的 runtime 是离线传输（写请求一律 503），`MachineInfo.readOnly`。会话页的输入框位置只剩一行「只读：这个会话在另一台设备上」，模型、思考强度、权限、附件都不显示（第二轮评审）；`useShowingCopy` 也认 `readOnly`，不会把消息放进发件箱排队；页头不显示置顶与会话菜单，元数据行开头是设备名；左栏除「会话」外的入口仍指这台设备；新建会话里它置灰并写「在这里只读：请在那台设备上新建会话」；列表里**只要不是这台设备的行就是只读**（不看 runtime 怎么说），置顶、改名、归档只会发给这台设备的服务；行与组头的右键菜单只剩「复制会话 id」。
  - **保存的东西里没有其它设备**：窗格与设置页的设备选择不列只读的设备，窗格的默认设备也不会是它，所以屏幕布局（会同步）里不会出现其它设备的 id。
  - 「需要你」台账里另一台设备上在问你的行，按钮写「打开」（不是「去回答」），标题下注明「要在那台设备上回答」（S2 之前）。
  - 设置 → Droppen：所有者一行写「登录了这台设备：总能看到」；没有所有者时只有说明与「这是我加入的」按钮（`POST /api/cloud/claim`）；页面是经局域网以 `http://` 打开的（不是 localhost、127.*、::1）时，开关旁多一行提醒：同一网络上的人能看到页面上显示的内容。
  - `GatewayBoot` 读设置页同一个查询（`cloudKey`），每 60 秒再看一次；网关连续三次连不上也再看一次；收到 `bye` 立刻关掉外壳，重读一次，仍该显示（比如权限收回又给回）就开一个新的网关。
  - 网关只发整份 `dir`，没有 `dir_delta`；没有开 deflate（除了一份会话快照，帧都很小）。`(fromEpoch, from)` 等于当前的时不发快照。
  - 页面一侧的副本：一条叠加的 `copy` 落在页面没有的副本上（快照丢了）不硬叠，退订后从头再订；折不进 fold 的同样从头再订，间隔 1、2、5、10、30 秒；`overrun` 与 `busy` 1 秒后原样再订；`gone`、`no_device` 时若这台设备还在名单里，过一会儿再订（名单被清空又列回来时）；其它 `copy_error` 就此结束（服务端已放下，页面再发一次 `unsub` 兜底）。
  - 打开过的会话的兴趣只在内存里，重启后要再打开一次才跟随。
- **与「新建会话改成空会话视图」合在一起**：远端只读会话在 `ComposerBox` 的位置只显示那一行只读说明；新建会话视图的设备选择只列能新建会话的设备（`DevicePicker writable`，除这台设备外都是只读的远端时干脆不出现），默认设备不会是只读的远端，打开 `/m/<只读设备>/sessions/new` 会转到这台设备的新建会话；统一列表里的「新建会话」仍在这台设备上。
- 验收记录（本地端到端的计时、评审后的复跑、浏览器与截图）是内部记录，不在公开仓库。

### S2 在本机里直接操作其它设备

- S2-1：本机开通道到其它设备（设备票据、中继签入、Noise 发起、`paths/1`、直连、通道池）。
- S2-2：经本机网关承载 rpc、feed、终端与文件（`GatewayPipes`、每台各自的 `SwitchingTransport`）。
- S2-3：跨设备分屏、新建会话选设备、每台设备的设置（§6.7 的 M-2）。

### S3 droppen.app 多设备同屏（S1-6 之后可并行）

- S3-1：droppen.app 同时运行每台设备（`HubShell`、`CopySource`，不再 `reload()`）。
- S3-2：设备仪表盘与跨设备「需要你」台账。

### S4、S5（以后）

- S4-1：每台可运行 AI 的设备选择谁可以控制它（链条目 `SetAccept`，已提前到 v3，见 §3.2 的实现备注）。
- S5-1：发版包带 `droppen` 二进制，`remote-hand-server` 作它的别名（§7.2、§7.4）。
- S5-2：Hub 二进制改名为 `droppen-hub`。

### 顺序与理由

> 内部排期，不在公开仓库。

## 9. 风险

| 风险 | 缓解 |
|---|---|
| 一切都是设备之后，任何 host 被攻破都能控制其它 host（v2 里机器只能冒充自己） | 默认非 admin；拒绝清单；移除即断并轮换 `MK`；告警；S4 的「接受谁控制」；被连的 host 记日志 |
| 钥在 0600 文件里的安装被开了 admin，本机 AI CLI 就能接管账户 | 默认关；开时警告；Tauri 与钥匙串之后才默认开 |
| 链 v3 迁移出错（映射不一致、门槛漏判、旧验证者撞上 v3） | 读先于写；门槛；Hub 的 v3 开关（§3.5）；独立评审（§3.6）；演练；导出备份；已有账户的第一条 v3 条目由用户单独确认 |
| 写入后不能回退到 N 以下 | 写之前所有组件都已经是 N；出问题就拨回 `read`，向前修 |
| 控制类新鲜度放宽到 7 天 | 管理类仍是 12 小时；诚实 Hub 即时吊销；提前提醒；过期后降级为只读 |
| 弹窗被钓鱼；登录 CSRF | 同意页、`cb` 规则、指纹、告警、`hosts.max`；nonce 绑定本机用户 |
| 旧 agent 放行所有设备的全部 API | `policy/1` 与 `chain/3` 同为门槛 |
| `window.opener` 被切断；移动版 Safari | 不设 COOP 并有测试；显式链接；真机验收 |
| SPA 改造面大 | M-0 到 M-4 机械拆分；截图不变；lint |
| 远端明文副本落在本机磁盘 | 说明；关闭聚合即删；按保留期裁剪 |
| 中继流量翻倍（droppen.app 与本机各开通道） | 列表靠流；通道按需建立、空闲关闭；局域网直连不计流量 |
| 开源客户端依赖了 Hub 未写进文档的行为 | S0-1 对照脚本与契约测试；S0-2 镜像 e2e；CI 拒绝 `rh-hub` 依赖 |
| droppen.app 的 JS 由闭源 Hub 投递 | SPA 开源；可复现构建与公开哈希；本机界面与 Tauri 应用 |
| 套餐的键改动让人困惑 | `machines.max` 作别名一个版本；界面只说「设备」与「可运行 AI 的设备」 |

**未决问题**

- **UI 文档的钥**：布局、待办上云后，非 admin 设备怎么读。届时定「文档钥」。
- **跨设备搜索**：排在 S2 之后。
- **客户自建中继**：先不开放。
- **其它平台**：跟着平台支持一起走。

## 10. 待定的问题

> 本节是给负责人的待决清单（内部计划），不在公开仓库。已经定下的结论写在正文各节。

## 11. 图

图在 `docs/local-aggregator/`，用制图纸样式：一种墨，细线，直角，连线是直线、最多一个弯；颜色只用来表示状态。

1. `programs.svg`：三个程序；本机 `droppen` 用一个身份同时运行 AI 与控制其它设备。
2. `signin.svg`：在本机设置里登录（弹窗、注册码交接、进程内注册、上链与交钥）。
3. `gateway.svg`：数据怎么走，以及同一份 SPA 的三种外壳。
