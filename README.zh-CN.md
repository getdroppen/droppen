# Droppen

[English](README.md) | 简体中文

Droppen —— 远程浏览与管理本机的 Claude Code / Codex 会话。

> Droppen 原名 remote-hand。产品改了名，有些内部名字没改：可执行文件 `remote-hand-server`（`droppen` 是同一个程序的另一个名字）、数据目录 `~/.local/share/remote-hand` 与 `~/.config/remote-hand`、`APP_*` 环境变量都照旧，下文的命令请原样输入。服务端自己的环境变量已改叫 `DROPPEN_*`；原来的 `RH_*` / `REMOTE_HAND_*` 仍然认，用到时提示一次：加上同值的新名字；还可能回滚到改名前的版本时，旧名字也留着。

## 功能

- **插件**：Droppen 的核心不带任何插件；插件在构建时由「发行版清单」组合进来，按实例在「设置 › 插件」里开关（默认关闭，配置 `[plugins] enable/disable` 可强制），页面在 `/p/<插件 id>/`，数据在插件自己的文档存储 `instances/<实例>/plugins/<id>/store.db`。开源构建用仓库根目录的 `distribution.json`（不含插件）；私有构建用 `DROPPEN_DISTRIBUTION=<清单路径>` 指向仓库外的清单。SDK 是 `packages/plugin-sdk`（`@droppen/plugin-sdk`）与 `server-rs/crates/droppen-plugin-sdk`，MIT OR Apache-2.0。见 [插件与发行版](docs/plugins/distribution.md)、[文档存储协议](docs/plugins/storage-protocol.md)。

- **会话置顶**：列表右键菜单或会话详情标题栏可置顶／取消置顶。置顶会话显示在整个列表最上方（目录分组之前），仍遵守搜索与来源筛选，置顶项内部沿用原列表顺序。状态保存在实例自己的数据库，通过实时 feed 同步到手机和电脑；不修改 CLI 的会话文件。

- **Sessions**：支持来源过滤、标题 / 路径搜索、单个会话重命名（列表右键或详情页右上角）；transcript 归一化渲染（user / agent / thinking / tool call / tool result，工具明细可折叠）。可直接在网页里发消息驱动 agent。列表行、文件夹标题、分屏窗格边框、屏幕图标和浏览器标签页都会标出哪些会话在工作（含本轮已运行时长）、排队、后台运行或在问你（agent 用 `AskUserQuestion` 提问、还没人回答时才算；一轮正常结束的会话不算，只看未读数；codex 不会进入这个状态），以及每个会话有几条未读的 agent 消息（服务端计数，上限 99+）。打开会话、切回这个标签页 / 应用、聚焦分屏窗格或点开关于它的通知时若有未读，会出现「↑ N 条未读 · 跳到第一条」按钮：这次进入时发出的已读上报会带回它覆盖掉的**上一次**已读时间（`POST …/seen` 的 `previousReadAt`），按它找到第一条未读的 agent 消息（`GET …/first-unread?after=`，扫整份 transcript，超过 99 条也照样找到最早那条、数字是准确的），必要时自动加载更早的历史并高亮它；跳转中显示进度、可以停止，到不了会提示；点过、自己滚到那条或滚过它之后按钮消失。
  - **归属（owned / external）**：一个 server 的会话列表 = **它自己创建的会话**。主要看目录：账号会话落在本实例私有的 config root（`~/.local/share/remote-hand/instances/<name>/claude`）即为 owned。目录答不上来的两种情况靠**归属登记**（`instances/<name>/owned-sessions.json`，创建时写入）：codex（只有一个 `~/.codex`，没法隔离），以及**用本机登录态新建的 claude 会话**（`accountId: null`、或没设默认账号的 instant chat——它本来就该写进你自己的 `~/.claude`）。其余 `~/.claude` 里的会话（你在终端里自己跑的、另一个 Droppen 服务端建的）一律是 external。
  - 登记只决定**谁能续写**，不改变**用哪个登录态续写**：登记为 owned 的本机会话 resume 时依旧不带 `CLAUDE_CONFIG_DIR`，用的还是你自己的 `~/.claude`。登记是每实例的，所以实例 B 只会在 external 里看到 A 用本机登录态建的会话。
  - External 会话**可读不可驱动**：详情、transcript、websocket 全部照常，发消息、改名、归档都返回 `409 {"error":"external_session"}`（带 `hint`）。要继续这样的会话，回到启动它的地方（终端里的 `claude --resume`、另一个实例）。早期版本有「接管已有会话」（adopt，把 transcript 复制进本实例的 root），现已移除；当时接管过的会话就是本实例 root 里的普通会话，照常列出、打开、发消息、续写和归档。
  - 给一个本机登录态的会话**绑账号**会返回 `409 local_session_cannot_use_account`（带 `hint`）：账号 spawn 必须跑在本实例的 root 下，而把 transcript 搬过去会分叉对话。要换账号，用**「换个账号继续」**（`POST /api/sessions/claude/{id}/continue`，`{accountId}`）：把这段对话复制进本实例 root、换一个**新的会话 id**（每行的 `sessionId` 改成新 id，其余原样），绑定到该账号，下一条消息就在那个账号上 `--resume` 接着聊；原会话和 `~/.claude` 里的原文件一个字节都不动。只限本实例自己用本机登录态建的会话（external 返回 `409 external_session`，已在本实例 root 的返回 `409 session_not_on_local_login`）。会话头部的「本地」徽章和发送失败提示里都有这个入口。
  - **本机登录态的状态**：`GET /api/accounts` 带 `local.claude` / `local.codex`（`state: signed_in | signed_out | unknown` 和本机登录的 `rest`）。证据按时间取最新：本机登录上某轮因凭据被拒（`auth` 休息）、或 `/usage` 探针答「未登录」算 `signed_out`；之后只要本机登录又读到一次额度窗口（重新 `/login` 后跑一轮）就回到 `signed_in`，不需要手动清除。Codex 没有额度读数可用，改为本机登录上有一轮成功跑完就结束它的 `auth` 休息——有顶替账号时没指定账号的启动不会跑在本机登录上，所以 `codex login` 之后要在本地登录上跑一次聊天或会话，才会切回来。没指定账号的启动（新建会话、即时聊天、定时任务）都走服务端同一条规则（`accounts::rotation::start_for`）：有默认账号用默认账号（Claude 的默认账号在休息或快用完时照旧换到池里另一个账号）；在设置里**选了**「本地登录」作默认（`PUT /api/accounts/default` 传 `null`，记为一个选择，`local.<cli>.isDefault`）就一直用本地登录，哪怕它未登录；选「自动」（`automatic: true`，清除默认，新装时就是这样）时用本机登录，但本机登录是 `signed_out` 时改用**参与自动轮换**、没在休息的账号（Claude 按所请求模型的系列判断休息范围；有最近额度读数的按余量最多的优先，没有读数的按列表顺序）——不在轮换池里的账号（env 网关、API key，以及默认不进池的 Codex 账号）不会被拿来顶替；池里没有空闲账号时仍是本机登录，第一轮会报凭据错误并说明怎么重新登录。换到顶替账号时服务端日志会记一行 from/to/reason。`GET /api/accounts/start?provider=&model=` 返回这条规则此刻的答案（`accountId`、`reason`），新建会话、即时聊天和定时任务表单显示的就是它；没存过模型时 composer 的默认模型是该账号列表的第一个，而模型的系列又可能决定账号，所以这时用 `chooseModel=true` 让服务端把账号和模型一起选（按启动顺序逐个账号、在它自己的列表里找第一个按规则会落回它本身的模型），返回的 `model` 就是要发的模型；页面不指定账号、发这个模型，服务端按同一条规则放回同一个账号，画面上的账号和模型与实际运行的一致。找不到这样的组合时 `placed: false`，页面就直接指定该账号。答案到达之前，没指定账号的启动会等它（新建会话先挂起、即时聊天先禁用发送），按下发送时还会再问一次，答案变了就先显示新的再发；最多再问 3 次、最多等 8 秒，之后不指定账号直接发出，由服务端在启动时放置。等待期间切换 CLI（或设备）会取消这次发送。服务端为 `chooseModel` 询问各账号的模型列表时一次最多并发 3 个，找到合适的组合就不再继续问。新建会话和即时聊天里本机登录仍可手动选（`POST /api/chats/new` 也接受 `accountId`，`null` 即本机登录），显示为「本地登录（未登录）」并提示在这台设备的终端运行 `claude` 再 `/login`；定时任务表单会写明这期间它跑在哪个账号上，没有可顶替的账号时也会说明。设置页里没选默认、本机登录又未登录时，本地登录那一项不再显示为已选，并说明此刻没指定账号的新会话会在哪个账号上启动。另外，本实例 root 里没绑账号的会话（用的是 root 自己的登录）撞到的限额不再记在 `local:claude` 名下，免得把你自己的 `~/.claude` 误判成未登录。
  - `GET /api/sessions` 的 `scope=owned|external|all` 选择看哪一半，**默认 `owned`**；全局 feed 仍是一条流，每行自带 `scope`。
  - **worktree 分组**：每行带 `repoKey`（该 cwd 的 `git rev-parse --git-common-dir` 规范化路径），同一个仓库的多个 worktree 共享一个 key，客户端可以把它们收进一个标题下；不在 git 里则为 `null`。纯展示用，不影响任何 server 能看见什么。新建会话的目录候选（`recent-cwds`）按 `repoKey` 去重，每个仓库只留最近开工的那个 worktree。
  - **附件**：支持上传**任意类型**文件（点纸夹图标或直接粘贴）——图片显示缩略图，其他文件（md / csv 等）显示为可下载的文件卡片；Claude 用 Read 工具读取，Codex 图片走 App Server 的 `localImage`、其余在 prompt 中给出路径。
  - **Codex App Server**：每个受管会话持有常驻 `codex app-server`，通过原生 `turn/start`、`turn/steer`、`turn/interrupt` 驱动。忙时消息进入服务器持久队列，⚡ 将消息递送到当前回合，不再以终止进程模拟 steer。服务器重启后通过 session shim 重连原进程并回放确认；旧版 `codex exec` 会运行至结束，下一次发送才迁移。未获确认的递送不会自动重发；CLI 异常退出后保留队列，需查看 transcript 后再撤回或重发。使用当前 Codex CLI（已验证 0.153.4）；App Server 仍是上游持续演进的接口。
  - **幂等发送**：每条消息在创建时生成一个 `clientMessageId`（UUID），和消息一起存在本机待发队列里，每次重试都带同一个 id；服务端在入队**之前**按 id 占位（本实例 `state.db` 的 `client_messages`，保留 7 天），重复的 id 只回放第一次的结果（`duplicate: true`）、不再入队，`/api/sessions/new` 重试拿到的是同一个会话；被 4xx 拒绝的尝试不记录。每条外发消息标四种状态：**已保存**（只在本机，还没发出）、**已送达**（服务端已收到，排队中）、**已执行**（已交给 agent）、**未知**（发出去了但没收到回应）。网络错误、超时或认不出的 5xx 一律记为未知，**不会被自动重发**，它后面的消息也先不发；恢复连接后客户端用 `GET /api/sessions/{source}/{id}/receipts?ids=` 问服务端：收到过就采用服务端的状态，从没见过就标「未送达，可安全重发」，一键重发沿用同一个 id。会话 `state` 帧和 `SessionDetail` 带最近两小时（最多 100 条）的 `receipts`。claude 与 codex 都支持。
  - **agent 提问（`AskUserQuestion`）**：受管的 claude 会话现在能真正向你提问并等你回答。常驻进程带 `--permission-prompt-tool stdio` 启动，模型调用 `AskUserQuestion` 时 CLI 把它作为 `control_request` 发给服务端、整轮停住等答案（没有超时）；服务端把问题记进本实例 `state.db` 的 `session_questions`（按 CLI 的 `request_id`），会话行与窗格立刻标成「在问你」（即使这一轮仍在 working），并推一条通知（每个问题只推一次，Bark 走 time-sensitive）。transcript 里的问题卡片变成可交互：单选点一下就提交，多选勾完再提交，每题都能填「其他」自由文本，也可以「让 agent 自己决定」（告诉它你不回答、按它的判断继续）。回答是对数据库的一次 compare-and-set（`pending → answered`），**先**落库再写进 CLI 的 stdin，所以两台设备同时回答时后到的一方拿到 `409 question_settled`，卡片显示谁、何时已回答。问题挂着时在输入框里发的普通消息会**直接作为这道题的回答**（`response` 自由文本）送进去，而不是排在被卡住的那一轮后面；输入框上方会写明这一点。接口：`POST /api/sessions/{source}/{id}/questions/{requestId}/answer`（`{answers: {题目原文: 字符串 | 字符串数组}, notes?, response?}`，多选数组由服务端用 `", "` 拼接、`questions` 原样回传给 CLI）、`…/dismiss`、`GET …/questions`；会话 `state` 帧和 `SessionDetail` 都带 `questions`。问题挂着期间：卡顿看门狗、「transcript 说已结束」的幽灵轮次判定、空闲退役、设置变更重启和提前换号都会让路；点停止会把问题标成已取消。进程没了（崩溃、换号重启、空闲退役……）问题就**过期**——`--resume` 不会再问一遍——但仍可回答，答案会作为一条普通消息（「Answer to your question "…": …」）发给会话。服务端重启不影响：进程在 session shim 后面活着，重启后回放 stdout 重建挂着的问题（库里已答、但答案还没写进 stdin 的，会把存下的答案再写一遍，CLI 会忽略重复的 `request_id`）。已在跑的常驻进程不会为此被强制重启，等它下一次因为别的原因重启（换设置、换号、崩溃、空闲退役）时才带上新参数。codex 不受影响（它以 `approvalPolicy: never` 运行，不会提问）。
  - **下载按钮**：agent 回复里提到的、真实存在于 `$HOME` 内的文件，会在消息底部生成一键下载卡片，省去去文件列表里翻找。
  - **断线续传**：会话 websocket 断线重连时不再整页重发。每个 transcript 帧都带 `endIndex` 和 `prefixFp`（服务器对前 `endIndex - 3` 条的指纹），客户端重连时带上 `?since=<endIndex>&fp=<prefixFp>`；指纹仍然对得上、落后不超过 2000 条（且不超过 4 MiB）就只补一个从 `since - 3` 开始的 `delta`（末尾 3 条重发，覆盖事后被改写的条目），已加载的更早历史和滚动位置都保留，否则才回到整页 `snapshot`。判断只依赖文件本身，服务器重启后照样能续。实测 1000 条的会话：重连从约 108 KB 降到约 3 KB（无新内容）/ 7 KB（8 条新内容）。「加载更早」也改为只校验 `before` 之前的前缀，会话在增长时不再失败。
  - **思考计数（只追加的 transcript）**：claude 写完一个块（thinking、正文、工具调用）要等整个 API 响应结束才写进 transcript，思考中更是什么都不写。常驻进程跟着 stdout 的 `system:thinking_tokens` 记下正在进行的 thinking 的 token 数，经会话 websocket 的 `draft` 帧推给页面（约 80 ms 节流，接入时随首帧补发；帧里只有这一个计数块，思考结束就是空帧，codex 会话没有）。输入框上方的状态条把「最近活动」换成「思考中… ~50 tokens」（「28 条消息 · 思考中… ~50 tokens · 7m14s」，计数在固定宽度的槽里右对齐，手机上省掉「tokens」字样），思考结束后「最近活动」从计数最后一次变化算起。transcript 只追加已经写入的条目，不再在最后一条消息下方画临时气泡——那个气泡一轮里反复出现、变高、变矮、消失，整个信息流跟着跳；代价是大文件 `Write` 之前说的那句话要等 transcript 写入才出现。计数不进 transcript 窗口，不算未读、搜不到、不影响断线续传。旧版页面收到新帧只画一行「思考中…」，新版页面收到旧版 agent 带正文和工具调用的帧只读其中的计数。一轮开始和结束也会立即推送 `state` 帧，不再等 2 秒轮询。
  - **弱网下的会话切换**：会话 feed 不再各开一条 websocket，而是挂在页面本来就有的 `/api/events` 上（`?v=2`：先收到 `hello`，再用 `sub`/`unsub`/`resync` 按订阅号收发，帧带 `"sub":N`；`ping`/`pong` 识别看似还开着、其实已断的连接），切到新会话不用再握手、不用再换 ticket；断线重连时所有订阅在一条消息里各自带断点续传。`&z=1` 时 ≥1 KB 的帧以 raw deflate 二进制帧发送（经 tunnel 的 websocket 没有任何压缩），新会话首屏只发最新 100 条，更早的照旧按页加载。最近看过的 8 个会话窗口留在内存里，切回去立即显示、只补差量，其中最近 3 个保持订阅；最近 5 个窗口和会话列表另存一份在本机 IndexedDB，冷启动先画这份副本（标明「正在显示本机保存的副本」）再从它续传，登出时清空。前端：第三方库单独分包，设置、统计、终端按需加载；service worker 预缓存当前版本的启动资源，服务端报告新版本时提前在后台拉好，点「重新加载」不用再等；`index.html` 和图标带 ETag（未变就 304），带 `?v=` 的图标长期缓存。旧页面继续走 `/api/sessions/{source}/{id}/ws`，新页面遇到不认识 `v=2` 的旧服务端会自动退回每会话一条 socket。实测（400 ms RTT、800 kbps）：首次打开一个会话约 5 s → 1.1 s（330 KB → 52 KB）、打开含 3 个会话的分屏 6.6 s → 1.2 s，刷新页面 5.9 s → 1.4 s 出内容，发版后刷新 7.7 s → 1.4–2.0 s。
  - **会话内搜索**：会话页按 Cmd+F（macOS）/ Ctrl+F，或点标题栏的放大镜，打开搜索条。搜的是**整个** transcript（`GET /api/sessions/{source}/{id}/search?q=&limit=`），不只是页面里已加载的那几百条：空格分隔的词须同时出现在同一条里，不区分大小写，中日文按子串匹配；覆盖消息正文、thinking、工具名与调用摘要、工具输入和输出。计数从最新的一条数起，Enter 跳到更早的一条、Shift+Enter 跳回更新的一条，也可以从下拉列表点选；目标还没加载时沿用「加载更早」逐页补齐（可取消），自动展开所在的折叠步骤并高亮命中。会话在跑时新消息不会把你拉走，再按一次 Enter 或改一下关键词才刷新计数。搜索框已有焦点时再按一次 Cmd+F 交还给浏览器自带的查找；分屏时作用于最近操作过的那个会话窗格。external 会话同样可搜。
- **Screens**：sidebar 可新建持久化的分屏工作区，把 session / 终端 / 聊天任意拼在一起（Cmd+D 左右分屏、Cmd+Shift+D 上下分屏）。布局存在服务端（`~/.local/share/remote-hand/instances/<instance>/screens.json`，每个实例各存各的：布局里引用的终端 id 只有本实例认得），切走、刷新、换浏览器都不丢；已打开的屏幕在会话内保持挂载，终端滚动位置和连接切回来原样还在。同一个屏幕在手机和电脑上同时打开时，一边的改动会实时推到另一边；两边同时改，后保存的一方不会悄悄覆盖先保存的，而是同步到最新并提示一句，保存失败会在屏幕标签上标「未保存」并自动重试。删除屏幕只删布局，不会关闭里面引用的资源。
- **定时任务（Scheduled tasks）**：给一条 cron 表达式、一个目录和一句 prompt，到点自动**新开一个会话**跑它。设置页里配置，存在 `~/.local/share/remote-hand/instances/<instance>/scheduled-tasks.json`（mode 0600，每个实例各存各的——共用一份会让机器上每个 server 都触发一遍，一次到点跑出两个会话）。
  - **表达式是五段**（分 时 日 月 周），**按服务器本地时区解释**：`0 9 * * *` 就是服务器所在时区的早上九点。若一律按 UTC 算，东八区用户的「早九点」会跑在下午五点。不引 `chrono-tz`、不做 per-task 时区：表单上多一个字段、二进制里多一份时区库，换来的只是「人和自己的服务器不在同一个时区」这个对本地 CLI 工具来说并不常见的场景。带秒的六段表达式会被**拒绝**而不是被当成秒来解释——每次触发都是一个真实的 CLI 进程和真实的 token。夏令时不做特殊处理，交给 croner 按 wall clock 解析：跳过的那一小时里没有触发点，重复的那一小时只匹配一次，和 `cron` 本身的行为一致。
  - **错过的不补跑**。服务器起来时会按**当前时间**重算每个任务的下次触发，文件里存的旧时间直接丢弃；笔记本睡了一觉醒来，超过 5 分钟没人管的触发点记一笔 `skippedMissed` 而不是补一发。理由是同一件事说两遍：早上九点的「过一遍昨晚的改动」在下午四点跑出来，问的已经是一个别人替你过完了的昨晚；停机一夜之后一口气补上一打，就是一打 CLI 同时开跑、同时烧额度。
  - **只做「新建会话」**，不往存量会话里注入。往存量会话发消息要过 `sendability`、要会话仍可写、要么够到 resident 要么按指纹重开一个进程——这些都是当年那颗按钮背后**有人在看**着做的判断，无人值守的定时器猜错了没人能纠正它。新建没有这些前提：会话还不存在，也就没有什么状态可以是错的。
  - **claude 和 codex 都能跑**，任务上存 `source`。两边的 model / effort / permission 取值不同，**创建和编辑时就按所选 CLI 校验**（claude 是五档 effort 且按模型收窄，codex 的档位读它自己写的 `models_cache.json`；这台机器从没跑过 codex 就不拦，免得挡住新模型的新档位）。校验放在保存那一刻，因为凌晨三点发现 `cwd` 打错了的定时器没人可以告诉。注意 `rh-notify` skill 是 Claude Code 的机制，只注入 claude 会话——定时开出来的 codex 会话里没有这个工具，但**调度器自己**的失败通知与 CLI 无关，照发。
  - **`skipIfBusy`（默认开）**：到点时如果上一轮开出来的会话还在干活（claude 看 resident 的 phase 和队列长度，codex 看那一轮进程是否还活着），这一次就跳过并记 `skippedBusy`。Droppen 的队列从不拒绝消息，只是等，所以没有这道闸的话，一个每半小时触发、每次要跑四十分钟的任务会安静地堆出一条只增不减的 backlog。
  - **失败可见**：任务行上记 `lastRunAt` / `lastStatus` / `lastError` / `lastSessionId`。**调度器级**的失败（连 CLI 都没起来，因而不存在任何一个 turn 会去报告它）额外推一条 Bark 通知——走的是 `notify` 那条配置好的通道，自带 10 分钟 3 条的限流（它绕过了 `POST /api/notify` 的限流，所以得自己带一个）。turn 级的失败本来就由 push 的回合结束通知覆盖，不重复。
- **Files**：浏览 home 目录下的文件系统，支持路径输入/面包屑、前后导航、当前目录筛选、排序和隐藏文件切换；可预览文本（上限 256 KiB）与图片、上传/下载文件。普通 Tab、临时分屏与保存的屏幕共用文件管理，每个分屏独立保存目录和预览位置；窄分屏及手机可一键返回列表。不提供文件编辑或删除操作。
- **鉴权**：本机账号 + bearer token（`remote-hand-server user add`），账号**按实例**存放。**某个实例没有账号时它的 API 依然全开**——升级兼容，第一条 `user add` 才关上那个服务端的门；新实例会一次性继承本机原有的账号，不会凭空敞开。详见[运维 › 鉴权](#鉴权)。
- **Settings**：服务器与数据源信息、会话计数、健康状态。
- **Accounts**：注册多个账号，**claude 和 codex 各自一套**，新建会话时选择用哪个账号跑，会话头部可随时切换；composer 旁的配额 popover 按账号展示 5h / 周 / 按模型的剩余额度。
  - **两种模式**。一种是**OAuth 授权**（`kind: oauth_token`，仅 claude）：`claude setup-token` 向导跑一遍，拿到的 token 存起来。另一种是**本地 env 模式**（`kind: env`）：你自己写一段环境变量，spawn 时原样注入子进程。两种模式共用一个账号列表、一套绑定和一套切换 UI，区别只在凭据是怎么来的。
  - **env 模式就是一个多行输入框**，每行一条 `KEY=VALUE`，没有「key 字段 + base URL 字段」那套猜测——因为那套猜的永远是别人的配置：网关要一个 token 加一个 base URL，Bedrock 要四个变量而且根本没有 token，中转站的 key 叫一个谁也想不到的名字。它们唯一的共同点是「这些变量，放进子进程的环境里」，所以存的就是这个。claude 常见写法：

    ```
    # 官方 API key
    ANTHROPIC_API_KEY=sk-ant-…

    # 或者自建网关签发的 bearer token
    ANTHROPIC_AUTH_TOKEN=…
    ANTHROPIC_BASE_URL=https://gw.example.com/v1
    ```

    解析规则刻意做得很窄（就是一个 `.env` 的子集，没有插值、没有转义、没有多行值）：空行和 `#` 开头跳过；可以带 `export ` 前缀；按第一个 `=` 切分；KEY 必须匹配 `^[A-Za-z_][A-Za-z0-9_]*$`；VALUE 两端的成对 `"` 或 `'` 会被去掉（里面的内容一个字符都不改），允许为空。**读不懂就报错，绝不跳过**——不是注释、不是空行又不是键值对的行会带着行号 400，重复的 KEY 也是 400：静默丢一行等于存进一个缺了变量的块，而你以为设过了。上限 64 个变量 / 8 KiB，空块不收。
  - **不许设的变量**：`CLAUDE_CONFIG_DIR`、`CODEX_HOME`、`HOME`、`PATH`，以及 `NODE_OPTIONS`、`LD_PRELOAD`、`LD_LIBRARY_PATH`、`DYLD_INSERT_LIBRARIES`、`DYLD_LIBRARY_PATH`、`DYLD_FRAMEWORK_PATH`（400 并点名）。前四个是「我的 token 没生效」和「Droppen 再也找不到我的会话了」之间的区别——前两个决定 transcript 落在哪里，后两个一次搬走所有东西。后六个是因为账号在整台机器上共用：一个账号块能让加载器往 CLI 进程里塞代码，那它就会在**别的实例**绑到这个账号的每个会话里跑起来。
  - **codex 的块会被翻译**。`codex app-server` 不读 `OPENAI_API_KEY`，也不读 `CODEX_API_KEY`，它只认 `CODEX_HOME/auth.json`；而搬 `CODEX_HOME` 会把 `sessions/`、`state_5.sqlite` 一起搬走，Droppen 就再也找不到你的会话了。唯一既能隔离凭据、又让 `~/.codex` 原地不动的办法是**自定义 model provider**：所以 codex 账号的块除了照常整块注入之外，还会取 `OPENAI_API_KEY`（没有就取 `CODEX_API_KEY`）的值塞进 `DROPPEN_CODEX_API_KEY`，并加上 `-c 'model_providers.remote_hand={…,env_key="DROPPEN_CODEX_API_KEY",…}' -c model_provider=remote_hand`；块里有 `OPENAI_BASE_URL` 就用它当 provider 的 `base_url`，否则用 `https://api.openai.com/v1`。两个 key 变量一个都没有 → 400 并说明 codex 需要 `OPENAI_API_KEY`（或 `CODEX_API_KEY`）。`base_url` 必须是 `http(s)://<host>…`、不许空 host / 空格 / 引号 / 反斜杠——它会被插进行内 TOML 里，能闭合字符串的字符就是一条注入路径。实测这个 provider 压得过 `auth.json` 里已有的登录态，因此账号选择是确定的，不会被机器上的既有登录抢走。
  - codex 的注入 provider **写死 `wire_api = "responses"`**。只支持 chat-completions 的中转站因此用不了——症状是添加账号时显示「已保存，但未能验证」，而不是一条看得懂的配置错误；换一个支持 Responses API 的端点，或者就用官方 `base_url`。
  - **值只进不出**。API 只回 `envKeys`（变量名，按你写的顺序），从不回值——包括回给写它的人：想读回一个已存的值，唯一的用途就是把它读到它不该在的地方。改块是**整块替换**：文本里没有的变量就是没设。改块和换 token 一样要重新跑一次真实验证——一个块里任何一条变量都可能是让它失效的那条。
  - **老账号自动转换**。v0.2.58 的 `api_key` / `auth_token` 账号在读取时就变成它一直以来的简写形式：claude 是 `ANTHROPIC_API_KEY=…` 或 `ANTHROPIC_AUTH_TOKEN=…`（设过 base URL 再加一条 `ANTHROPIC_BASE_URL=…`），codex 是 `OPENAI_API_KEY=…`（+ `OPENAI_BASE_URL=…`）。升级前能用的账号升级后照常能用，下一次写入时新形状落盘。
  - **每个 CLI 各自的默认账号**（`defaultAccountId` / `defaultCodexAccountId`）：新会话跟自己那一套走，另一套动不了它。一个 CLI 没配默认账号、跑在本机登录上时，它的 spawn 与从前逐字节相同——不多一个参数，不多一个环境变量；只有设成「自动」、本机登录被判定为未登录、池里又有空闲账号时，没指定账号的启动才会改在那个账号上（见上文「本机登录态的状态」）。
  - **codex 账号没有额度、没有 usage login**：配额面板和「启用额度」都是 `claude auth login` 那条链路上的东西，codex 账号在这些入口一律 409 并说明原因，行为与 claude 的 env 账号一致（只显示标签）。
  - 一个活着的 `codex app-server` 就是它启动时那个账号，socket 上换不了 key。所以**切账号 = 换进程**：空闲时优雅关掉再以 `thread/resume` 起一个新的，正在跑 turn 时拒绝（和「已有消息在跑」同一个回答），不砍你正在看的那一轮。
  - 账号会话运行在**本实例私有**的 Claude config 目录（`~/.local/share/remote-hand/instances/<name>/claude`；token 只在 spawn 时以环境变量注入；该目录里若有 `.credentials.json`（这个 root 自己的登录态）只记一条警告、不再拒绝——实测 claude 2.1.266 环境变量凭据压过文件凭据，`CLAUDE_CODE_OAUTH_TOKEN` / `ANTHROPIC_AUTH_TOKEN` 都排在文件之前，`ANTHROPIC_API_KEY` 在 `-p` 下必用）。没选账号的 owned 会话同样跑在这个 root 下，只是不注入 token，用这个 root 自己的登录态。
  - 老版本共用的 `~/.local/share/remote-hand/claude` 会在 `default` 实例启动时**自动搬进** `instances/default/claude`（只搬一次，其他实例名不碰；两边都存在时都不动并告警）。
  - **setup-token 向导不会替你打开浏览器**（服务器上弹出的窗口，手机端的你也看不见）：向导把 CLI 打印的授权 URL 以可复制文本呈现，你自己去授权，再把拿到的 code 粘回输入框，服务端替你敲进 pty。向导的 pty 固定跑在 400 列且不接受前端 resize，避免 CLI 按终端宽度硬折行把 token 截断。
  - **re-auth / 编辑 env**：账号列表每行都有操作：OAuth 账号是「重新授权」，可以重跑 setup-token 向导；env 账号是「编辑」，打开时通过 `GET /api/accounts/{id}/env` 把已存的整段环境变量读回来填进编辑框，改一行不用整段重打，保存时整段替换，**原地更新凭据**——account id 不变，因此 `sessionAccounts` 里绑到它的会话可以直接继续跑（额度用完换号的主场景）。列表本身仍然只带变量名（`envKeys`），不带值。
  - 新增和 re-auth 存库前都会用该凭据跑一次真实的最小请求验证（claude 是隔离 config 目录里的 `claude -p`，codex 是隔离 `CODEX_HOME` 里的 `codex exec`，都不碰你自己的 `~/.claude` / `~/.codex`）；被 Anthropic 拒绝的 token 不入库，错误原文（脱敏后）返回给 UI。探测本身跑不起来（没装 CLI、超时、429、CLI 自己崩了）不算拒绝：凭据照存，但 UI 会明确显示「已保存，但未能验证」，不会假装验过。
  - 唯一的例外是**折行导致一条 token 有两种读法**时：这种情况下必须由 Anthropic 裁决，探测跑不起来就整个失败、什么都不存（宁可让你重跑向导，也不赌一个会 401 的账号）。
  - **额度读数来自会话本身**：claude 每轮之前写的 `rate_limit_event`（`allowed` 也算，不只是被拒的那条）都带 `unifiedWindows`，驱动把每一条都当成该会话所用登录的一次读数——绑了账号就记到那个账号，跑在你自己的 `~/.claude` 上就记到本机登录，托管 root 里没选账号的会话（用的是那个 root 自己的登录态，不是任何一个已知登录）不记。窗口按标签与 `/usage` 的读数合并：`five_hour` → `5h`、`seven_day` → `weekly`、`seven_day_opus` → `weekly (Opus)`、`seven_day_sonnet` → `weekly (Sonnet only)`（与 `/usage` 的字样一致），`seven_day_overage_included` → `weekly (Fable)`——这一条是推断，依据只有一次实测（那次命中的文案是 "You've reached your Fable limit"）。每个窗口带自己的读取时间（`updatedAt`）和来源（`session` / `probe`），新读数只替换它带的那几个窗口。读数存在机器级的共享文件里（账号是 `~/.local/share/remote-hand/accounts/<id>/usage.json`，本机登录是 `~/.local/share/remote-hand/local-usage.json`；加锁读改写、写临时文件再 rename），同一台机器上的每个实例都读它；同一登录的会话读数最多每 15 秒落盘一次。没开探测的账号（包括 env 账号）只要有会话跑过，也显示会话读到的窗口。
  - **`/usage` 探测只是兜底**：只有当某个登录最新的窗口超过 15 分钟没更新（`DROPPEN_USAGE_FALLBACK_MS`，也就是这段时间没有会话在它上面跑过）、并且最近 30 分钟内没有探测失败（未登录 / 探测失败 / 429）时，才会在「启用额度」那份登录（`claude auth login`，落在每账号隔离目录；setup-token 的 scope 只有 `user:inference`，自己查不了 `/usage`）上跑一次 `claude -p /usage`。持有实例锁的实例每 60 秒扫一遍，打开配额面板时按同样的规则触发。**整台机器同一时刻只有一个进程能探测同一个登录**：`<usage 目录>/.rh-probe.lock` 上的非阻塞 `flock`，抢不到就跳过，对方的结果会落进共享文件（模型列表探测也拿同一把锁）。原因是 usage login 的 refresh token 会轮换：两个实例各自探测时可能拿同一个 refresh token 各刷新一次，第二次被当成重放，整个登录被吊销——之前两个 usage login 就是这样死掉的；而产生 `rate_limit_event` 的 setup-token 不刷新，一直可用。env 模式账号没有订阅额度，没有会话读数时只显示标签。
  - **限额日志与休息状态（第一阶段：记录）**：claude 一轮的 `result` 报错（含 CLI 标成 success 的 `You've hit your session limit · resets 2:30am (Asia/Tokyo)`）、被拒的 `rate_limit_event`、因鉴权失败退出的进程，以及 codex 失败的 turn 和满额的 `account/rateLimits/updated`，都会被分类成 `session` / `weekly` / `rate` / `credit` / `auth` / `other`，原文脱敏后截到 500 字记进本实例 `state.db` 的 `account_limit_events`（最多留 500 条），并给该账号（本机登录记为 `local:claude` / `local:codex`）写一条 `account_rest`：有重置时间就休到那时，没有就按类别默认（session 30 分钟、weekly 60 分钟、rate 1 分钟、credit 30 分钟、auth 直到手动清除或换凭据、other 连续出现才休并从 1 分钟翻倍到 10 分钟封顶）。上下文超长一类错误先被排除，不算限额。休息范围默认是整个账号（`all`，含普通的 weekly limit——它就是所有模型共用的 `seven_day` 窗口），只有文案或事件点名了模型家族（`Opus weekly limit`、`You've reached your Fable limit`、`seven_day_opus`）才按家族记；`seven_day_overage_included` 这类没写明覆盖哪些模型的周窗口只记事件（scope `unknown`）、不单独休息，由同一会话 10 秒内紧随其后的 `result` 定范围、借用它的重置时间。同一会话 10 秒内同类同范围的重复上报（CLI 一次被拒会先发 `rate_limit_event` 再发 `result`）只记一条，休息取更晚的结束时间。`GET /api/accounts` 的每个账号带 `rest`（仅生效中的），`GET /api/accounts/limit-events`、`GET /api/accounts/{id}/limit-events` 看日志，`DELETE /api/accounts/{id}/rest[?scope=]` 手动清除；重新授权会清掉 `auth` 休息。这些记录本身不重试、不动队列；换号由下一条（第二阶段）决定。休息状态按实例各记各的，不跨实例共享。
  - **自动轮换（第二阶段）**：claude 会话**自己的一轮**以 `session` / `weekly` / `credit` / `auth` 类限额结束时（`rate` / `other` 不换；子任务失败不算），driver 先扣住队列——不再把排队消息喂给刚拒绝的账号——再决定换到哪个号。**轮换池**是账号上的 `rotation` 开关（`POST/PATCH /api/accounts` 可设）：claude OAuth 账号默认在池里，env 账号和所有 codex 账号默认不在；只在池内账号之间换，**codex 会话和本机登录的会话不换**。若进程还在旧账号上时会话已被手动绑到别的账号，就直接换到那个账号（它也在休息时才回到常规挑选）。候选排除当前账号和休息范围覆盖本会话模型家族（`all` 或 `fable` / `opus` / `sonnet` / `haiku`，先看 `system:init` 报的模型名；认不出家族时，任何按家族记的休息都算覆盖）的账号，30 分钟内有读数且 `5h`、`weekly`（及该家族的周窗口）都低于 98% 的按最紧窗口的剩余排前，没读数或读数过期的按列表顺序其后，≥98% 的垫底。**等待策略**：整号范围的限额（5 小时、普通周额度、余额、凭据）立即切换，不等后台子任务（它们用的是同一份耗尽的凭据）；只限某个模型家族的（如 Fable 周额度）先让后台子任务跑完，沿用改设置时的等待（`DROPPEN_AWAIT_BACKGROUND_MS` 封顶）。切换 = 把会话绑到新账号、排队消息改用新账号的环境、用 `--resume` 在新账号下重开进程，然后不重发原消息（它已在 transcript 里），而是在队首放一条中文续写提示（被中断的后台子任务逐条列出，附输出文件），沿用原消息的模型/effort/权限。同一条消息链最多自动换 3 次；到上限或没有可换的账号时，这一轮照旧 Failed，队列保持扣住（不丢）：扣住期间同一账号再报限额不算新决定、不再推送；每个 watchdog 周期（60 秒）以及有账号的休息被清除、账号新增或重新授权、读数显示有余量时重新挑选，一出现可换的账号就换过去；该账号自己的限额恢复（休息到期或被清除）而没换号时，先在队首放一条同账号的续写提示（`账号 A 的额度已恢复……请从中断处继续`）再放行；有人发新消息或插队也会放行。扣住状态和挂起中的续写提示都写进 run journal，服务端重启（自我更新）后照样扣住 / 在绑定的新账号下重开。每次决定写进本实例 `state.db` 的 `session_rotations`（每会话留 20 条），`SessionDetail.rotation` 和会话 feed 的 state 帧带最新一条，并推送一条不受 60 秒节流的通知（Web Push，以及配置了的 Bark）。新建会话 / 定时任务 / 即时聊天用默认账号时，若默认账号（在池内）正休息且范围覆盖所请求的模型，改用第一个候选。已知限制：已经 steer 进旧进程 stdin 的消息仍在旧账号上跑（管道里的内容收不回来）；一次限额会推送两条通知（先是 Run failed，再是轮换结果）；切换中途失败（新账号的 recipe 或绑定写不进去）按 `no_candidate` 记，会话留在原账号扣住。立即切换时，已插队进旧进程的消息会被放回队列，但旧进程退出前可能还会为它开一轮（同样被拒），于是新进程里会再收到同一条消息（未对照 CLI 在 stdin 关闭时的行为验证）；切换时按旧账号**当前**的 env 块移除变量，若进程启动后从块里删过某个键（如 `ANTHROPIC_BASE_URL`），它的旧值会留在新环境里。
  - **提前轮换**：池内账号的某个相关窗口（`5h`、`weekly`，及会话模型家族自己的周窗口）在 15 分钟内的读数里已用到 `DROPPEN_ROTATE_AHEAD_PERCENT`（默认 95，设 `0` 关闭）且尚未重置时，会话在安静点（一轮正常结束后、或空闲会话开始下一轮前，且没有后台子任务在跑）就换到所有相关窗口都低于阈值 −10 的已知读数账号，下一条消息用 `--resume` 在新账号下跑、不加续写提示，每会话 30 分钟最多一次、手动改绑后 30 分钟内不动，记录为 `SessionRotation.trigger: "ahead"` 并推送「已提前从 A 切换到 B（A 的 5 小时额度已用 96%，23:10 重置）」；新建会话的默认账号快用完时同样改用这样的账号。
  - 账号数据存在机器级的 SQLite 库 `~/.config/remote-hand/state.db`（mode 0600）的 `accounts` / `session_accounts` 表里；老的 `~/.config/remote-hand/accounts.json` 不再被读入（0.2.125 及更新的安装早已把这些数据放进数据库，更老的安装需要重新添加账号）。Droppen 永远不读、不写、不复制 `~/.claude/.credentials.json` 和 `~/.codex/auth.json`——codex 账号靠自定义 provider 的 `env_key` 注入，正是为了不碰后者。

## 架构

- `server-rs/`：Rust + [gotcha](https://crates.io/crates/gotcha)（axum + OpenAPI 派生），监听 `127.0.0.1:8700`，OpenAPI 文档位于 `/openapi.json`。**单个可执行文件**：前端产物在编译期由 rust-embed 打进二进制，运行时不需要任何 Node 依赖。框架填不上的洞（额外状态码、zod 的两种「可缺省」、spec 导出、优雅停机、静态资源）集中在少数几个模块里，见 [`server-rs/README.md`](server-rs/README.md)。同一个二进制还负责**看住自己**：`remote-hand-server up` 是一圈极简的 supervisor loop，起一个 `serve` 子进程守着——干净退出就立刻重新拉起（自我更新正是这样切进新二进制的），反复起不来就退避并逐级回滚，同时用 `waitpid(-1)` 收掉所有被过继过来的孤儿进程。
- `web/`：Vite + React + TanStack Router / Query + Tailwind CSS v4 + Base UI，shadcn 风格紧凑三列布局，开发端口 `5180`（`/api` 与 `/embed` 代理到后端）。
- `openapi/openapi.json`：由后端导出的契约，`openapi-typescript` 生成前端类型，`openapi-fetch` 提供强类型客户端。

## 开发

需要 Node 22 + pnpm 与一套 stable Rust 工具链。

```bash
pnpm install
pnpm dev:server   # cargo run -- serve, http://127.0.0.1:8700
pnpm dev:web      # http://localhost:5180
```

`pnpm dev:server` 首次编译需要几分钟；此时 `web/dist` 通常还不存在，二进制里嵌的是一个
提示「请先构建前端」的占位页，但 API 全部可用，配合 `pnpm dev:web` 的代理即可开发。

`web/dist` 缺失默认会**直接构建失败**——否则 `cargo build --release` 会悄悄产出一个只会
显示占位页的二进制。`pnpm dev:server` / `pnpm gen:api` / `pnpm test` 都已带上
`DROPPEN_ALLOW_MISSING_WEB_DIST=1`，直接跑 `cargo` 时若不需要前端，也请自行加上。

测试与构建：

```bash
pnpm test         # web 的 node:test + cargo test
pnpm typecheck
pnpm build:web    # 必须先于 build:server：SPA 要嵌进二进制
pnpm build:server # cargo build --release
```

`pnpm test` 里的 `cargo test` 是单进程多线程跑的，在 macOS 上随手跑一遍没问题；CI 和提 PR 前要过的是 cargo-nextest
（在 `server-rs/` 里 `cargo nextest run`，每个测试一个进程：这些测试会改进程的环境变量，Linux 上同一进程里多线程跑可能卡住），
具体命令见 [`CONTRIBUTING.md`](CONTRIBUTING.md#run-the-tests-before-you-open-a-pull-request)。

## 契约更新流程

契约的唯一来源是 Rust 服务端的路由与类型定义。后端路由 / schema 变更后：

```bash
pnpm gen:api      # export openapi.json from server-rs + regenerate web/src/api/schema.d.ts
pnpm typecheck
```

`gen:api` 会在 0 号端口上把真实的 router 跑起来、`GET /openapi.json`、再写盘——gotcha 没有
别的途径拿到装配好的 spec，而这么做也顺带保证导出的文件与线上服务的文档逐字节一致。输出经
`serde_json` 重新序列化，key 顺序稳定，改动可 diff。`openapi/openapi.json` 与
`web/src/api/schema.d.ts` 都是生成物，请连同后端改动一起提交。

## 运维

服务端由自己的 supervisor 看护:进程模型、二进制的播种与回滚、自我更新,与部署在哪无关。

`remote-hand-server up` 是一圈只做三件事的 supervisor loop:**重启**(serve 子进程干净退出就
立刻再拉起来,自我更新就靠这一下切进新二进制)、**收孤儿**(终端 shim、嵌入式 app、后台 CLI
都被刻意放进各自的进程组,好活过服务端重启;服务端一退,它们被过继给 supervisor,由它
`waitpid(-1)` 收割)、**转信号**(SIGTERM 只转给 serve 子进程,绝不发给整个进程组,否则那些
「活过重启」的进程会被一起带走)。

supervisor 真正执行的二进制在 `~/.local/share/remote-hand/bin/`:

- 首次启动时把当前调用的那份**播种**过去,此后**永不覆盖**——升级只通过自我更新或手动替换。
  想降级:删掉 `bin/remote-hand-server` 让调用的那份重新播种,或者直接传一个旧二进制上去。
- 升级时旧的留成 `remote-hand-server.prev`,新的用 `rename` 原子换上,不存在「换了一半」的
  中间态。
- 连续 3 次快速失败,supervisor 会把它挪成 `remote-hand-server.bad`(留着排查,不删)换回
  `.prev`;没有 `.prev` 就退回最初调用的那份。退避从 2s 涨到 30s,每降一级都会以 `[rh-up]`
  前缀在日志里大声说出来——「你的升级被回滚了」不该靠版本号自己发现。

### 自我更新

`POST /api/server/self-upgrade` 用 multipart 上传一个新的服务端二进制。它先落在 `bin/` 下的
临时文件里,校验 `sha256`(可选,给了就必须对得上),再让它**自证能在这台机器上跑**——
`--version` 与 `preflight` 都必须退 0,也就是架构不对、传坏了、或读不了当前配置的构建都会在
这一步被拒掉,而此时在跑的仍是老的那个。通过之后才 `rename` 换上,并在**响应发出之后**才优雅
退出,由 supervisor 拉起新的(先回响应再退出,否则客户端分不清「升级成功」和「服务端崩了」)。

```bash
sha256sum ./remote-hand-server | cut -d' ' -f1   # -> <digest>

curl -sS -X POST http://127.0.0.1:8700/api/server/self-upgrade \
  -H "Authorization: Bearer $RH_TOKEN" \
  -F binary=@./remote-hand-server \
  -F sha256=<digest>
```

（`$RH_TOKEN` 见下面的「鉴权」；这台机器上还没有账号时可以省掉那一行。）

响应里的 `restart` 有两种取值:

- `restarting`:这台服务端由 `up` 看着,几百毫秒后自行退出,supervisor 换成新二进制拉起来。
- `manual`:没人看着(例如直接 `remote-hand-server serve`),二进制已经换好了,但得你自己
  重启它才会生效。

换二进制的过程中,**终端、agent 运行、嵌入式 app 都不断**——它们本来就活在各自的进程组里,
只是底下的服务端换了一个。网页这边也不会自己刷新:websocket 重连时发现 `serverVersion` 变了,
只弹一条横幅让你自己点重新加载,免得在「什么都没丢」的那一刻把 composer 草稿和终端回滚缓冲
一起丢掉。版本号形如 `0.1.0+g<commit>`,同一个 crate 版本的两次构建靠这个后缀区分。

### 鉴权

本机账号 + bearer token。账号**按实例**存放在
`~/.local/share/remote-hand/instances/<实例>/users.json`(mode 0600),密码是
argon2id(m=64 MiB, t=3, p=1)的 PHC 串。

早先的设计是全机共用一份账号,理由是「一台机器上的多个实例是同一个人的多个视角,不是多个
租户」。这个前提在把某个实例交给别人的那一刻就不成立了:同一台机器上跑着两个实例,自己用的
`alice` 和开给另一个人的 `bob`,只为 `bob` 加的那一条账号会把**两个**服务端一起关进同一个
密码后面——自己的服务端从此挡在别人的密码后面,而且没有办法给两个人两套登录。所以账号跟着
实例走。

这不是沙箱:两个服务端跑在同一个操作系统账号下,`bob` 的账号照样读得到全部 `$HOME`。它买到
的只是「两个人、两个密码、可以单独吊销其中一个」。真要隔离,请开两个 Unix 账号或两台机器。

```bash
APP_INSTANCE=bob remote-hand-server user add bob     # 或写 --instance bob
remote-hand-server user add ada --instance alice     # 两次输入,不回显;非 tty 时从 stdin 读一行
remote-hand-server user list --instance alice
remote-hand-server user passwd ada --instance alice  # 顺带撤销该用户在**这个**实例上的会话
remote-hand-server user rm ada --instance alice      # 该用户在这个实例上的 token 立即失效
                                                     # 删最后一个用户账号要加 --yes(会先说清后果)
remote-hand-server setup-code --instance alice       # 再看一次服务端保存的一次性设置码
remote-hand-server open --instance alice             # 没有账号时:以这台设备主人的身份打开页面
```

不写 `--instance` 时用的就是服务端启动时那个 `$APP_INSTANCE`(默认 `default`);如果默认出来
的实例在本机根本没有目录,命令会直接拒绝并把现有实例列给你看——把账号加到没人在跑的实例上,
等于报告了成功却让真正在跑的那个服务端继续停在设置状态。

升级不会开门:某个实例第一次读自己的账号表时,会**一次性**继承原来全机那份账号
(`~/.config/remote-hand/state.db` 的 `users` 表,表空时回落到 `~/.config/remote-hand/users.json`)。
全机那份**只冻结、不删除**——回滚到旧二进制时它得还能找到账号、继续 fail-closed。代价写在
`server-rs/src/auth/users.rs` 的模块注释里:升级之后吊销的账号只会从实例里消失,回滚后旧
二进制仍然认它。这个方向是故意选的:它可能让一扇本该换锁的门还能被开,但绝不会让一扇门根本
没有锁。

#### 谁需要登录

| 实例的状态 | 浏览器怎么进来 |
|---|---|
| 有至少一个用户账号 | 登录 |
| 没有用户账号(桌面 App 的应用内部账号不算) | 在这台设备上用 `remote-hand-server open`(桌面 App 的实例:App 的「在浏览器中打开」);其他设备用一次性设置码建第一个账号(桌面 App 的实例没有设置码) |
| `APP_AUTH__ENABLED=false` | 什么都不问(⚠️ 仅供开发和测试,启动时会大声警告) |

**没有任何请求因为「从哪儿连进来」而被放行。** 从 127.0.0.1 连进来不代表坐在这台设备前:
`ssh -R`、frp、ngrok tcp、socat、Clash / Surge 的「允许局域网」这类 TCP 转发,会让陌生人的
连接以回环地址到达,`Host` 想写什么写什么。所以没有账号时,所有接口都要一个会话:没有会话的
请求只能碰状态接口、两条「进门」的接口(设置码、local-open)和 SPA 外壳——这就是**设置状态**。

**local-open(本机能力)**:服务端在 `instances/<实例>/control/` 下开一个 unix socket(目录
0700、socket 0600,连接时还核对对端 uid),只有运行服务端的那个系统用户够得着——也就是 AI CLI
以其身份运行、本来就能读一切的那个用户。`remote-hand-server open` 经这个 socket 要一个一次性
令牌(256 位随机,只能用一次,60 秒内有效,服务端只在内存里存它的 sha256),再让浏览器打开
`http://127.0.0.1:<端口>/#local-open=<令牌>`:令牌通过一个 0600 的跳转页交给浏览器,不出现在
任何进程的命令行里(别的系统用户 `ps` 看不到)。页面启动时先把令牌从地址里抹掉,再放进请求体
`POST /api/auth/local-open` 换成一个普通的长期会话,身份是 `local:owner`(这台设备的主人),
所以刷新和重启服务端都不掉。没有图形界面时 `open --print` 把链接打印出来;浏览器打开成功后链接
不再打印(输出可能被包装脚本、日志或 AI CLI 读到),打开失败时打印链接并以非零状态退出(桌面 App
据此改为打开普通页面)。网页上给出的 `open` / `setup-code` / `user add` 命令会带上这个服务端的实例
(`APP_INSTANCE=…`)和非默认的数据 / 配置目录,复制即可用于第二个实例或第二套安装。`open` 只信这台设备
自己服务端的回答:socket 所在目录必须是本用户的 0700 目录、对端 uid 必须是本用户、页面必须是
`127.0.0.1` / `[::1]` 上本实例记录的端口、跳转页必须是控制目录里本用户的普通文件,否则一律拒绝,
什么都不交给浏览器。页面地址取监听实际绑定的地址字面量,不用配置里的主机名(防局域网 DNS 冒充)。
所以只绑定在某一个局域网 / Tailscale 地址上的服务端(`APP_SERVER__HOST=100.x.y.z`),`open` 会拒绝:
那里用设置码,或者在设备上 `user add`。监听 `127.0.0.1`、`::1` 或全部地址(`0.0.0.0` / `::`)都可以用。

⚠️ 控制 socket 对**同一系统用户的任何进程**都可达——包括 AI CLI 自己,除非它的沙箱不让它连任意
unix socket。给 AI CLI 配沙箱时,请拒绝访问 `instances/<实例>/control/` 和 `/tmp/remote-hand-<uid>-*/`。
设置码文件 `instances/<实例>/setup-code` 同理。所以**桌面 App 的实例**(它永远处在设置状态:唯一的
账号 `droppen-app` 是应用内部账号)两样都不给:服务端不生成设置码,控制 socket 只回一句拒绝、不发
令牌。App 的「在浏览器中打开」改用它自己的登录经 HTTP 要一次性令牌
(`POST /api/auth/local-open/grant`,只认桌面 App 服务端上的应用内部账号),和以前一样要钥匙串里
那份密码才进得去。

只在**没有用户账号**
时可用:主人可以不要设置码直接建第一个账号,也可以把这台设备连到 Droppen、看到其他设备的副本
(主人就是链接的管理者);**第一个用户账号一建好,所有 `local:owner` 会话立刻作废**,从此处处
登录——有账号的安装和以前完全一样。

**应用内部账号**(`user add --app`;`user list --app` 列出;`user mark-app` 把已有账号转过去):
桌面 App 自己窗口的隐藏登录,名字是 `droppen-app`,密码随机、没人见过。它能登录,但**不算用户
账号**——不结束设置状态,`user list` 不列它,网页上不能改它的密码,也不能凭它看到其他设备。
桌面 App 的服务端没有凭据就进不去;同一台 Mac 上的浏览器要么用 App 的「在浏览器中打开」(App 用
自己的登录要一次性令牌,浏览器以主人身份打开页面),要么建一个用户账号。

**第一个用户账号**有三条路:

- 在设备上 `remote-hand-server user add <name>`——不需要设置码,能在这台机器上开 shell 的人
  本来就被信任;
- 用 `remote-hand-server open` 打开的页面(主人身份)里直接建——也不需要设置码;
- 在别的设备的浏览器里,用**一次性设置码**:服务端在没有用户账号时启动会生成一个(80 位随机,
  写成 4×4 组 Crockford base32,如 `7KQ2-M9XD-4HTB-W0RC`),存进 `instances/<实例>/setup-code`
  (0600),`remote-hand-server setup-code` 可以再看一次。**只有 stderr 是终端时才打印码本身**;
  launchd / systemd / nohup 的日志里只写「码在哪个文件」。

设置码:常数时间比较;只能用一次;实例一有用户账号(不管哪条路建的)就连内存带文件一起作废;
服务端退出时文件删掉;每次在没有账号的状态下启动都换新码;删掉最后一个账号后,运行中的服务端
几秒内生成新码。错码按来源地址(IPv6 按 /64)和全局两层计数:同一地址前 3 次免费,之后每错一次
等待翻倍(从 2 秒起);全局前 10 次免费,之后所有人都要等(从 1 秒起翻倍);上限 15 分钟,返回
`429` 和 `retryAfterSeconds`。来源地址取 socket 的,从不信转发头。全局限流被人故意打满时,出路是
`remote-hand-server open` / `user add`,或重启服务端(计数在内存里,重启还会换新码)。

删最后一个用户账号(`user rm … --yes`)前,命令会说清后果:实例回到设置状态,所有用账号登录的
浏览器都被登出。`remote-hand-server agent enroll`(含 `agent enroll -`)在实例没有用户账号时拒绝
执行,并提示先 `user add` 或用页面建账号;`APP_AUTH__ENABLED=false` 时也拒绝,服务端也不会启动
这台设备的 Droppen 连接。

其他加固:`/api/*` 上浏览器标明 `Sec-Fetch-Site: cross-site` 或 `same-site` 的请求(别的网站、
别的端口的页面发来的,包括 `<img src>` 和 websocket)一律 403;`/api/*` 的回应带
`Cross-Origin-Resource-Policy: same-origin`;除 `/embed/{id}` 以外的回应都带
`X-Frame-Options: SAMEORIGIN` 和 `Content-Security-Policy: frame-ancestors 'self'`(防点击劫持);
不再无条件信任 `localhost:5180` / `:8700` 这几个开发用 Origin(`pnpm dev` 本来就走代理)。
websocket / 下载用的 ticket 绑定签发它的会话:会话被登出、撤销或账号被删,ticket 立刻失效;下载
ticket 只活 60 秒。

`APP_AUTH__ENABLED=false` 是给开发和测试留的口子:任何能连上端口的人拥有全部权限,服务端
每次以这种方式启动都会在 stderr 和日志里大声警告。

开门之后:

- `POST /api/auth/login` `{username, password}` → `{token, expiresAt, username}`。token 是
  32 字节随机数的 base64url(43 字符),**服务端只存 `sha256(token)`**(在
  `instances/<name>/auth-sessions.json`,mode 0600),所以它自己也拿不出你的 token,
  「列出我的会话」这个接口在原理上就漏不了。
- 之后每个请求带 `Authorization: Bearer <token>`。有效期 **7 天绝对过期**,用它不会续期——
  滑动过期正是「泄漏一次 = 永久有效」的成因(攻击者自己的轮询就能把它续活)。
- 会话和账号都是**按实例**的:停掉 staging 实例不会把手机从 default 实例踢下线,给 staging
  加的账号也开不了 default 的门。
- 连续 5 次密码错误锁该账号 5 分钟。用户不存在 / 密码错 / 锁定中**返回完全相同的
  `401 {"error":"invalid credentials"}`**,并且都会跑一次 argon2 校验,时间上也分不出来——
  否则登录接口就成了「这台机器上有哪些账号」的查询接口。
- `GET /api/auth/status` 不需要 token,回 `{authRequired, setupRequired, authOff, user,
  desktopApp, droppenPath, droppenEnv}`,前端靠它决定画登录页、设置页,还是直接进应用;`user`
  可能是账号名、`local:owner` 或 `cloud:<设备>`;`droppenPath` / `droppenEnv` 是页面上命令的写法
  (二进制的路径,以及第二个实例 / 第二套安装要带的环境变量)。它是只读的:设置码由服务端启动时和之后每 3 秒的自检来生成
  或作废,不靠任何 `GET`。
  其余接口一律要:**`/openapi.json`、`/redoc`、`/scalar`、`/api/settings`、
  `/api/push/vapid`、self-upgrade 都不豁免**。
- `POST /api/auth/setup` `{code?, username, password}` 也不需要 token,只在设置状态下可用;
  带着 `local:owner` 会话时不要 `code`。成功时回和登录一样的 `{token, expiresAt, username}`,
  并作废所有 `local:owner` 会话;错误码 `invalid_account` / `code_required`(400)、
  `wrong_setup_code`(403)、`setup_done` / `name_taken` / `auth_off` / `no_setup_code`(409)、
  `too_many_attempts`(429,带 `retryAfterSeconds`)。
- `POST /api/auth/local-open` `{token}` 不需要 token:把 `remote-hand-server open` 给的一次性
  令牌换成 `local:owner` 会话;错误码 `invalid_local_open`(403)、`accounts_exist` /
  `auth_off`(409)。
- 豁免的只有四类:上面四条登录 / 设置相关的路由;CORS 预检(`OPTIONS`,浏览器本来就不带凭据);
  `/api/` 之外的 `GET` 静态资源(SPA 外壳与 assets——没有它就没地方输密码,但那三个文档
  路径被挖回去了);以及 `/embed/{id}/*`。
- 会话自己发通知那条路(`POST /api/notify`)**也不豁免**:它由本服务 spawn 出来的
  `rh-notify` 调用,凭据是 `crate::notify` 给每个会话签的无状态 keyed MAC(HMAC-SHA256),
  在 spawn 时注进环境。守卫在这一条路径上多认这一种凭据,认出来的是 `NotifyCaller` 而不是
  身份——它在别的任何路由上都换不到东西(实测:带着它打 `/api/settings` 依然 401)。
  这条路由会往外发 HTTP,所以开着它等于开一个垃圾通知中继;出站地址只来自服务端配置,
  请求体里没有 URL 可填,每个会话再压一层 10 分钟 6 条的限流。
- **websocket 与 `<img>` 带不了 header**,所以走 ticket:`POST /api/auth/ticket`
  `{scope: "ws"|"download"}` → `{ticket, expiresAt}`,附在 URL 上作 `?ticket=…`。
  `ws` 是**一次性**的、活 60 秒;`download` 可重复用、活 60 秒;各自只在自己那几条路径上
  认,换条路径就是废纸;签发它的会话一失效它就跟着失效。只有登录会话能签发。长期 token 永远不进
  URL。

> ⚠️ **`/embed/{id}/*` 是 capability URL,不在鉴权之内**:VS Code 服务端在这里说自己的协议,
> 路径不是本服务建模的。id 是 v4 uuid(122 位熵)且只能从鉴权后的 `GET /api/apps` 拿到,但
> **任何拿到这个 id 的人都能访问那个 app**。这是已知的残留风险。

> ⚠️ 鉴权不等于可以裸奔上公网:登录是明文 POST,token 也是明文 header。远程访问仍然应该放在
> TLS 反向代理 / SSH 隧道 / Tailscale 之后。默认绑定 `127.0.0.1`。

环境变量(服务端自己的是 `DROPPEN_*`;改名前的 `RH_*` / `REMOTE_HAND_*` 仍然认,用到时提示一次(加上同值的
新名字;还可能回滚到改名前的版本时旧名字也留着),两个都设而值不同时以新名为准,见 `server-rs/src/env.rs`):

- `APP_SERVER__HOST` / `APP_SERVER__PORT`:监听地址(默认 `127.0.0.1:8700`)。`APP_` 前缀与
  `__` 分隔是 gotcha 的配置约定。
- `APP_INSTANCE`:实例名(默认 `default`)。同一台机器跑多个 Droppen 服务端时各起一个名字,
  终端 shim / 运行记录 / 嵌入式 app **以及会话**的归属按实例隔离,互不认领:每个实例有自己的
  Claude config root(`instances/<name>/claude`),列表里只出现自己创建的会话,`~/.claude`
  作为 external 对所有实例可见。账号本身(machine 级 `state.db` 里的 `accounts` 表、每账号的额度目录)仍然共用。
- `APP_HOME_DIR`:会话与数据的发现根目录,默认 `$HOME`。
- `APP_AUTH__ENABLED`:**不设**和 `true` 一样,是默认(见上面「谁需要登录」的表)。`false`:
  关闭鉴权,任何能连上端口的人都有全部权限——**不安全,只供开发和测试**,启动时会大声警告。
- `APP_AUTH__SESSION_TTL_DAYS`:token 有效期,默认 `7`。绝对过期,不滑动。
- `APP_AUTH__LOCKOUT_ATTEMPTS` / `APP_AUTH__LOCKOUT_MINUTES`:连续失败多少次锁多久,
  默认 `5` / `5`;attempts 设 `0` 表示不锁。
- `DROPPEN_ALLOWED_ORIGINS`:逗号分隔的额外可信 Origin;与请求同 host 的 Origin 自动信任。
- `DROPPEN_ALLOWED_HOSTS`:逗号分隔的额外主机名(端口忽略)。服务端只回答发给这些名字的请求
  (看 `Host`,所有路径):IP 地址、`localhost` 和 `*.localhost`、只有本地网络才给的名字
  (不带点的单个名字如 `mymac`,以及 `*.local`、`*.lan`、`*.home.arpa`、`*.internal`、tailnet 的
  `*.ts.net`)、`APP_SERVER__HOST` 配的名字、`DROPPEN_ALLOWED_ORIGINS` 里的主机,再加这里列的。
  别的名字一律 `403`,所以经反向代理 / 隧道用自己的域名访问(Caddy、Traefik、Cloudflare Tunnel
  会把公开域名原样转过来)时要把那个域名写进来;`*` 表示任何名字都回答(以前的行为)。本地网络的
  名字由这个网络自己的名字解析决定,所以同一网络里能控制名字解析的设备仍能访问不需要会话的那几个
  接口(登录、首次设置、状态);其余一切都要会话。
- `DROPPEN_IDLE_TIMEOUT_MS`:一次 agent 运行「多久没有任何进展」才判定为卡死并终止(默认
  20 分钟)。进展 = CLI 有输出,或会话 transcript 文件仍在增长;**运行时间本身不设上限**,
  长时间的 agentic 任务不会被打断。

固定上限(不可配置,写死在代码里):

- 同时存在的终端数上限 64(含已退出但尚未回收的);超出时 `POST /api/terminals` 返回
  `429 {"error":"too many terminals"}`。
- 单个终端 websocket 客户端的待发送积压上限 4 MiB;超过说明该客户端已经不再读取,
  服务端会发一帧 `[client too slow]` 并关掉它——pty 读取线程绝不为任何一个浏览器标签页阻塞。
- 上传大小:`POST /api/fs/upload` 200 MiB,`POST /api/uploads`(消息附件)100 MiB。
- websocket 心跳:服务端每 30s 发一次 Ping,90s 内没有收到任何帧(浏览器会自动回 Pong)
  即断开——用来回收睡眠的笔记本、失效的 NAT 映射留下的死连接。

## macOS 裸机部署

日常使用推荐 Droppen 桌面 App（`Droppen-<version>.dmg`，与下面的发布包在同一个发布页；源码、构建与签名见
[`desktop/`](desktop/README.md)）。
下面是服务端安装，用于后台或无界面运行。

一条命令装好（下载、校验 SHA-256、建本地账号、注册 launchd、打开页面），重跑即原地升级：

```bash
curl -fsSL https://droppen.app/install.sh | sh
```

`droppen.app/install.sh` 直接给出 Droppen Hub 构建时所用那次发版的 `install.sh`，与那次发版里的文件逐字节相同
（`https://github.com/getdroppen/droppen/releases/download/<tag>/install.sh`；它的 sha256 见
`https://droppen.app/.well-known/droppen-build` 的 `install_sh`），不再跳转；源码是
[`scripts/install.sh`](scripts/install.sh)。二进制只从本仓库的
[Releases](https://github.com/getdroppen/droppen/releases) 下载，hub 不分发任何二进制（v0.2.126 及以前的发版在
[getdroppen/releases](https://github.com/getdroppen/releases)）。公开发布包由 GitHub Actions（[`.github/workflows/release.yml`](.github/workflows/release.yml)）
从本仓库的源码构建（公开发行版；macOS aarch64 + x86_64 + universal、Linux、桌面 App），每个文件都附
构建来源证明，可以用 `gh attestation verify <文件> --repo getdroppen/droppen --signer-workflow getdroppen/droppen/.github/workflows/release.yml --source-ref refs/tags/<版本>` 验证
（[`docs/releasing.md`](docs/releasing.md)）。它用的仍是 [`scripts/release-public.sh`](scripts/release-public.sh)
和 [`scripts/publish-release.sh`](scripts/publish-release.sh)：先建草稿、把桌面 App 的 `latest.json`
（没有新桌面 App 时沿用当前最新版本里的那份，连同它的 DMG）最后上传，然后才发布并标为最新版本
（[`desktop/SIGNING.md`](desktop/SIGNING.md)「Making a release」）。安装脚本用
`default` 实例和 launchd 标签 `app.droppen.server`（见下文[开机自启](#开机自启launchd)），发现本机
已有别的 Droppen（remote-hand）服务（名字或 plist 里带 `remote-hand` / `droppen` 的 launchd 任务、
systemd 单元）只列出、不碰。

**Linux 与无界面设备**用同一条命令（x86_64 / aarch64，glibc ≥ 2.28）：装成 systemd 用户服务
`droppen.service`（`KillMode=process`，终端和 agent 跨重启存活；登出后仍要运行需
`sudo loginctl enable-linger $USER`），没有 systemd 用户会话时只打印手动启动命令；`--system`
用 sudo 写一个开机启动、以当前用户运行的系统服务。带上 droppen.app 上「我的设备 → 添加可运行 AI
的设备 → 显示一次性注册码」给的注册码，就直接把这台设备加入账号，全程不需要浏览器：

```bash
curl -fsSL https://droppen.app/install.sh | DROPPEN_CODE=<code> sh
```

注册码只从环境变量 `DROPPEN_CODE` 或 `--code -`（在终端里输入、不回显）读取，从不放在命令行上；
脚本再经 stdin 交给 `remote-hand-server agent enroll -`，所以不出现在任何进程的参数里。登录账号和
入网都先用下载好、尚未替换的新二进制完成，失败时设备保持原样；替换后标记 `restart=1`，直到服务
真正重启进新版本。没有显示器时不打开浏览器，本地页面只给出经 SSH 隧道访问的端口。Linux 二进制由
[`scripts/build-linux-in-container.sh`](scripts/build-linux-in-container.sh) 在固定版本的镜像里用
cargo-zigbuild 构建（对 glibc 2.28 链接）：发版时由 `release.yml` 运行，在本机兜底时由
[`scripts/build-linux.sh`](scripts/build-linux.sh) 放进一次性 pod（集群由 `DROPPEN_K8S_CONTEXT`、
`DROPPEN_K8S_NAMESPACE` 指定）；打包后在 Debian 10 容器里冒烟测试
（[`.github/scripts/smoke-linux.sh`](.github/scripts/smoke-linux.sh)）。

**可选：网页里的 VS Code。** 侧栏「+」→ `vscode` 用 [code-server](https://github.com/coder/code-server)
在浏览器里打开 VS Code，Droppen 不会替你安装它。要用的话，把 code-server 装在服务端所在的设备上
（装法见 [code-server 的安装说明](https://coder.com/docs/code-server/install)），并且装在服务端的
`PATH` 能找到的目录里：安装脚本设置的服务（launchd agent 或 systemd 用户服务）和桌面 App 启动的
服务端，`PATH` 里都有 `~/.local/bin`，code-server 的独立安装（`--method=standalone`）正好装在这里；
运行安装脚本时用 `DROPPEN_SERVICE_PATH=…` 可以指定别的 `PATH`。服务端每次打开编辑器时才去找
`code-server`，装好后不用重启；找不到时，网页会提示缺少 code-server，并给出安装说明的链接。

手动安装：前端已经嵌进可执行文件里，**整个服务端就是这一个文件**，不装
运行时、不装依赖。从 [Releases](https://github.com/getdroppen/droppen/releases) 下载
`droppen-<tag>-macos-aarch64.tar.gz`（Apple Silicon；Intel 用 `-x86_64`，两者通用的是 `-universal`）。
包里的 `droppen` 和 `remote-hand-server` 是同一个文件（硬链接）、同一个程序；旧名字的
`remote-hand-<tag>-…tar.gz`（只有 `remote-hand-server`）给旧版安装脚本用，至少发到 2027-01-31：

```bash
curl -fsSL -O https://github.com/getdroppen/droppen/releases/latest/download/droppen-<tag>-macos-aarch64.tar.gz
shasum -a 256 -c <(curl -fsSL https://github.com/getdroppen/droppen/releases/latest/download/SHA256SUMS \
  | grep droppen-.*-macos-aarch64)

mkdir -p ~/.local/share/remote-hand/bin
tar -xzf droppen-<tag>-macos-aarch64.tar.gz -C ~/.local/share/remote-hand/bin droppen
mv ~/.local/share/remote-hand/bin/droppen ~/.local/share/remote-hand/bin/remote-hand-server
ln -sf remote-hand-server ~/.local/share/remote-hand/bin/droppen
~/.local/share/remote-hand/bin/remote-hand-server up -- serve
```

**文件名不能改**：supervisor 认的是 `<data>/bin/remote-hand-server` 这个固定路径（自我更新和
`.prev`/`.bad` 回滚阶梯也只换它）；`droppen` 是旁边指向它的符号链接，平时敲的命令用它
（`~/.local/share/remote-hand/bin/droppen user add <名字>`），升级后跟着指向新的二进制。别把
`droppen` 做成硬链接或复制件，否则自我更新之后它还是旧版本。跑起来后打开 <http://127.0.0.1:8700>。

用 `up -- serve` 而不是裸 `serve`：自我更新要靠它换二进制后
把新的拉起来，回滚阶梯和孤儿回收也都在它手里。裸 `serve` 也能跑，只是自我更新只会告诉你
`manual`，得自己重启。

> **Gatekeeper**：用浏览器下载的压缩包会被打上 `com.apple.quarantine` 扩展属性，解出来的
> 二进制一跑就被拦（"cannot be opened because the developer cannot be verified"）。清掉即可；
> 上面用 `curl` 下载则**不会**带这个属性，也就无需处理：
>
> ```bash
> xattr -d com.apple.quarantine ~/.local/share/remote-hand/bin/remote-hand-server
> ```

### 开机自启（launchd）

上面的安装脚本在 macOS 上把服务端装成 launchd user agent `app.droppen.server`
（`~/Library/LaunchAgents/app.droppen.server.plist`）：登录时启动，退出了由 launchd 重新拉起；
跑的是 `~/.local/share/remote-hand/bin/remote-hand-server up -- serve`，实例 `default`，只监听
`127.0.0.1`，端口从 8700 起找第一个空闲的（`DROPPEN_PORT` 可改起点，升级时沿用上次的端口）。
日志在 `~/Library/Logs/remote-hand/default.log` 和 `default.err.log`（目录 0700、文件 0600，
只有你自己能读：里面会写设置码在哪个文件）。

```bash
launchctl print gui/$(id -u)/app.droppen.server          # 状态
launchctl kickstart -k gui/$(id -u)/app.droppen.server   # 重启
tail -f ~/Library/Logs/remote-hand/default.log           # 日志
launchctl bootout gui/$(id -u)/app.droppen.server        # 停掉（下次登录还会启动）
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/app.droppen.server.plist  # 再启动
curl -fsSL https://droppen.app/install.sh | sh -s -- --uninstall   # 停掉并卸载，数据保留
```

plist 由安装脚本生成，别手改：重跑安装脚本时它会按当前设置重写这个文件，有变化就带着新设置
重启服务。手动安装的服务端不带开机自启，安装脚本也不接管它（发现不是自己装的二进制或数据就
停下，什么都不改）；要自己写 plist 的，照着 [`scripts/install.sh`](scripts/install.sh) 里
`launchd_write` 生成的那份写。其中有两个键值得单独讲：

- **`AbandonProcessGroup=true`**——macOS 上最关键的一行。不加的话，job 一停 launchd 就把整个
  进程组杀干净，而那正是终端 shim、嵌入式 app 本该**活下来**的时刻，一次自我更新会顺手带走你
  开着的所有终端。（这些子进程本来就 `setpgid(0, 0)` 脱离了 job 的进程组，所以它更像是保险，
  但很便宜。背景见 `server-rs/src/instance.rs` 的模块注释。）
- **`PATH`**——launchd agent 不读你的 shell profile，默认只有
  `/usr/bin:/bin:/usr/sbin:/sbin`，于是 `claude` / `codex` / `node` 全都找不到，会话一起就报
  command not found。安装脚本写进去的是你运行它时 shell 里的 `PATH`，后面再补上常见目录
  （`~/.local/bin`、`/opt/homebrew/bin`、`/usr/local/bin` 等）；之后装了新工具或换了 node 的
  版本管理器，重跑一次安装脚本就会刷新，也可以用 `DROPPEN_SERVICE_PATH=…` 直接指定。

### 与自我更新的关系

升级就是从新版 release 里解出二进制，`POST /api/server/self-upgrade`
传上去（见[上一节](#自我更新)）。注意 `sha256` 字段要的是**二进制自己**的摘要，
`SHA256SUMS` 里记的是 tar 包的，别搞混：

```bash
tar -xzf droppen-<new-tag>-macos-aarch64.tar.gz droppen
curl -sS -X POST http://127.0.0.1:8700/api/server/self-upgrade \
  -F binary=@./droppen \
  -F sha256=$(shasum -a 256 ./droppen | cut -d' ' -f1)
```

服务端会先让新二进制自证 `--version` 和 `preflight` 都能过，再原子换上、优雅退出，由 `up`
拉起新的——终端和正在跑的 agent 全程不断。旧的留成 `.prev`，坏了自动回滚。所以装完这一次
之后，后续升级不必再碰 `~/.local/share/remote-hand/bin/`，也不必重启 launchd job。

发版：服务端的 tar 包（macOS、Linux）、桌面 App 的 DMG、安装脚本和 `SHA256SUMS` 都发布在
本仓库的 [Releases](https://github.com/getdroppen/droppen/releases)，由 GitHub Actions 从本仓库构建并发布，
每个文件都附构建来源证明（[`docs/releasing.md`](docs/releasing.md)）。新装和升级都可以直接（重）跑
`curl -fsSL https://droppen.app/install.sh | sh`。二进制的版本号形如 `0.1.0+g<commit>`：构建时
取环境变量 `DROPPEN_BUILD_REV`（旧名 `RH_BUILD_REV` 照样认），没有就用 `git rev-parse --short HEAD`
（树里有未提交的改动时加 `-dirty`）。自我更新正是靠这个后缀区分同一个 crate 版本的两次构建，
所以在没有 `.git` 的源码目录里构建时请自己传 `DROPPEN_BUILD_REV`，否则版本号只有光秃秃的 `0.1.0`。

## 自己运行中继

`droppen-relay` 是一个静态进程：不要数据库，也不写文件（除非用 `DROPPEN_RELAY_STATE_DIR` 指一个卷来存它的钥）。
在仓库根目录构建镜像、生成钥，放在终结 TLS 的反向代理后面运行：

```sh
docker build -f server-rs/crates/rh-relay/Dockerfile -t droppen-relay .
docker run --rm droppen-relay droppen-relay keygen   # 打印 DROPPEN_RELAY_KEY=…（保密）
docker run -p 8080:8080 \
  -e DROPPEN_RELAY_HUB=https://<Hub 的源> \
  -e DROPPEN_RELAY_ORIGIN=wss://<这个中继对外的源> \
  -e DROPPEN_RELAY_KEY=<上面的钥> \
  -e DROPPEN_RELAY_REGISTRATION_TOKEN=<Hub 为这个中继发的一次性令牌> \
  -e DROPPEN_RELAY_TRUST_PROXY=railway \
  droppen-relay
```

注册令牌只在第一次启动时被 Hub 接受一次；之后也请留着它，因为中继的 id 就取自它，或者改设 `DROPPEN_RELAY_ID`。
中继按地址限流，要拿到每个客户端自己的地址：放在反向代理后面时设 `DROPPEN_RELAY_TRUST_PROXY=railway`，并让代理用客户端的地址
**覆盖** `X-Real-IP`（不能把客户端自己带来的值传进来）；在 Fly.io 上设 `fly`（`Fly-Client-IP`）。不设时中继用连接的地址，
放在代理后面就是代理的地址，所有客户端共用一个。

所有变量见 [`server-rs/crates/rh-relay/src/config.rs`](server-rs/crates/rh-relay/src/config.rs)，中继与 Hub 之间的协议见
[`docs/relay-and-plans.md`](docs/relay-and-plans.md)。中继只按自己的 Hub 签发的票配对通道，所以它只服务注册它的那个 Hub
的账户。droppen.app 只列出它自己运行的中继，暂不接入别人运行的中继。

## 安全边界

- droppen.app 上的网页由闭源的 Hub 下发。它是 `web/` 的可复现构建，每次发版附带逐文件哈希清单；`node scripts/verify-web.mjs` 检查站点此刻下发的字节是否与之相同。它证明不了有针对性的一次性投递，高保障请用本机界面。见 [`docs/web-integrity.md`](docs/web-integrity.md)。
- 裸机运行只绑定 `127.0.0.1`；公网部署请置于反向代理/隧道之后并自行加鉴权层。
- 跨源请求默认拒绝（同 host 与白名单 Origin 除外），防 CSRF 读写。
- 文件 API 限制在 home 目录内；上传不覆盖已有文件。
- 常驻 claude 用 `--permission-prompt-tool stdio` 启动后，**所有**本来会弹给终端用户的权限确认（写 workspace / `--add-dir` 之外的文件、`.git/config`、`.claude/settings.json`、ask 规则、沙箱联网、连续被拦后的兜底确认……）都会以 `can_use_tool` 请求发到服务端。Droppen **一律当场拒绝**（`behavior: deny`，消息说明是 Droppen 不做交互式授权），行为与以前 `--permission-mode auto` 下静默拒绝一致；网页只会被问到 agent 自己提的 `AskUserQuestion`，从不替你批准任何权限。`EnterPlanMode` / `ExitPlanMode` 用 `--disallowedTools` 关掉：进入计划模式无需确认，而退出计划模式本身是一个权限确认、会被上面的规则拒绝，模型会把自己锁在只读的计划模式里。
- `POST /api/server/self-upgrade` 会**替换这台机器接下来运行的程序**。在一个本来就无鉴权、
  能读写 `$HOME`、能开交互式 shell 的端口上，它并不算新增能力——打得到这个端口的人早就能以
  同一个用户执行任意命令了——但它确实新增了**持久化**：换上去的二进制留在卷里，重启还在。
  所以结论和第一条一样，只是更硬：这个端口必须待在带认证的反向代理 / SSH 隧道 / 私有网络
  之后，切勿直接绑公网。同源检查对这个路由同样生效，用户随手打开的网页 POST 不进来。

## 参与贡献与安全

贡献须知见 [`CONTRIBUTING.md`](CONTRIBUTING.md)：每个提交都要 `Signed-off-by:`（DCO，`git commit -s`），
每个 PR 都有一项必须通过的检查（DCO GitHub App 或同等的必需状态检查）核对它，缺了就不能合并；PR 以
squash 方式合并，合并后的提交信息保留各个提交的 `Signed-off-by:` 行。安全漏洞请按
[`SECURITY.md`](SECURITY.md) 私下报告，不要开公开的 issue。

## 许可证

Copyright (C) 2026 Kilerd。

- **应用**以 [GNU Affero General Public License v3.0 only](LICENSE)（AGPL-3.0-only）授权：
  本机程序（`server-rs/` 的根 crate `remote-hand-server`，即 `droppen`，含 agent 和本机网关）、
  桌面 App（`desktop/`）、网页（`web/`）和中继程序 `droppen-relay`。
- **协议与库 crate**——`rh-proto`、`rh-relay`（库）、`rh-client`、`rh-devkit`——以及插件 SDK
  （`packages/plugin-sdk`、`server-rs/crates/droppen-plugin-sdk`）以 `MIT OR Apache-2.0` 授权，
  二选一；许可证全文在各自目录的 `LICENSE-MIT` 和 `LICENSE-APACHE` 里。闭源的 Droppen Hub
  只链接这一类 crate。

公开发布包里附带 `LICENSE` 和 `THIRD_PARTY_NOTICES`（由
[`scripts/release-public.sh`](scripts/release-public.sh) 用 cargo-about 和
[`scripts/notices/`](scripts/notices) 生成，许可证白名单见 [`about.toml`](about.toml)）。网页的每次构建都在
`/THIRD_PARTY_NOTICES.txt` 提供其中 npm 包的许可证（droppen.app 上也是），公开页面的页脚有链接
（[`web/build/third-party-notices.mjs`](web/build/third-party-notices.mjs)）。
