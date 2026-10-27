# remote-hand-server

Droppen（原名 remote-hand）的服务端，Rust + [gotcha](https://crates.io/crates/gotcha) 0.4.1
（axum 0.8 + 从类型派生的 OpenAPI）。由 `server/`（Hono + `@hono/zod-openapi`）
逐模块移植而来，唯一**刻意**没有移植的是 browsers（`/api/browsers*`）——那个功能连同
TypeScript 服务端一起退休了。

构建产物是**一个可执行文件**：`web/dist` 在编译期由 rust-embed 打进二进制，运行时不需要
Node、不需要静态资源目录。

```bash
cargo run -- serve                                   # 127.0.0.1:8700
cargo run -- export-openapi --out ../openapi/openapi.json
cargo run -- replay --log <录像>.ndjson --home /tmp/replay   # 见「会话飞行记录器与 replay」
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

`web/dist` 缺席时 `build.rs` 会把 embed 降级成一个「请先跑 pnpm build」的占位页，所以
在没构建过前端的干净 checkout 上 `cargo check` / `cargo test` 照样能跑。

## 布局

每个子系统都是一个模块目录，对外只暴露 `routes` 与 `describe` 两个函数，在 `api/mod.rs`
里各占一行注册；它的状态是一个 struct，在 `api::AppState` 上加一个字段。

| 路径 | 负责 |
| --- | --- |
| `api/` | HTTP + websocket 表面：`AppState`、扁平的路由注册、错误体、`Created`、返回 JSON 400 的抽取器 |
| `app.rs` | 应用装配：config / state / routes / bind，以及取代 `GotchaApp::run` 的可停机 serve loop |
| `config.rs` | 手工组装的配置加载器，带类型化默认值（loopback 8700） |
| `origin.rs` | 每个 `/api/*` 请求都要过的同源守卫 |
| `sessions/` | 会话发现、transcript 归一化与线上数据形状 |
| `feed/` | 单会话 websocket feed；`feed/global.rs` 是 `GET /api/events` 背后的会话列表 feed |
| `drive/` | 驱动会话的路由，以及所有进程类子系统挂靠的 `Drive` 状态 |
| `inject/` | 驱动本机 CLI，每个会话一次运行 |
| `record/` | 会话飞行记录器：一个会话一份 NDJSON 录像，以及把它放回文件系统的 `replay` 子命令 |
| `accounts/` | 服务端保存的多个 Claude 登录，以及产出它们的两个 pty 向导 |
| `chats/` | 即时聊天：锚点目录里的 CLI 会话，加上 BYO-API 后端与它的流式回复 |
| `apps/` | 每个 app 一个 code-server，`apps/embed.rs` 是 iframe 用的 `/embed/{id}/*` 反向代理 |
| `quota/` | 订阅额度面板 |
| `schedule/` | 定时任务：一条 cron 表达式（按**服务器本地时区**解释）到点新开一个会话；`runner.rs` 是那个五秒一跳、只在持有实例锁时才启动的循环 |
| `notify/` | 会话自己决定「这件事值得打扰你」时走的推送通道（Bark），以及注进会话的 `rh-notify` 与 skill |
| `files/` | `$HOME` 下的浏览、读取与上传——客户端传来的路径只在这里被解析和收敛 |
| `terminals/` | 浏览器通过 websocket 驱动的 PTY |
| `prefs/` `screens/` | 两个边车存储：界面偏好、持久化分屏布局 |
| `stats/` `models.rs` | token 用量统计与模型列表 |
| `claude_roots.rs` `process_env.rs` | 一次 spawn 跑在什么根目录下、带什么环境 |
| `instance.rs` | 一个服务端独占的实例目录，以及宣示所有权的 `flock` |
| `procs.rs` | 「文件里那个 pid 还是当初那个进程吗」——重启后收养与发信号前的两道守卫 |
| `selfupdate/` | `POST /api/server/self-upgrade`，以及 `<data>/bin` 这个二进制仓库 |
| `supervisor.rs` | `remote-hand-server up`：重启子进程、回滚坏二进制、收孤儿 |

## 四个「填坑」模块

框架给不了的东西集中在这四个文件里，而不是散落在各子系统：

- **`openapi_ext.rs`** — 一个声明式的补丁注册表，在唯一一次 `GotchaRouter::openapi(..)`
  里统一应用。承担：额外状态码、把派生出的 `200` 改成 `201`、二进制与 multipart 请求体、
  重写描述，以及 `Nullable<T>` / `Optional<T>` / `AnyValue` 这三个 zod 对齐包装。
- **`export.rs`** — `export-openapi` 命令。在 0 号端口上把**真实的** router 跑起来，
  发一个 `GET /openapi.json` 再写盘。这不只是绕路：它保证导出的文件与线上服务的文档逐字节
  一致。
- **`shutdown.rs`** — gotcha 完全没有的停机流程：SIGINT/SIGTERM、放在 state 里的
  `CancellationToken`（让活着的 websocket 与 listener 同时收到停机），以及按序拆掉 feeds /
  terminals / apps / 全局 watcher 的 `Cleanups`。
- **`static_files.rs`** — 挂在 router fallback 上的单个 tower `Service`，从二进制里取资源、
  深链接回落到 `index.html`、让 `/api/*` 的 404 仍然是真 404。

## 部署与运维注记

这些不是「填坑」，而是几条**必须由部署方式满足**的前提。破掉任何一条，重启存活就失效，
而且失效得很安静——终端还在网页上列着，只是背后的 shell 已经没了。

### 信号只能发给 serve 进程，绝不能发给进程组

pty shim、嵌入式 app、后台 CLI 都被刻意 spawn 进各自的进程组（`process_group(0)`），
就是为了活过服务端退出。**shim 自己不装任何信号处理器**——它没有 `SIGTERM` handler，
也不需要有：它靠的是「信号根本打不到它身上」。所以停止服务端时，信号必须精确地发给
serve 那一个 pid：

- `remote-hand-server up` 已经这么做了（`kill(child, sig)`，永远不是 `kill(-child, sig)`）；
- systemd 下必须写 `KillMode=process`。默认的 `control-group` 会把整个 cgroup 一起杀掉，
  也就是把所有 shim 一并带走；
- launchd 下必须在 plist 里写 `AbandonProcessGroup`（`<key>AbandonProcessGroup</key><true/>`）。
  没有它，job 停止时 launchd 会杀掉整个进程组。

反过来说：`up` 之所以能当容器里的 pid 1，正是因为这些进程在服务端退出后会被过继给它，
而它用 `waitpid(-1)` 无差别收尸。镜像里因此不需要 tini / dumb-init / `--init`。

### 退出码 75：实例锁被别人占着，不是二进制坏了

`serve` 发现实例锁已被另一个服务端持有时，提示文字不变，但退出码是 **75**
（`EX_TEMPFAIL`），其余任何失败仍是 1。`up` 把 75 当作「等持有者走」：不计入快速失败、
永远不会因此把二进制挪成 `.bad`，日志里写出持有者的 pid，按平常的退避（最长 30 秒）
反复重试，锁一空出来就接管。

最常见的来路是 `up` 自己被 `SIGKILL`（OOM、服务管理器停止超时）：它的 `serve` 按设计活了下来、
继续持锁并继续服务，服务管理器拉起的新 `up` 于是一直撞锁。以前这会在三次后把好好的二进制
回滚掉，没有 `.prev` 时服务就再也起不来。`up` 不去「收养」那个孤儿进程（它不是自己的子进程，
无法 `waitpid`，而锁文件里的 pid 只是提示、可能被复用），只认内核的 `flock`。

### `<data>/bin` 是按数据目录共享的，不是按实例

自我更新换的是 `<data>/bin/remote-hand-server`，而 `<data>` 由**同一个数据目录下的所有
实例**共享——`instances/<name>/` 才是每个实例独占的部分。这带来一个需要知道的语义：

- 在实例 A 上调用 self-upgrade，换掉的是所有实例都会用到的那个二进制文件，但**只有 A 会
  重启**（退出的是 A 的 serve 进程，A 的 supervisor 把它拉起来）；
- 实例 B 继续跑着旧二进制，直到它自己下一次重启为止；
- 因此「升级完成」在多实例部署里意味着「每个实例都重启过一次」。要立刻生效就逐个重启，
  或者干脆给每个实例配一个自己的 `DROPPEN_DATA_DIR`，让它们连二进制也不共享（旧名
  `REMOTE_HAND_DATA_DIR` 已弃用，v0.2.135 起不再生效）。

回滚阶梯（`.prev` → 镜像里那份）同样是共享的：A 触发的回滚会改变 B 下次重启时跑的东西。

### Web Push 要求 secure context，这条是浏览器定的

`crate::push` 的三条 REST 路由（`GET /api/push/vapid`、`POST`/`DELETE
/api/push/subscriptions`）在 HTTP 下**照常工作**——它们只是普通 JSON，诊断时正好可以直接
打。真正被卡住的是浏览器那一侧：`PushManager` 只在 secure context 里存在，也就是 HTTPS，
或者 `http://localhost`。局域网 IP 上跑纯 HTTP 时，`pushManager.subscribe` 根本无从调用，
网页因此显示「不支持」而不是静默地什么都不做。

能用的部署形态就这几种：

- 前面套一层带证书的反向代理（Caddy / nginx / Traefik），浏览器访问 `https://…`；
- Tailscale / Cloudflare Tunnel 之类自带 HTTPS 的隧道；
- 只在本机用时访问 `http://localhost:<port>`——`localhost` 被规范单独豁免，IP 形式不算。

前两种经过的名字（反向代理的域名、隧道的域名）要写进 `DROPPEN_ALLOWED_HOSTS`（逗号分隔），
否则服务端对发给这个名字的请求一律回 `403`（`crate::origin`「Which names the server answers
to」）。不用写的：IP 地址、`localhost` 和 `*.localhost`、本地网络的名字（不带点的单个名字、
`*.local`、`*.lan`、`*.home.arpa`、`*.internal`）和 tailnet 的 `*.ts.net`（`tailscale serve`
的名字也在其中）。本地网络的名字由这个网络自己的名字解析决定，所以同一网络里能控制名字解析的设备
仍能访问不需要会话的那几个接口（登录、首次设置、状态）；其余一切都要会话。

另外两条运维前提：

- **VAPID 私钥落在实例自己的 `state.db`（`0600`）的 `kv` 行里**
  （`~/.local/share/remote-hand/instances/<instance>/state.db`），首次有人调
  `/api/push/vapid` 时才生成。它是「这台服务端的身份」，换掉等于让所有已注册设备的
  订阅一次性作废；备份时要把实例目录一起带上，别只备份 `~/.config/remote-hand` 那几个。
  订阅归属于一个 origin，所以这个库按实例分开存：同机第二个实例有自己的密钥和自己的
  设备列表，不会把通知推到另一个实例的 PWA 上。老版本留下的
  `~/.config/remote-hand/push.json` 与实例目录里的 `push.json` 都不再被读入。
- **`DROPPEN_PUSH_CONTACT`**（旧名 `REMOTE_HAND_PUSH_CONTACT` 已弃用，v0.2.135 起不再生效）覆盖 VAPID 的 `sub`（RFC 8292 §2.1：push service 出事时
  联系谁）。只接受 `mailto:` 或 `https:` 开头的值——Apple 的 push service 会直接拒掉别的
  形式——缺省是项目主页。

### 会话主动推送（Bark）：设备密钥永远不进会话

`crate::push` 是服务端替你判断「这一轮结束了」；`crate::notify` 是**会话自己**判断「这件事
值得打扰你」——卡在需要你决策、发现了你会想立刻停下来看的东西、长任务的里程碑、真的做完了。
这个判断只有 transcript 答得出来，文件系统答不出来，所以它是 agent 发起的，不是服务端推断的。

链路刻意绕了一圈：

```text
agent 跑 rh-notify ──POST /api/notify──> 本服务 ──> Bark ──> 手机
      （持会话 token）                    （持设备密钥）
```

把 Bark 的设备密钥塞进会话环境让 agent 自己 `curl` 是最省事的写法，也正是这个模块存在的
原因之所在——**不能那么做**：会话环境对 agent 跑的一切都可读，值会被引用进 transcript 与
总结，一个被要求排查网络问题的 agent 会大大方方把它打出来。所以密钥只留在
`~/.config/remote-hand/state.db`（`0600`；`notify` scope，与 `chat-provider` 同级，**全机共享**——
「你想收通知的那台手机」在哪个实例都是同一台），读接口只回掩码
（`https://api.day.app/••••Wc31`），写接口才收明文。作为交换，服务端是唯一同时知道两头的
一方，于是它能补上 agent 补不了的东西：会话标题，以及点开就回到那个会话的深链。

几条实现约束：

- **鉴权**：`POST /api/notify` **不豁免**。`crate::notify::token` 给每个会话签一个无状态的
  keyed MAC（HMAC-SHA256，密钥在上面那个库里），在 spawn 时注进环境；`auth::guard` 在这
  一条路径上多认这一种凭据，认出来的是 `NotifyCaller` 而不是 `AuthIdentity`——它不是身份，
  在别的任何路由上都换不到东西。无状态是因为常驻 claude 活得比服务端久，重启后没法把新的
  内存表递给一个已经在跑的进程。
- **不是 SSRF 跳板**：出站地址只来自服务端配置，请求体里没有 URL 可填。
- **限流**：每个会话 10 分钟 6 条，第 7 条 `429`，并且明说「留给真正需要的那条」。
- **`rh-notify` 是 Rust 子命令而不是 shell 脚本**：参数是模型写的散文，里面有引号、反引号、
  `$`、换行、CJK 和 emoji，手搓的 shell-JSON 编码器不是把请求体写坏，就是把
  「构建失败：`rm -rf $HOME`」变成一次事故。注进 PATH 的那一行 shell 只有
  `exec '<binary>' notify "$@"`——`"$@"` 是 POSIX 里唯一不重新分词、不重新展开的传参构造。
- **skill 只写进本服务自己的 Claude root**（`$CLAUDE_CONFIG_DIR/skills/rh-notify/SKILL.md`）。
  CLI 对 skill 的发现完全跟着 `CLAUDE_CONFIG_DIR` 走，没有回落到 `~/.claude` 这一说，所以跑
  在用户自己登录（本机 `~/.claude`）上的会话拿得到 `rh-notify` 命令、拿不到 skill：那个目录
  是用户的，往里撒文件的服务端是卸不干净的服务端。通道一关，下次 spawn 就把 skill 目录删掉，
  免得继续给 agent 广告一个只会被拒绝的工具。

## 会话飞行记录器与 replay

Session 页的渲染只能拿真实数据迭代，而值得拿来迭代的会话——四个子 agent 同时在写、一次
turn 卡住、崩溃中途 fork——要花一下午才能碰上一次，而且只碰得上一次。所以把它录下来。

`record/` 给**每个 claude 会话**写一个只追加的 NDJSON：

```text
<data>/instances/<name>/session-logs/<session-id>.ndjson
```

跨 turn、跨进程、跨服务端重启都是同一个文件：settings 变了要重启 claude、升级换了服务端要
重新 attach，这些都只是往同一份录像里继续追加。每行一个事件，`v` + `at`（ISO-8601 毫秒
UTC）+ `kind` 三个公共字段，扁平结构，`jq` 一行就能看：

| `kind` | 内容 |
| --- | --- |
| `spawned` | selector（fresh/resume/fork）、pid、shim、cwd、model/effort/permission |
| `attached` | 收养一个上个服务端留下的 claude：pid、shim、journal 的 turn 标记、队列长度、待重放字节数 |
| `stdin` | 写给 claude 的整行（用户消息、`control_request`） |
| `stdout` | claude 写回来的整行，含非 JSON 的杂行；`replayed` 标记 shim 重放期间的历史 |
| `gap` | shim 环形缓冲溢出，`from`/`to` 之间的 stdout 永久丢失 |
| `lifecycle` | `turn-open` / `result` / `stalled` / `interrupt` / `escalate`(rung) / `respawn` / `recover`(selector + 死因) / `adopt` / `exit`(墓碑) |
| `transcript` | CLI 往 transcript 追加的每一整行：`file` 是 `main` 或 `agent:<id>`，`path` 相对于 Claude 配置根 |
| `sidecar` | CLI **整体重写**的文件：agent meta（单行、没有结尾换行，按行 tail 永远看不见它）与 `tasks/<id>/*.json` 检查清单 |
| `note` | 记录器自己说话（上限停录、游标丢失） |

**token 不会进录像，这是类型层面保证的**：账号 token 只活在 `process_env::Env` 里，spawn 时
`env_clear` 后整体套上，从不写进任何流；而记录器唯一能接受 spawn 信息的类型 `SpawnSummary`
根本没有能装环境的字段。剩下的 stdio 因此**原样记录、不做脱敏**：stdin 是用户自己的 prompt
和我们自己拼的 `control_request`，stdout 是 claude 的 stream-json——同样的内容 CLI 正在毫无
遮掩地写进两个目录之外的 transcript，而录像干的就是把那些文件抄下来。对它跑
`redact_secrets` 换不来任何安全，却会毁掉 replay 唯一依赖的性质：重建出的 transcript 与 CLI
写的那份逐字节相同。（脱敏仍然守着所有把子进程输出塞进 **API 响应**的路径，录像不是其中之一。）

几条运维事实：

- `DROPPEN_SESSION_LOG=0` 全局关掉；`DROPPEN_SESSION_LOG_MAX_MB`（默认 256，`0` 表示不限）是**单会话**
  上限。到顶是**停录**而不是轮转——录像只有从第一行开始才有意义，丢头保尾等于丢掉整份——
  停之前会写一条 `note` 说明，免得看起来像会话自己结束了。
- 旁边还有一个 `<session-id>.offsets.json`，记着每个 transcript 抄到了第几字节。没有它，每次
  服务端重启都会把整份 transcript 再抄一遍，replay 出来就是四份重复的对话。它要是丢了，
  transcript 会从**当前末尾**接着抄并写一条 `note`：录重了是静悄悄的错，录漏了是能被人看见
  并处理的不完整。
- 写入走一个独立 task + 无界 channel，成批 flush、**不 fsync**。记录失败永远只是沉默降级
  （目录建不出来、id 不像 id、文件打不开都退化成一个什么都不做的句柄），绝不会拖慢或拖垮
  会话本身。
- session shim 为了不截断 turn，留着 100 MiB 的 stdout 环形缓冲，每次 attach 都会整段重放。
  重放部分**最多只再录 8 MiB**，超出写一条 `note`：这些行上一个服务端已经写进同一个文件了，
  按重启次数重录会把上限全花在重复内容上，而录像一停，transcript 也跟着停——那才是 replay
  真正需要的一半。
- 会话被**归档**后 `find_claude_session` 就看不见它了，录像也就不再增加 transcript 行（记录器
  刻意走服务端自己的查找，好让它跟的永远是别处认定的那份文件）。取消归档即恢复。
- 只录 claude 的常驻会话。codex 是一次性 `exec` 运行、thread id 要跑起来之后才公布，没法在
  开跑时命名文件，暂时不录。

### 拿录像重放一遍

```bash
remote-hand-server replay --log <data>/instances/default/session-logs/<id>.ndjson \
                          --home /tmp/replay --max-gap 3 --speed 2
APP_HOME_DIR=/tmp/replay APP_SERVER__PORT=8701 remote-hand-server serve
```

replay 只做一件事：把 `transcript` / `sidecar` 事件按原始时间轴写回文件系统，在 `--home`
下按真实布局重建 `<data>/instances/<instance>/claude/projects/<project-dir>/<id>.jsonl` 与
`<id>/subagents/agent-*.jsonl`。它不渲染、不解析、不起服务——渲染要留给**真的服务端**，这
才是整件事的意义：真实数据流过真实管线。

- `--max-gap <秒>`（默认 3）把超过它的沉默压缩成它。会话跑六小时，其中五小时五十分在等人，
  这条参数是「几小时的会话不用等几小时」的全部原因。
- `--speed <N>` 把剩下的时间除以 N；`--instant` 完全不看表，一次写完。
- `--instance <name>` 决定 `managed` 的文件落进哪个实例的根，要和之后 serve 时的
  `APP_INSTANCE` 对上；两边默认都是 `default`。
- 同一个 `--home` 重放两次会**替换**而不是叠加：改渲染 → 重放 → 看效果这个循环，不该还要
  先删目录。
- 录像里的路径当作不可信输入处理：绝对路径、带 `..` 的、空的一律拒绝，replay 不能成为往
  `--home` 之外写文件的方法。

## 已经撞上的 gotcha 限制

按撞上的位置分组；括号里是上游出处，方便升级框架时逐条复核。

### OpenAPI 响应派生

- 一个 handler 只派生一个成功响应，`Result` 的错误侧被折进 `default`，路由无法声明自己的
  `404` / `409`（`gotcha_core-0.4.1/src/responsible.rs:129`）→ `openapi_ext` 的
  `ExtraResponses` 注册表。
- 派生出的成功响应恒为 `200`，而 `Responsible` 有一个 `impl<T: Schematic>` 的全覆盖实现，
  下游 crate 无法补充——所以拿不到 `201`（`gotcha_core-0.4.1/src/responsible.rs:25`）→
  `api/created.rs` 的 `Created<T>` 借用 `T` 的 schema，再由 `openapi_ext` 改状态码。
- 那个 `default` 响应删不掉，只能靠重新声明成功侧把它挤走 → `ResponseOverride` 上的
  `drop_default`。
- 响应描述被写死成 `"default return"` → `openapi_ext::describe` 事后改写。
- 返回 `axum::response::Response` 的 handler 根本无法被文档化，因为全覆盖实现要求
  `Schematic`（`gotcha_core-0.4.1/src/responsible.rs:81`）；而去掉 `#[api]` 也不是出路——
  未标注的 handler 干脆不会被记成 operation（`gotcha-0.4.1/src/router.rs:104`）→
  `files/mod.rs` 用一个手写 `Schematic`、不注册任何组件的 `FileBytes` newtype 兜底。
- `Multipart` 抽取器派生不出请求体 schema，只给出媒体类型 → `RequestBodyOverride`。

### schema 形状（与 zod 对齐）

- `<Option<T>>::required()` 恒为 `false`，表达不出 zod 的 `.nullable()`（一定在，但可以是
  `null`）（`gotcha_core-0.4.1/src/lib.rs:262`）→ `Nullable<T>`。
- `<Option<T>>::nullable()` 恒为 `Some(true)`，表达不出 zod 的 `.optional()`（可以缺省，但
  不会是 `null`）（`gotcha_core-0.4.1/src/lib.rs:275`）→ `Optional<T>`，配
  `#[serde(default, skip_serializing_if)]`。
- 两者都覆盖不到第三种：可缺省且不涉及 nullability 的 `z.any()` 盲盒 → `AnyValue`
  （`Screen.layout` 这类完全由客户端拥有的数据）。
- `Nullable<T>` 字段会把 `nullable` 写成 `$ref` 的兄弟键，而所有生成器都会忽略它 →
  `openapi_ext::hoist_nullable_refs` 事后把它提到组件上。

### 参数与宏

- `WebSocketUpgrade` 不是 `ParameterProvider`，`#[api]` 生成的
  `<WebSocketUpgrade as ParameterProvider>::generate(..)` 直接编译失败——websocket 路由
  根本没法标注（`gotcha_macro-0.4.1/src/route.rs:88`）→ 三个 socket 路由
  （`api/mod.rs`、`feed/global.rs`、`terminals/routes.rs`）都不标注、不进文档，与
  TypeScript 服务端的选择一致。SSE 同理：`chats/mod.rs` 的 `POST
  /api/api-chats/{id}/message` 也留在文档外。
- 元组 `Path` 抽取器只对一段和两段有文档实现（`gotcha_core-0.4.1/src/parameter.rs:43,60`）
  → 三段的路由（`drive/routes.rs`）改用结构体 `AgentPath`。

### 路由组装

- `GotchaRouter::nest` 拼接时多一个斜杠，会把路径记成 `/api//sessions`
  （`gotcha-0.4.1/src/router.rs:161`）→ 所有路径都写成扁平的全路径。
- 只有整体 `layer`，没有按路由生效的中间件（`gotcha-0.4.1/src/router.rs:207`）→
  `origin.rs` 铺满全局，再在守卫第一行自己判断 `/api/` 前缀。
- 没有自己的静态文件支持，也没有 `nest_service`，只有 `fallback_service`
  （`gotcha-0.4.1/src/router.rs:236`）→ `static_files.rs` 用一个 tower `Service` 占住
  fallback。
- 组合点因此只剩 `fallback_service` 与 `layer` 两个，而 fallback 已经被 SPA 占了；同时
  `GotchaApp::routes` 在 state 存在之前就被调用，需要 state 的服务没法在那里注册 →
  `apps/embed.rs` 的反向代理改在上一层组合：在 `app.rs` 拿到 `build_router` 返回的 axum
  router 之后重新接管 fallback，把非 `/embed` 的请求转交给它包着的 SPA service。

### spec 获取

- `GotchaRouter::operations` 与 `into_axum_router`——装配好的 spec 仅有的两个存在
  位置——都是 crate-private，文档本身以路由闭包的形式挂载，因此除了走 HTTP 没有别的办法
  拿到它（`gotcha-0.4.1/src/router.rs:38` 与 `:263`）→ `export.rs`。

### 生命周期

- 完全没有停机故事：`GotchaApp::run` 裸调 `axum::serve(listener, router).await`，没有
  `with_graceful_shutdown`、没有信号处理、也没有可以挂上去的生命周期钩子
  （`gotcha-0.4.1/src/lib.rs:230`）→ `app.rs` 覆写 `run` 自己 serve，借的是**公开**的
  `GotchaApp::build_router`，从而绕开 crate-private 的 `into_axum_router`。
- 框架的 `run` 把绑定到的地址藏在自己肚子里，调用方用不了 0 号端口 → `app.rs` 把
  bind/prepare 从 `run` 里拆出来，`export-openapi` 与集成测试才能拿到真实地址。

### 配置

- `APP_SERVER__PORT` 在原生加载器下**不工作**：mofa 的环境源只产出字符串，而它只在更早的
  源已经在同一路径放过数字时才会做强转（`mofa-0.2.0/src/utils.rs:122`），偏偏 gotcha 的
  加载器没有默认值源——于是在没有配置文件的机器上直接以
  `invalid type: string "9000", expected u16` 失败 → `config.rs` 手工组装加载器，把一个
  类型化的 `DefaultsSource` 放在文件源与环境源前面。
- `ServerConfig` 默认端口是 3000，而事后打补丁分不清「没设过」和「就是要 3000」→ 同一个
  `DefaultsSource` 把契约里的 `127.0.0.1:8700` 表达成真正的默认值。
- `Gotcha` builder 会吞掉配置错误、静默回落到 `Default`（`gotcha-0.4.1/src/builder.rs`）→
  用 trait API 而不是 builder，写错的配置文件会响。

### 错误

- `GotchaError` 把一切失败渲染成裸 `500` + 文本 body，既不携带状态码，也不是 web 客户端读
  的 `{ "error": string }`（`gotcha-0.4.1/src/error.rs:57`）→ `api/error.rs` 自带
  `ApiError`（状态码 + JSON body）。

### 非 gotcha、但同源的几处

- axum 的抽取器拒绝请求时返回 `text/plain` 与飘忽的状态码（`422` / `415` / `400`），而
  `@hono/zod-validator` 一律 `c.json(result, 400)` → `api/extract.rs` 用
  `ApiJson` / `ApiPath` / `ApiQuery` 包一层，并转发 `ParameterProvider`，OpenAPI 贡献不变。
- serde 的 `missing_field` 会让一个朴素的 `Nullable` 字段悄悄变成可缺省 → `openapi_ext` 加
  了 `deserialize_with`，缺 key 直接报错。
- `rust-embed` 的 derive 在目录缺失时直接让构建失败 → `build.rs` 判断 `web/dist` 是否存在，
  以 `web_dist_missing` cfg 的形式交给 crate 选择占位实现。

`tests/probe.rs` 里有一条测试专门守住上面最大的一类改动：断言 `openapi_ext` 的注册表确实
生效了（该有的 `404` 在、派生出的 `default` 不在）。升级 gotcha 后先看它。
