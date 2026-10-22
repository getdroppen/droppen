# Droppen 通行密钥：用 passkey 登录，并可选地用它解锁新设备

> 设计稿，2026-10-17（基于 v0.2.102，参考 `docs/local-aggregator.md`）。只写设计，不改代码。
>
> 前提与协议见 `docs/cross-machine-architecture.md`（下称「架构文档」）、`docs/relay-and-plans.md`（下称「中继文档」）和 `docs/local-aggregator.md`（下称「聚合器文档」）。
>
> **设备模型（链 v3，聚合器文档 §3）。** 一切都是**设备**：每台设备一对钥，每台设备都能远程控制。另有两个标志：
> - **可运行 AI**（链里 `host`）：加入时声明，由 admin 签进链；
> - **可管理账号**（链里 `admin`）：唯一能开关的一项。
>
> 只有 admin 持用户钥 `UK`；其它设备由 admin 把各 host 的 `MK` 封给它。新鲜度按操作计：控制 7 天，管理 12 小时。通行密钥的链条目是 v3 的 16–18 号，外加签名者 `Passkey(slot)`，编码以聚合器文档 §3.2 为准。§2 描述今天 v2 的代码，所以那里仍用 v2 的名字。
>
> **已定**（2026-10-17）：第二层做，默认关闭；方案 (c)，两位数批准或 24 小时；通行密钥永不授予管理权；待定期间什么都读不到；droppen.app 的子域不能用 WebAuthn（§3.8）。见 §10。
>
> **本文改动一条既有原则。** 架构文档 §1「为什么不算托管」和 §10 写着「不做 passkey PRF 解锁：那等于把钥匙托管给 Apple/Google 的同步账号」。这个判断本身没错，所以本文把它改成一个**默认关闭、按把开启、开启时明说代价**的例外（§4.1、§6.3），扫码配对和恢复词仍是默认和最后的退路。
>
> **开源边界**（聚合器文档 §2.1）：客户端的一切密码学（PRF 派生、`PAK`、封装、链上的通行密钥条目）都在开源的 `web/` 与 `rh-proto` 里；闭源 Hub 只校验 WebAuthn 断言、存公钥和打不开的密文。每个新接口都写进 `docs/hub-api/`。
>
> 路径缩写：`S/` = `server-rs/src/`，`C/` = `server-rs/crates/`，`W/` = `web/src/`。方括号里的数字是 §12 的资料来源。

## 0. 结论

- **通行密钥不取代每台设备自己的钥。**
  - Noise 要 X25519 ECDH，通行密钥做不了。
  - Hub 会话的签入和每条通道的握手太频繁，不能每次都要一次 Face ID。
  - WebAuthn 签名只覆盖 `authenticatorData ‖ clientDataHash`，签不了我们的 `rh/v1 …` 结构。

  所以设备钥、设备链、Noise、钥匙投递一概不变。通行密钥只加两层。
- **第一层：用通行密钥登录 Droppen Hub。**
  - RP ID `droppen.app`；可发现凭据；必须 UV；`attestation: none`。
  - 它只替代密码换来的那张 15 分钟「登录会话」。设备会话照旧由设备钥签挑战换取，已有设备永远不再需要密码或通行密钥。
  - 一个账户可以有多把；可以完全没有密码。添加、删除通行密钥和改密码都要 **admin** 设备。
  - 登录不填邮箱，所以无从枚举账户，也不需要 Turnstile。钓鱼站拿不到断言（RP ID 与 origin 绑定）；droppen.app 的子域另有规矩（§3.8，已定：Turnstile 搬到独立域名）。
- **第二层：用通行密钥解锁新设备（PRF），已定要做，默认关闭，按把开启。**
  - `PRF(salt)` → HKDF → 一把 Ed25519「通行密钥授权钥」`PAK` 和它的 X25519 孪生钥。构造与恢复钥 `RSK` 相同。
  - admin 用 `PasskeySlotSet`（16 号）在链上登记一个**槽**：`PAK` 公钥、salt，以及 `PAK` 对（账户, `AK`, 槽, 凭据, salt）的证明。再把每台 host 的每一轮 `MK` 封给孪生钥，存进钥环（`pk/m/…`）。Hub 只有公钥和打不开的密文，从来拿不到 PRF 输出。
  - **封给公钥，而不是用对称 KEK 包。** 所以以后轮换 `MK` 不要任何手势：做轮换的 admin 自动补封，与今天的恢复包一样。这与 Bitwarden 的做法同一思路 [36]。
  - 只有一种情况要手势：被移除的设备**算过**这个槽的 PRF（登记它的 admin，或用它加入的设备）。这时链上的槽随之作废（照搬「移除设过恢复钥的设备即清掉恢复钥」），要在一台 admin 设备上用一次 Face ID、换新 salt 重开。在此之前通行密钥解锁暂停：会过期，但从不失守。
- **方案 (c) 混合（已定）**（§4.9）。
  - `PAK` 像 (a) 一样，是链上独立登记、独立吊销的授权；但它的权力像 (b) 一样只有两样：以 `Passkey(slot)` 签一条 `flags = 0`、待定的 `AddDevice3`，以及 24 小时后的 `PasskeyActivate`（18 号）。
  - 常见路径：在另一台 admin 设备上输入新设备屏幕上的两位数，就批准了，前后约 3 次点按。
  - 待定期间诚实的 Hub 不给流、页和通行密钥包（已定）。所以偷到同步账户的人在 24 小时内**既读不到、也控制不了**；所有设备都会告警，任一 admin 都能否决。
- **通行密钥永不授予管理权（已定）**（§4.10）。用它加入的是普通设备：能控制、能读全部历史，读不到 UI 文档，不能管理账号。管理权只能由已有的 admin 在批准时一并用 `SetAdmin` 授予，或者用恢复词恢复。所以开启时仍建议保留恢复词。
- **代价要写明。** 谁掌握你的通行密钥同步账户（例如 Apple 账户 + 一台受信设备的锁屏密码），谁就可能解锁。这是交给第三方的托管，所以默认关闭，开启要勾选一句明确的话（§6.3）。
- **本机 Droppen 安装不用通行密钥**（§4.11）：它不在 droppen.app 的源上，`MK` 照旧由 admin 投递。`/link` 弹窗里可以用通行密钥登录，也可以用它解锁弹窗所在的浏览器。
- **支持面**（§5）：
  - 通行密钥登录在主流浏览器与系统上都可用。
  - PRF 可用的有：Safari 18+ 配 iCloud 钥匙串；Chrome、Edge 配 Google 密码管理工具或 iCloud 钥匙串；Firefox 139+；1Password、Proton Pass；YubiKey 5.2+。Windows Hello 要 Windows 11 2026 年 2 月的更新加 Chrome 146+ 或 Firefox 148+。
  - 缺口：Windows 10；Microsoft 密码管理器；Dashlane、NordPass、KeePassXC；Bitwarden 只部分可用；Tauri 的 webview 用不了（其源不在 droppen.app）。
  - 探测不到 PRF 时，这把通行密钥只用于登录，界面照实说明。
- **分 6 步**（§8 的 K1–K6）：
  - K1 通行密钥登录；
  - K2 管理通行密钥、只有通行密钥的账户、邮件找回登录；
  - K3 派生与证明的原语（聚合器文档的 SC-1 负责解码与应用 16–18 号）；
  - K4 开启解锁、新设备加入、两位数批准（要求账户能写 v3）；
  - K5 24 小时自行激活、槽作废与重开；
  - K6 `/link` 弹窗。

## 1. 为什么分两层，用到的术语

| | 设备钥 | 通行密钥 |
|---|---|---|
| 跟着谁 | 这一台浏览器或这个安装，不同步到别处 | 这个人：在同步账户里跟到他的每台设备 |
| 能做什么运算 | Ed25519 签名、X25519 ECDH（Noise、封装） | 对 `authData ‖ clientDataHash` 签名；PRF（若支持） |
| 每次要手势吗 | 不要 | 要（UV） |
| 适合做 | 设备身份、链条目、握手、收发钥匙 | 登录凭据；「新设备怎么拿到第一把钥匙」的备用凭据 |

第二层用到的只是 PRF：它把「这个人手里的通行密钥」变成一个稳定的 32 字节秘密，再由客户端从中派生出我们自己的钥匙（§4.2）。WebAuthn 的签名本身从不进入我们的协议。

**术语**（以聚合器文档 §3 为准）

| 词 | 意思 |
|---|---|
| 设备 | 设备链里的一员：浏览器、手机、Droppen 安装。每台都能远程控制可运行 AI 的设备 |
| host（可运行 AI） | 运行 agent，为自己的流持有 `MK` 的各轮；加入时声明，用户不能开关 |
| admin（可管理账号） | 签链条目、持 `UK`、作证、创建注册码、给别的设备投递钥；用户逐台开关 |
| 普通设备 | `flags = 0`：能控制、能读离线副本，不可运行 AI、不可管理账号。通行密钥只能加入这种设备 |
| `UK` | 只给 admin。其它设备由 admin 把各 host 的各轮 `MK` 用信箱用途 2 封给它；缺钥时发用途 4 索取，在线的 admin 自动只回 `MK`（聚合器文档 §3.3） |

## 2. 现状（已在代码里核对，v2）

| 方面 | 现状 | 位置 | 对本设计的影响 |
|---|---|---|---|
| 登录 | 邮箱 + 密码（argon2id，每个地址同时只算一个哈希）换一张 15 分钟的**登录会话**：只能登记设备、读链、`/v1/me`、登出、重发验证信 | Hub | 通行密钥换来的是同一种登录会话，不新增角色 |
| 设备会话 | 设备对无状态挑战签 `HubLogin{device-session}`，换一张 30 天的会话，不需要登录；链上移除即吊销 | Hub | 已有设备永远不再需要密码或通行密钥 |
| 账户 | 每个账户都有邮箱和密码 | Hub | 只有通行密钥的账户要允许没有密码 |
| 防滥用 | 登录按（账户, 地址前缀）锁定，5 次锁 5 分钟；账户 1 小时失败 3 次或地址失败 10 次后要 Turnstile；注册总要 Turnstile 与邮箱验证 | Hub | 通行密钥登录没有可猜的东西，不需要锁定和 Turnstile |
| Turnstile 的源 | 组件页在 `verify.droppen.app`（同一服务按 Host 分流），只在那里加载 Cloudflare 的脚本 | Hub、中继文档 §4.2 | 它是 **droppen.app 的子域**，能以 RP ID `droppen.app` 调 WebAuthn，见 §3.8 |
| 唯一的源 | 其它主机名一律 308 到 Hub 的源 | Hub | WebAuthn 的合法 origin 只有一个 |
| 恢复钥 | `RK` → HKDF → `RSK`（Ed25519）+ X25519 孪生钥；每轮 `UK` 封给孪生钥，钥环名 `rk/<UK 轮>/<发送者>/<标签>/<写入者>`；链上的 `RecoveryKey` 带 `RSK` 对（账户, `AK`）的证明；`RSK` 只签 `RecoveryJoin`、`ActivateRecovered`；移除设过或用过恢复钥的设备，恢复钥随之清掉 | `C/rh-proto/src/chain.rs:530-575,961-1080`、`W/lib/e2e/wrap.ts`（`sealForRecovery`）、`W/lib/hub/account.ts:1056-1095`（`ensureRecoveryPackages`）、Hub | 第二层照搬这一整套，只换派生来源和封装的内容 |
| 待定设备 | `ChainState::device()` 只返回活跃设备：agent 不与待定设备握手，Hub 不让它 `SUB` 流、不给页；但钥环对任何已登记、未移除的设备可读 | `chain.rs:1349`、Hub | 待定期间读不到历史；通行密钥包要另加一道门（§4.6） |
| 链版本 | 条目版本 2；v3 由聚合器文档定义，通行密钥条目在同一版本里 | `chain.rs:58`、聚合器文档 §3.2、§3.5 | 共用 `chain/3` 门槛与 Hub 的 v3 开关 |
| 告警 | 加删设备、恢复加入、自行激活、批准、换恢复词：所有设备的告警条，所有机器的日志与推送（Web Push + Bark） | 架构文档 §9「账号变更提醒和恢复否决」、聚合器文档 §3.2「告警」 | 通行密钥条目已列进 v3 的告警表 |
| 本机登录 | 凭据只在 droppen.app 的弹窗里输入，本机拿不到 Hub 的登录会话 | 聚合器文档 §4.2 | 通行密钥天然只能在那里用（RP ID） |

## 3. 第一层：用通行密钥登录 Hub

![用通行密钥登录：注册时创建通行密钥、登录时可发现凭据的断言、之后与今天相同](passkeys/signin.svg)

### 3.1 依赖方与参数

| 参数 | 值 | 理由 |
|---|---|---|
| `rp.id` | `droppen.app`（Hub 的配置，默认取 Web 源的主机名；`/v1/signup/config` 的 `passkeys.rp_id`） | 与本机登录的 `/link` 同源 |
| 合法 origin | 恰好 `https://droppen.app`（Hub 的配置；开发时加 `http://localhost:<端口>`；以后加原生应用的源，§5.4） | 子域即使能调 WebAuthn，Hub 也不收它的应答（§3.8） |
| `user.id` | 账户的随机 32 字节（§3.3） | 不泄漏邮箱或账户 id；规范上限 64 字节 [42] |
| `user.name`、`user.displayName` | 邮箱 | 只显示在用户自己的密码管理器里，用来认出账户；改邮箱后用 Signal API 更新（§3.6） |
| `pubKeyCredParams` | Ed25519（-8）、ES256（-7）、RS256（-257） | 覆盖平台认证器与旧安全钥匙 |
| `authenticatorSelection` | `residentKey: "required"`，`userVerification: "required"` | 可发现凭据才能不填邮箱；UV 让它成为单独就够的一个因素 |
| `attestation` | `"none"` | 不需要也不验证认证器出处；AAGUID 只作显示 |
| `extensions` | `credProps`；创建时总带 `prf: {eval: {first: 一次性随机 32 字节}}` | `rk` 是否真的可发现；Windows Hello 只有在创建时收到 PRF 输入，才建出支持 PRF 的凭据 [12]。这次的 PRF 结果直接丢弃（除非是 §4.5 的合并开启） |
| `timeout` | 120 s | — |
| 挑战 | 无状态（§3.4），2 分钟，只认一次 | 申请挑战不在 Hub 上存任何东西，与设备会话的挑战同一做法 |

浏览器侧用 `PublicKeyCredential.parseCreationOptionsFromJSON` / `parseRequestOptionsFromJSON` 和 `toJSON()`，缺失时用自带的 base64url 换算（`W/lib/hub/passkey.ts`）。

### 3.2 Hub 存什么

> 本节是 Hub 的表结构与迁移，不在公开仓库。

对客户端要紧的：

- Hub 为每把通行密钥存凭据 id、公钥、计数、BE/BS、AAGUID、标签与时间（`GET /v1/passkeys` 返回的那些，见 `docs/hub-api/`），从不存 PRF 输出；只有通行密钥的账户可以没有密码。
- 解锁槽**不**在 Hub 的表里：它在链上（§4.4）。Hub 从自己验证过的链里按 `cred_hash = SHA-256(credential_id)` 对上哪把通行密钥开了解锁，只用于显示和清扫。
- AAGUID 到提供方名的对照取自公开的 passkey-authenticator-aaguids 列表，构建时更新；全零或不认识时显示「通行密钥」。

### 3.3 user handle

- 每个账户一个随机 32 字节的 user handle，第一次创建通行密钥时生成，之后不变，账户里的每把通行密钥都用它。
- 不由邮箱、账户 id 或任何可推出的东西派生：提供方和任何拿到断言的人都对不上邮箱。
- 断言里的 `userHandle` 必须等于凭据所属账户的这个值，否则拒绝。
- 同一个提供方里，同一 RP + 同一 `user.id` 再建一把，通常会**替换**旧的（例如 iCloud 钥匙串）。所以添加时带 `excludeCredentials`（本账户已有的凭据 id），避免在同一个提供方里重复建；被替换掉的旧凭据下次登录时报「未知」，客户端调用 `signalUnknownCredential` 让提供方隐藏它（§5.2）。
- 「全部丢失后重置」（新创世）不换 user handle：登录与解密是两套秘密，登录凭据照常可用。

### 3.4 注册与添加

**挑战的格式**（无状态，与设备会话的挑战同一种结构，标签不同）：

```
challenge = 1 ‖ 用途 u8 ‖ issued_ms u64 ‖ rand 16 ‖ HMAC(session_key, "rh/v1 webauthn\0" ‖ 用途 ‖ 绑定 ‖ issued_ms ‖ rand)[..16]
用途：1 = 注册账户，2 = 添加，3 = 登录
绑定：注册 = SHA-256(规范化邮箱) ‖ user.id；添加 = 账户 ‖ 设备；登录 = 空
```

收到应答时重算 MAC、查 2 分钟有效、再在有界的「已用」集合里记一次（过期即忘），重放一律拒绝。

**A. 通行密钥优先的注册**

1. `POST /v1/passkeys/options {purpose: "signup", email, turnstile, invite?, lang}`（不登录）。Hub 依次做今天 `signup` 的前半段：按地址限流 → Turnstile → 邮箱语法与一次性邮箱黑名单 → 邀请码（`invite` 模式）→ 邮箱是否已占用（409 `email_taken`，与今天相同）。都过了才生成一个**新的** `user.id`，回创建选项。
   - Turnstile 放在这一步而不是最后一步，是为了在用户的密码管理器里**建出通行密钥之前**就把会失败的情况挡掉，免得留下一把孤儿通行密钥。
2. 浏览器调 `navigator.credentials.create()`。
3. `POST /v1/signup/passkey {challenge, email, invite?, lang, credential}`。Hub 校验（下表「注册时」一列），建账户：没有密码、user handle = 挑战里绑定的值、邮箱未验证；记下这把通行密钥；入队验证信。回登录会话与 `verified: false`，之后与今天的「查收邮件」流程完全相同（中继文档 §4.1）。
4. 第 3 步仍然失败（例如并发注册抢走了邮箱）时，客户端调用 `signalUnknownCredential` 让提供方隐藏这把通行密钥。

**B. 以后添加一把**

- 只能由**链上活跃的 admin 设备的设备会话**发起：`POST /v1/passkeys/options {purpose: "add"}` → `create()`（带 `excludeCredentials`）→ `POST /v1/passkeys {challenge, label?, credential}`。
- 为什么不许登录会话添加：否则偷到密码（或邮件找回链接）的人就能给自己加一把长期有效的登录凭据。admin 本来就能改写账户里有谁，让它来管登录凭据不会扩大任何权限。普通设备（例如用通行密钥加入、没被授予管理权的手机）也不能添加。
- 每个账户至多 10 把（与套餐无关）；每个账户每天至多添加 10 次。
- 添加后：Hub 给账户的所有设备推 `passkey_added {id, label, by}` 控制消息（与 `DeviceRegistered` 同一机制），并发一封通知信。

**Hub 的校验**

| 检查 | 注册时 | 登录时 |
|---|---|---|
| `clientDataJSON.type` | `webauthn.create` | `webauthn.get` |
| 挑战 | MAC、用途、绑定、2 分钟、未用过 | 同左 |
| `origin` | 在合法 origin 列表里（精确比较）；`crossOrigin` 不为真 | 同左 |
| `rpIdHash` | = SHA-256(RP ID) | 同左 |
| 标志 | UP、UV、AT 置位 | UP、UV 置位 |
| 算法 | -8 / -7 / -257 之一，公钥合法（Ed25519 非小阶、P-256 在曲线上、RSA ≥ 2048 位） | — |
| 凭据 id | ≤ 1023 字节，全局未登记过 | 已登记 → 得到账户；`userHandle` 等于该账户的 user handle |
| 签名 | 不验证明（`none`）；`fmt` 不是 `none` 时只取 authData，丢弃证明 | 对 `authData ‖ SHA-256(clientDataJSON)` 用存下的公钥验签 |
| 计数 | 记下 | 存的或新的不为 0、且新的 ≤ 存的 → 拒绝，并给账户所有设备告警「可能有复制的认证器」；同步型通行密钥一直是 0，不受影响 |
| 记录 | BE、BS、transports、AAGUID | 更新 `last_used_ms`、`sign_count`、BS |

服务端怎么解析与验签是闭源 Hub 的内部选型，不影响 API。

### 3.5 登录

1. 页面加载时 `POST /v1/passkeys/login/options`（不登录，按地址限流）拿到挑战，并在邮箱输入框（`autocomplete="username webauthn"`）上挂一次 `get({mediation: "conditional"})`：用户点输入框，自动填充里直接出现通行密钥。
2. 主按钮「使用通行密钥登录」走一次模态的 `get()`（`allowCredentials` 为空）。
3. `POST /v1/login/passkey {challenge, credential}`。Hub 按上表校验，成功后回：
   ```json
   {"account": "a-…", "token": "…", "expires_ms": 0, "email_verified": true, "verification": "required",
    "passkey": {"id": "pk-…", "label": "iCloud 钥匙串", "unlock": "on" | "off" | "paused" | "none"}}
   ```
   `unlock` 由 Hub 从链上算出（§4.4），只是给新设备的提示，客户端仍以自己验证的链为准。
4. 之后与密码登录完全相同：`POST /v1/devices` 登记设备钥 → 设备签挑战换设备会话（`/v1/session`）。

**失败时**：未知凭据回 401 `unknown_passkey`，客户端调用 `signalUnknownCredential`；其它校验失败一律回 401 `bad_passkey`。通行密钥登录不碰登录锁定计数，也不要求 Turnstile：没有可猜的东西，伪造的断言在验签处就失败。

### 3.6 多把通行密钥、只有通行密钥的账户、密码

- **多把**：每把各自显示提供方、是否同步（BE/BS）、创建与上次使用时间、解锁状态（§7.3）。建议至少两把，或一把加密码。
- **只有通行密钥的账户**：没有密码。用密码登录这样的账户，与「邮箱不存在」走同一条路：照样算一次假哈希，回同一个 `bad_login`，不暴露账户没有密码。
- **设置或删除密码**：`PUT /v1/password {password}`、`DELETE /v1/password`，都要 admin 设备的设备会话，都给所有设备告警并发通知信。删除密码前必须至少有一把通行密钥。
- **删除通行密钥**：`DELETE /v1/passkeys/{id}`，admin 设备；不能删掉账户的最后一个登录凭据（没有密码时的最后一把通行密钥），回 409 `last_credential`。开着解锁的那把同时要撤销它的槽（§4.8）。
- **改邮箱**（以后）：客户端调用 `signalCurrentUserDetails` 更新提供方里显示的名字。
- **列表同步**：设置页打开时调用 `signalAllAcceptedCredentials`，让提供方隐藏 Hub 已不认的凭据（只在支持的平台上，§5.2）。

### 3.7 与邮箱验证、Turnstile、会话模型的关系

| 场合 | 邮箱验证 | Turnstile | 得到什么 |
|---|---|---|---|
| 通行密钥注册 | 照旧必须（`required` 时） | 总要（在创建选项那一步） | 登录会话，未验证前只能 `me`、重发、登出 |
| 密码注册 | 不变 | 不变 | 不变 |
| 通行密钥登录 | 未验证账户仍受同样限制 | 不要 | 登录会话（15 分钟） |
| 密码登录 | 不变 | 不变（失败多了才要） | 不变 |
| 邮件找回登录（§3.9） | — | 总要 | 登录会话，只此而已 |
| 添加通行密钥、设或删密码 | 要已验证 | 不要 | — |

- **会话角色不变。** 通行密钥换来的就是今天的登录会话（不绑定设备），不新增 `HubLogin` 角色，也不能直接换设备会话。设备会话只认设备钥对 `HubLogin{device-session}` 的签名；host 的 Agent 登录与中继的签入角色都不受影响。
- **Hub 侧的权限判断**今天是「链上当前的完整设备」。v3 之后改为「链上活跃、`admin` 标志为真」。本文的新接口一律按 admin 判断。
- **「任何密钥都不对 hub 选的原始字节签名」**（架构文档 §5）仍然成立：通行密钥签的是 WebAuthn 自己的结构（带类型与 origin），而且它只是登录凭据，不是协议里的钥。第二层用的是 PRF，不是这个签名（§4）。

### 3.8 子域与 origin：钓鱼抗性的边界

- 通行密钥绑在 RP ID 上：`droppen-app.com` 之类的钓鱼站拿不到 droppen.app 的断言，也拿不到 PRF 输出。中间人转发断言也没用：`clientDataJSON` 里的 origin 由浏览器写入，Hub 只收 `https://droppen.app`。
- **但 RP ID 允许子域。** `verify.droppen.app` 上的页面可以用 `rpId: "droppen.app"` 调 WebAuthn。Hub 不收它的 origin，所以登录无碍；可 **PRF 输出在客户端**，Hub 的 origin 检查保护不了它。而这个子域正好在加载 Cloudflare 的脚本（Turnstile）。
- 规则（第 1 条已定）：
  1. **Turnstile 页搬到独立的可注册域名**（例如 `droppen-verify.net`；只是换一个配置值，`frame-src` 跟着改）。K1 没有做这一条，见 §8。
  2. 在那之前、以及之后 droppen.app 的**每个子域**的每个响应都带 `Permissions-Policy: publickey-credentials-get=(), publickey-credentials-create=()`。嵌入 Turnstile 的 `<iframe>` 不带 `allow="publickey-credentials-*"`（跨源 iframe 默认就不能调，这里写成测试）。
  3. 中继的源（`*.relay.droppen.app`）只回 JSON 与 WebSocket，不回 HTML，响应同样带上面的头。客户自建的中继（聚合器文档 §9）永远不发 droppen.app 的子域。
  4. 应用页自己的 CSP 已经不放行第三方脚本（中继文档 §10「安全相关」）；新增的 WebAuthn 代码不引入任何第三方库。
- 不需要 Related Origin Requests（`/.well-known/webauthn`）：只有一个源。以后若真要第二个域名再说（Chrome/Edge 128+、Safari 18 支持，Firefox 未定 [20][21]）。

### 3.9 全部通行密钥都丢了、限流、账户枚举

**丢了全部通行密钥**

- 还有一台 admin 设备：它不需要登录（设备会话靠设备钥换），直接在设置里添加一把新的。
- 没有任何 admin 设备：
  - 有密码 → 密码登录；
  - 没有密码，或密码也忘了 → **邮件找回登录**：`POST /v1/login/email {email, turnstile}` 总是回 202（不论账户是否存在）；信里的链接 `https://droppen.app/signin#t=<token>`，token 用与验证信同一种无状态格式、标签 `rh/v1 login-link`，15 分钟有效、只能用一次（Hub 记下已用 token 的摘要到过期）；`POST /v1/login/email/redeem {token}` 换一张**登录会话**（登录方式记为邮件链接）。
  - 登录会话只能登记设备。这台新浏览器要进链，仍要扫码配对、恢复词，或者（开了第二层时）用别的通行密钥解锁。邮箱本身永远加不了通行密钥、设不了密码，也解不开任何东西。
  - 什么都没有了：架构文档 §1 的「全部丢失后重置」（新创世），然后再添加通行密钥。
- 这也补上了今天缺的「忘记密码」：没有密码重置，只有「用邮件登录，再从 admin 设备上改」。

**限流**（并入中继文档 §4.4 的表）

| 路由 | 每地址 | 每账户 | 其他 |
|---|---|---|---|
| `POST /v1/passkeys/options {signup}` | 与注册同一个桶：每小时 5 次、每天 20 次 | — | Turnstile |
| `POST /v1/passkeys/login/options` | 每分钟 60 次 | — | 无状态 |
| `POST /v1/login/passkey` | 与 `/v1/login` 同一个桶：每 10 分钟 30 次 | — | 不计锁定 |
| `POST /v1/passkeys/options {add}`、`POST /v1/passkeys` | — | 每天 10 次 | admin |
| `POST /v1/login/email` | 每小时 10 次 | 每小时 3 封、每天 5 封 | Turnstile；总是 202 |
| `POST /v1/login/email/redeem` | 每分钟 30 次 | — | token 只用一次 |

**账户枚举**：通行密钥登录不带任何标识，未知凭据只说明「这把凭据不认识」；邮件找回总是 202；注册仍会回 `email_taken`（中继文档 §4.1 已接受：在 Turnstile 与限流之下成本可以接受），通行密钥注册也一样，且发生在建出通行密钥之前。

### 3.10 新增的 Hub 接口（写进 `docs/hub-api/`）

| 路由 | 鉴权 | 说明 |
|---|---|---|
| `POST /v1/passkeys/options {purpose: "signup", email, turnstile, invite?, lang}` | 无 | → `{challenge, options}`（`PublicKeyCredentialCreationOptionsJSON`） |
| `POST /v1/signup/passkey {challenge, email, invite?, lang, credential}` | 无 | → 与 `/v1/signup` 相同的 `SignedIn` |
| `POST /v1/passkeys/options {purpose: "add"}` | admin 设备会话 | → `{challenge, options}`，带 `excludeCredentials` |
| `POST /v1/passkeys {challenge, label?, credential, prf?}` | 同上 | → `{id}`；推 `passkey_added` |
| `GET /v1/passkeys` | 设备会话 | → `[{id, label, provider, backup_eligible, backup_state, prf, created_ms, created_by, last_used_ms, cred_hash}]` |
| `PATCH /v1/passkeys/{id} {label}`、`DELETE /v1/passkeys/{id}` | admin | 删除最后一个登录凭据回 409 `last_credential`；推 `passkey_removed` |
| `POST /v1/passkeys/login/options` | 无 | → `{challenge, options}`（`PublicKeyCredentialRequestOptionsJSON`） |
| `POST /v1/login/passkey {challenge, credential}` | 无 | → `SignedIn` + `passkey {id, label, unlock}` |
| `PUT /v1/password {password}`、`DELETE /v1/password` | admin | 删除前必须有通行密钥 |
| `POST /v1/login/email {email, turnstile}`、`POST /v1/login/email/redeem {token}` | 无 | 总是 202；→ `SignedIn` |
| 控制消息 `passkey_added`、`passkey_removed` | — | 推给账户的所有设备 |
| 钥环名 `pk/m/<host>/<MK 轮>/<发送者>/<槽标签>/<写入者>` | — | 写入与读取规则见 §4.3、§4.6 |

错误形状沿用 `{error, message, …}`。`/v1/me` 增加 `credentials: {password: bool, passkeys: n}`。

## 4. 第二层：用通行密钥解锁新设备（PRF，可选开启）

![用通行密钥解锁新设备：admin 开启时登记槽并按 host 封 MK；新设备登录、解锁、待定；admin 输入两位数批准，或 24 小时后自行激活](passkeys/unlock.svg)

### 4.1 是什么，代价是什么

- **是什么**：在一台新手机或新浏览器上，只用同步过来的通行密钥（Face ID / 触控 ID）加入账户，不扫码，不输入 12 个恢复词。Bitwarden 的「用通行密钥登录并解密」是同一个思路 [36][37]。（1Password 的「用通行密钥解锁」不用 PRF，靠的是受信设备 [38]。）
- **代价**：能**使用**这把通行密钥的人就能解锁。同步型通行密钥由提供方端到端加密同步，提供方这家公司自己用不了它；但掌握了用户同步账户的人可以：例如 Apple 账户加一台受信设备的锁屏密码，Google 账户加 Google 密码管理工具的 PIN 或旧设备的锁屏，1Password 账户密码加 Secret Key。这是一种交给第三方的托管，与「不做任何密钥托管」相反。
- **所以**（已定）：
  - 默认关闭，**按把**开启（只对用户勾选的那把通行密钥生效）；开启时显示 §6.3 的文案并要求勾选。
  - 用它加入的是**普通设备**（`flags = 0`），先待定：批准或 24 小时后才生效，期间全体告警（§4.6）。它永远拿不到管理权，除非已有的 admin 授予（§4.10）。
  - 扫码配对和恢复词照旧可用，仍是默认与最后的退路；开不开第二层都不影响它们。
  - 安全钥匙（YubiKey 等，BE = 0）上的通行密钥不同步，开启它就等于多了一把「实体恢复钥」，没有同步账户这一层；文案相应改写（§6.3）。
- **PRF 从不是唯一的那一份。** WebAuthn 规范的编辑之一提醒过：拿 PRF 加密数据，通行密钥一删数据就没了 [39]；Google 密码管理工具的 PIN 一重置，所有通行密钥也会被删 [41]。本文里 PRF 只派生「又一条拿到 `MK` 的路」，`MK` 本身仍在各 host、各 admin 和钥环里，删掉通行密钥只是少了这条路。

### 4.2 salt 与派生

**槽**：每开启一次解锁，就在链上建一个**槽**：`slot = "p-" + 随机 12 字符`，id 永不复用。

**salt**：开启的 admin 设备生成随机 32 字节 `salt_nonce`，写进 `PasskeySlotSet`。PRF 的输入是

```
prf_input = "rh/v1 passkey-unlock\0" ‖ salt_nonce          （浏览器再自行做 SHA-256("WebAuthn PRF" ‖ 0x00 ‖ prf_input) [17]）
```

- 每个槽一个随机 salt（Yubico 的建议也是每把凭据一个随机 salt [17]），所以同一把通行密钥在作废后可以换 salt 重开（§4.7），不必重建通行密钥。
- 新设备**从链上的 `PasskeySlotSet` 取 salt**，不另信 Hub 给的字段。Hub 给错链或错 salt，派生出的 `PAK` 就对不上链上的公钥，新设备拒绝（§4.6 第 3 步）。

**派生**（Rust 在 `rh_proto::chain`、TS 在 `W/lib/e2e/passkey-unlock.ts`，双向互通向量）：

```
prf  = PRF(credential, prf_input)                                       32 字节，只在内存
seed = HKDF-SHA256(salt = "rh/v1 passkey", ikm = prf,
                   info = "signing key\0" ‖ str16 账户 ‖ str16 槽)       32 字节
PAK  = Ed25519(seed)                                                     通行密钥授权钥
twin = X25519(clamp(SHA-512(seed)[..32]))                                PAK 的孪生钥，与 RSK 的孪生钥同一构造
```

- `info` 带上账户和槽，做域分隔：同一份 PRF 输出放到别的账户或别的槽，派生出的是另一把钥。
- `prf` 与 `seed` 用完立即清零；`PAK` 与 `twin` 只以不可导出的 `CryptoKey` 活在这一次操作里，**任何设备都不存**。
- **为什么不是「HKDF → 对称 KEK → 包钥」**：对称 KEK 要求每次给新一轮钥加包时都重新算 PRF，也就是每次移除设备都要有人做一次 Face ID。封给孪生钥的公钥则任何 admin 都能做，与今天恢复包的做法相同（`sealForRecovery`）。Bitwarden 为同一个理由给每把凭据配一对 RSA 钥，私钥用 PRF 派生的钥加密存服务端，用户钥封给公钥 [36]；本文的私钥直接由 PRF 确定性派生，服务端连加密的私钥都不存。

**证明**：`PAK` 签

```
"rh/v1 passkey-proof\0" ‖ str16 账户 ‖ AK 32 ‖ PAK 32 ‖ str16 槽 ‖ cred_hash 32 ‖ salt_nonce 32
cred_hash = SHA-256(credential_id)
```

与恢复钥的证明（M2 安全复审 H2）同一个目的：只有算得出 PRF 的人才能把这把 `PAK` 放进链里，而且只能放进以这个 `AK` 为根的账户。Hub 用自己的 `AK` 包住账户真实（公开）的 `PAK`，新设备一验就知道。

### 4.3 封装什么：按 host 封 `MK`

v3 里 `UK` 只给 admin（聚合器文档 §3.3），而通行密钥只加入普通设备，所以通行密钥包**不含 `UK`**，按 host 封 `MK`：

- **封**：每台当前 host 的每一个链上已提交、封装者持有的 `MK` 轮次。新设备有了它们就能读每台 host 的全部离线历史，与 admin 投递给普通设备的那一套相同（信箱用途 2）。
- **不封**：`UK`（普通设备不持）；设备钥（每台设备自己生成）；`RK`（通行密钥与恢复词是两把互不牵连的钥）。
- **读不到的**：`UK` 加密的 UI 文档（布局、待办，等它们上云时）。聚合器文档 §3.3 的打算是到时给非 admin 设备一把单独的「文档钥」；它一旦定下，就照同样的方式封进通行密钥包。
- **封法**与恢复包相同：带发送方认证地封给 `twin` 的公钥，上下文 `{purpose: machineKey, account, sender: 封装的 admin, recipient: "passkey:<槽>", host, epoch}`。
- **钥环名** `pk/m/<host>/<MK 轮>/<发送者>/<槽标签>/<写入者>`，槽标签 = `PAK` 的前 8 字节 hex。Hub 的写入规则照 `rk/` 与 `mk/`：
  - 只有链上活跃的 admin 能写，名字以自己结尾；
  - host 必须是链上当前的 host，`MK` 轮次必须已提交；
  - 槽必须是链上当前的槽。
  - 钥环的条数上限（中继文档 §10「钥环上限」）加一项：每个当前槽、每台 host、每个 `MK` 轮次 1 条。回收规则与 `mk/` 相同：同一写入者被取代的副本才删。
- **打开时**：发送者必须是链上的 admin（今天 `openFromChain` 的规则），解出的 `MK` 必须等于链上 `KeyEpochs` 的提交（`checkMachineKey`）。被移除的设备或 Hub 塞不进自选的 `MK`。

### 4.4 链条目：v3 的 16–18 号与 `Passkey(slot)`

编码与种类号以聚合器文档 §3.2 为准，这里只列语义：

| 条目 | 谁签 | 效果 |
|---|---|---|
| 签名者 `Passkey(slot)` | `PAK` | 只能签两种条目：`flags = 0` 的待定 `AddDevice3`，以及它自己这个槽的 `PasskeyActivate` |
| 16 `PasskeySlotSet { slot, key: PAK, proof, cred_hash, salt_nonce, grants: u8, self_activate: bool }` | admin | 新建一个槽。`grants` 必须为 0（只能加入普通设备，§4.10；字段留着免得以后改编码）；`self_activate` 允许 24 小时自行激活；记下 `set_by` = 签名的 admin |
| 17 `PasskeySlotRemove slot` | admin | 槽作废 |
| 12 `AddDevice3`，签名者为 `Passkey(slot)` | `PAK` | `flags` 必须为 0；新设备进入**待定**，记 `via_passkey = slot`。同一个槽同时只有一台待定设备，新的加入顶替旧的 |
| 18 `PasskeyActivate id` | `Passkey(slot)`，且该设备的 `via_passkey` 就是这个槽 | 待定 → 活跃（仍是普通设备）。要求槽仍在、`self_activate` 为真，且**验证者自己**首次看到那条加入已满 `PASSKEY_DELAY_MS`（24 小时，`ChainClock`，规则同恢复） |
| `ApproveDevice`（v3 沿用） | admin | 待定 → 活跃。要同时给管理权，就在同一批里写 `[ApproveDevice(id), SetAdmin{id, true}]`；签名者不是被授权的设备，不违反「不能给自己加权限」 |
| 15 `RemoveDevice3`（作废规则） | admin | 被移除的设备若 `via_passkey == s`，或是槽 `s` 的 `set_by`，槽 `s` 一并作废（同今天的 `recovery_set_by`） |

其它规则（聚合器文档 §3.2「通行密钥」一段、§3.6 第 17 条）：

- 槽 id 与设备 id 一样永不复用；同一时刻至多 8 个槽；同一个 `cred_hash` 至多一个当前槽。
- `PasskeySlotSet` 的 `key` 必须是合法、非小阶、规范编码的 Ed25519 公钥，证明必须成立。
- 被顶替的待定设备算过 PRF，但它的 `PAK` 与新加入者的相同，所以不触发作废；Hub 侧同时吊销它的会话（与中继文档 §10「新的恢复加入顶替还在等待的旧加入」一致）。
- 槽作废时，靠它待定的设备仍然待定，但再也不能自行激活；admin 可以批准或移除。
- 告警（聚合器文档 §3.2「告警」）：16、17、槽签的加入、18 与批准，所有设备的界面、所有 host 的日志与推送；槽签的加入要求确认，任一 admin 可以否决。按条告警、批量里生效的每一条都告警（架构文档 §9「账号变更提醒和恢复否决」）。
- 「压后」（`heldBack`）与恢复相同：太早的 `PasskeyActivate` 及其后的条目被压后，到期后收下；期间只允许批准或移除那台设备（架构文档 §9「N1」的规则照搬）。
- 激活与套餐：自行激活不受 `devices.max` 约束（丢光设备的人不能被旧设备占着名额挡在门外）；admin 的批准以 `devices.max + 1` 为限。与恢复相同。

**发版**：与 v3 同一个门槛（聚合器文档 §3.5）。SC-1 解码与应用 16–18 号；在 droppen.app 开始写 v3（SC-5）之前，浏览器不提供「开启解锁」；门槛不满足时 Hub 回 409 `chain_v3_not_ready`，界面列出卡住的 host。

### 4.5 开启（在一台 admin 设备上）

1. 设置 → 登录与解锁 → 某一把的「解锁：关」→ 弹出 §6.3 的说明。只有 `prf` 不为 `no`、账户已能写 v3、本设备是 admin 时这个开关可用。
2. 勾选并点「用 Face ID 开启」：生成槽 id 与 `salt_nonce`，调用
   ```js
   navigator.credentials.get({ publicKey: {
     challenge, rpId: "droppen.app", userVerification: "required",
     allowCredentials: [{ type: "public-key", id: credentialId }],
     extensions: { prf: { evalByCredential: { [credentialIdB64u]: { first: prfInput } } } } } })
   ```
   挑战用 §3.4 的「登录」用途；断言照常发给 Hub 校验（证明这次手势确实是这把通行密钥，并更新使用时间），但 Hub 不需要它来做任何决定。
3. 没有 `prf.results.first` → 把这把标为 `prf: no`，提示「这把通行密钥在这台设备上不支持解锁」，结束。
4. 派生 `PAK` 与 `twin`，签证明，一次 `POST /v1/chain` 提交 `PasskeySlotSet`。
5. 把持有的每台 host 的每一轮 `MK` 封给 `twin`，写 `pk/m/…`（`ensurePasskeyPackages`，与 `ensureRecoveryPackages` 同一套，每台 admin 每次加载都补缺；新 host 加入时也补）。
6. 签 `ATTEST`。界面显示「解锁：开」。

**合并成一次手势**：在设置里「添加通行密钥」时若用户同时勾了「也用它解锁」，先生成槽与 salt，把 `prf_input` 作为 `create()` 的 `prf.eval.first`。认证器在创建时就给出结果（iCloud 钥匙串、Google 密码管理工具、1Password、YubiKey 5.8+ 等 [3][16]）就省掉第 2 步；没给就照上面多一次 `get()`。

### 4.6 新设备加入

**第 1 步：登录**。新浏览器「使用通行密钥登录」（§3.5）→ 登录会话 → `POST /v1/devices` 登记设备钥 → 设备会话 → `GET /v1/chain`。登录应答里的 `passkey.unlock` 是 `on` 时，下一屏直接是「用 Face ID 解锁」。

- 用户选的那把没开解锁、而账户里另一把开了：提示「这把通行密钥（1Password）没有开启解锁。用 iCloud 钥匙串里的那把？」，下一步的 `allowCredentials` 换成那一把（凭据 id 来自 `GET /v1/passkeys`）。

**第 2 步：解锁**。按链上的 `cred_hash` 找到槽，用它的 `salt_nonce` 调用与 §4.5 第 2 步相同的 `get()`，拿到 PRF 输出，派生 `PAK`、`twin`。

**第 3 步：核对，然后才信这条链**。新设备此时还没钉任何创世，链是 Hub 给的。只有下面都成立才继续：

1. 链从创世起能完整验证；
2. 槽是链上当前的槽，`PAK` 公钥等于 `PasskeySlotSet` 里的 `key`；
3. 证明成立，即 `PAK` 签过这条链创世里的 `AK`。

第 3 条挡住「Hub 用自己的 `AK` 另造一条链、包住真实的 `PAK`」：Hub 没有 PRF 输出，签不出这个证明。都成立后钉住创世与链头（与恢复的 H2 修复同一时机）。

> 落地时第 4 步和 5a 步改了：不显示两位数，待定的新设备显示普通配对页，另一台完整设备用配对（8 位数字码或二维码）批准，只批准配对认证过的那对钥。见 §8「K2 与『新设备用通行密钥加入』怎么落地的」。

**第 4 步：待定**。以 `Passkey(slot)` 签 `AddDevice3{本设备, flags: 0}`，`POST /v1/chain`。屏幕显示：

- 一个两位数的配对码：`SHA-256("rh/v1 join-code\0" ‖ str16 账户 ‖ str16 设备 ‖ sign_key ‖ dh_key)` 的前两字节按大端读出、模 100；
- 「在你的另一台设备上输入这个数字来批准」；
- 允许自行激活时，再加一行「或者 明天 14:02 之后，在这台设备上再用 Face ID 激活」。

**待定期间什么都读不到**（已定）：

- 待定设备不是活跃设备：host 不与它握手，Hub 不让它 `SUB` 流、不给页（今天对待定设备就是这样）；
- **新增**：`GET /v1/keys` 不把 `pk/…` 返回给链上不活跃的会话。所以小偷连 `MK` 都拿不到；
- 恶意 Hub 不受这些约束，见 §6.1。

**第 5a 步：批准**（常见路径）。

- 每台 admin 设备上出现告警条（今天的 `PendingRecoveryBanner` 推广为「待定设备」横幅），每台 host 也推送并发 Bark，点开直达批准页。
- 批准页要求**输入**新设备上的两位数（不是三选一）。连错 3 次，这台待定设备的批准锁住，页面建议「拒绝」。这防的是「小偷不停发起加入，赌你顺手点了批准」，不是防 Hub：两边的公钥已经由 `PAK` 的签名绑死。
- 批准页有一个「也让它管理账号」的勾选项，默认不勾（§4.10）。
- 输入对了：签 `ApproveDevice`（勾了就同批加 `SetAdmin{id, true}`）与 `ATTEST`，并把每台 host 的每一轮 `MK` 用信箱用途 2 带发送方认证地封给新设备（聚合器文档 §3.3 给普通设备投 `MK` 的同一段代码）。勾了管理权时，再按今天的做法用用途 1 封 `UK`。
- 新设备收到信、链上看到自己已活跃 → 解开 `MK` → 读历史、开通道。不需要第二次 Face ID。
- **新鲜度**：作为普通设备，它的控制类操作按聚合器文档 §3.4 的规则——任一当前 admin 在 **7 天**内为链头作过证即可；它自己不作证。所以一周内在任一 admin 设备上打开一次 Droppen 就够了。

**第 5b 步：没人批准**（丢光了所有 admin 设备）。

- 满 24 小时后，新设备再做一次 §4.5 第 2 步的 `get()`，以 `Passkey(slot)` 签 `PasskeyActivate`。
- 每个验证者按自己首次看到加入的时间判断。太早了：Hub 拒收，host 和其它设备压后。
- 收下之后它是活跃的普通设备：Hub 开始给它 `pk/m/…`，它用 `twin` 打开各 host 的各轮 `MK`。
- 这时账户里可能已经没有任何 admin 能作证，7 天的控制窗口会过期，控制降级为只读（聚合器文档 §3.4「体验」）。这正是 §4.10 说的「通行密钥恢复不了管理」的代价；恢复词能补上。

**第 5c 步：拒绝**。任一 admin 点「不是我」：

- 签 `RemoveDevice3(待定设备)`。它算过 PRF，按作废规则这个槽同时作废，Hub 在同一事务里清扫这个槽的 `pk/…`。
- 拒绝页写明：如果不是你，有人可能掌握了你的同步账户。请先去保护那个账户，再删除并重建这把通行密钥（§6.4 文案）。
  - 小偷手里有的是**通行密钥本身**，不只是一次 PRF 输出。所以换 salt 重开挡不住他，必须先把通行密钥从他手里拿回来。

### 4.7 密钥轮换、槽作废与「过期但不失守」

| 情形 | 发生什么 | 要不要手势 |
|---|---|---|
| 移除一台**没算过**这个槽的 PRF 的设备 | 轮换（架构文档 M2 安全复审 C1 的顺序；移除非 admin 时只轮换 `MK`，聚合器文档 §3.3）里多一步：把每台 host 的新一轮 `MK` 封给每个当前槽的 `twin`，写 `pk/m/…`。轮换报告多一行「通行密钥解锁：已更新（2 个）」 | 不要 |
| 新 host 加入、或 `SetHost true` 带来新一轮 `MK` | 同上，由签这批条目的 admin 补封 | 不要 |
| 移除一台**算过**的设备（登记这个槽的 admin，或用它加入的设备） | 链上该槽作废；Hub 在同一事务里清扫它的 `pk/…`；轮换照常进行，但不再封给这个槽 | 重开时要一次 |
| 轮换中途页面崩溃，新一轮没封给槽 | 每台 admin 加载时 `ensurePasskeyPackages` 补齐；补齐前用它解锁的新设备拿到的是旧几轮，按「缺少密钥」发用途 4 的索钥请求，在线的 admin 自动回 `MK` | 不要 |

**作废后的提示**

- 所有 admin 设备的设置页与顶部横幅显示：「通行密钥『iCloud 钥匙串』的解锁已暂停：被移除的设备用过它。在这台设备上用 Face ID 重新开启。」按钮是「重新开启」和「以后」；「以后」只把横幅收进设置页，不再弹出。
- 只在**这台设备上可能有这把通行密钥**时给按钮：平台认证器可用，且这把是同步型（BE = 1）或就是在这台设备上创建的；否则写「在有这把通行密钥的设备上重新开启」。用手机扫码的跨设备方式（hybrid）也可以，但 PRF 能否经它传过来取决于平台（§5.1）。
- 重开 = `PasskeySlotRemove`（作废时链已自动完成则省略）+ 一次新的 §4.5：新槽 id、新 salt、同一把通行密钥。旧的 PRF 输出对新槽毫无用处。

**没人重开时**

- 通行密钥**登录**照常可用。
- 新设备在第 1 步后看到的不是「用 Face ID 解锁」，而是「这个账户的通行密钥解锁已暂停。请用另一台设备扫码配对，或输入恢复词。」
- 永远不会出现「用旧槽解出了新数据」：作废的槽在链上已不存在，Hub 已清扫它的包，旧包里只有被移除设备本来就有的旧轮次。

### 4.8 删除通行密钥

- 设置里删除一把开着解锁的通行密钥：同一操作里先签 `PasskeySlotRemove`（Hub 清扫 `pk/…`），再 `DELETE /v1/passkeys/{id}`。
- 客户端调用 `signalUnknownCredential`，让提供方把它从账户选择器里拿掉（支持时，§5.2）。用户还需要自己去提供方里删除；界面给出路径（例如「设置 → 密码 → droppen.app」）。
- 靠这个槽待定的设备列在确认页里，默认一并移除。
- Hub 删凭据行与链上撤销槽是两件事：前者断登录，后者断解锁。界面把它们合成一个「删除」。

### 4.9 (a)、(b)、(c)：已定 (c)

| | (a) 授权钥，立即生效 | (b) 当作恢复 | **(c) 混合（已定）** |
|---|---|---|---|
| 机制 | admin 在链上登记 `PAK`；`Passkey(slot)` 签 `flags = 0` 的 `AddDevice3`，新设备立即活跃 | 通行密钥包住恢复钥 `RK` 本身；新设备解出 `RK` 后走现成的恢复加入 | 链上独立登记 `PAK`（同 a），但它只能签待定的 `AddDevice3{flags: 0}` 和 24 小时后的 `PasskeyActivate`（同 b 的节奏）；另加两位数批准与 Hub 扣包 |
| 新设备是什么 | 普通设备 | 恢复给什么就是什么：admin（v3 的 `RecoveryJoin` 必须 admin） | 普通设备；管理权只能由已有 admin 用 `SetAdmin` 授予 |
| 协议改动 | v3 的 16–18 号 | **无** | v3 的 16–18 号 |
| 新设备何时能读 | 立即 | 立即：恢复包对任何已登记设备可读 | 批准后；没人批准时 24 小时后 |
| 新设备何时能控制电脑 | 立即 | 批准后或 24 小时后 | 批准后或 24 小时后 |
| 同步账户被盗 + 诚实 Hub | **立即**读全部历史、开终端；告警只能事后 | 立即读全部历史（解出 `RK` = 拿到恢复词）；24 小时后还拿到 **admin**，能移除你的其它设备 | 24 小时内读不到也控制不了；全体告警，任一 admin 否决即作废这个槽；即使没人否决，也只是普通设备，你的 admin 随时能移除它 |
| 同步账户被盗 + 恶意 Hub | 同上 | 同上 | 立即读（Hub 给流和包）；控制仍等 24 小时（验证者用自己的时钟） |
| 两把钥的关系 | 独立 | **绑死**：通行密钥泄漏 = 恢复词泄漏，要换恢复词；开启前必须有恢复词，而且要把 12 个词重新输一遍（没有设备存 `RK`）；用通行密钥加入的设备被移除，会按现有规则清掉恢复钥 | 独立：各自吊销、各自重开 |
| 有另一台 admin 在场 | 1 次 Face ID | 等批准 | 2 次 Face ID + 另一台输两位数 |
| 丢光所有 admin 设备 | 立即用，管不了账号 | 24 小时后用并能管 | 24 小时后用，管不了账号（要恢复词） |

**为什么是 (c)：**

1. **爆炸半径。** Droppen 的控制意味着在用户电脑上驱动 AI CLI 和终端，也就是任意代码执行。(a) 让「Apple 账户被盗」直接变成「所有电脑被远程执行」，告警只能事后看到。这是第二层唯一真正新增的风险，必须挡在控制之前。
2. **读也能挡一截。** (b) 的恢复包对任何已登记设备开放（恢复要先读包才能加入）。(c) 让新设备先用 `PAK` 签名进链，Hub 就可以把包扣到它活跃为止。对「只偷了同步账户」的人，24 小时内什么都拿不到。
3. **不给管理权。** (b) 让偷到同步账户的人最终拿到 admin，可以把你的其它设备移出账户、把你锁在外面。(c) 的槽只能加入普通设备，你的任何一台 admin 随时能移除它。
4. **不把两把钥绑在一起。** (b) 让通行密钥成了恢复词的另一种存放方式：提供方那边一出事，就得换恢复词、重抄 12 个词；开启时还得把词重新输一遍。(c) 里两者互不牵连，通行密钥作废只需要一次 Face ID 重开。
5. **常见路径并不慢。** 有另一台设备在场时，批准就是输两位数，比扫码还省事，而且不必两台设备在一起：推送到手的那台就能批。丢光设备的情形本来就是恢复，24 小时可以接受，与恢复词一致。
6. **复用。** 待定、24 小时、压后、告警、否决、批准，全部是恢复已有的机制，(c) 只是多一个签名者。

不提供「立即生效」的高级选项：它把第 1 条的风险交给用户在一个开关上判断，而大多数人分不清这个开关的后果。

### 4.10 通行密钥永不授予管理权（已定）

`PasskeySlotSet` 的 `grants` 必须为 0，`Passkey(slot)` 签的 `AddDevice3` 的 `flags` 必须为 0，链规则直接拒绝更多（聚合器文档 §3.6 第 17 条）。管理权只有三条来路：

1. 创世（`Genesis3`）；
2. 已有的 admin 授予：批准通行密钥加入时勾上「也让它管理账号」（同批的 `SetAdmin{id, true}`），或者以后在「我的设备」里拨开关；
3. 恢复词：v3 的 `RecoveryJoin` 必须是 admin（聚合器文档 §3.2）。

**理由**

- 管理权是改写「账户里有谁」：加删设备、开关管理权、提交钥匙轮次、登记和撤销通行密钥槽、注册新电脑；它还持 `UK`。拿到它的人可以把真主人的其它设备全部移除。这比普通设备多出的正是「把你锁在外面」。
- 通行密钥是第三方托管的。让它恢复「用」，可以用 24 小时的延迟和告警来兜；让它恢复「管」，延迟一过就再没有补救（除了在每台电脑上 `agent re-enroll`）。
- 恢复词是用户自己的秘密，没有第三方。所以「管」的恢复留给它。

**代价**

- 丢光所有 admin 设备、又没有恢复词的人：用通行密钥 24 小时后能读全部历史，也能控制电脑，但最多一周（没有 admin 作证，控制的 7 天窗口会过期，之后只读）。他管不了账号：不能移除丢掉的设备，不能注册新电脑，也不能重开作废的槽。
  - 若丢掉的设备只是丢了，这些都无大碍；若是被偷了，偷到的人本来就有 admin，通行密钥给不给管理权都挡不住他否决。这时的出路与今天相同：在每台电脑上 `agent re-enroll` 换信任根。
  - 所以开启文案建议同时保留恢复词（§6.3）。
- 普通设备不作证，也不持 `UK`，所以读不到 UI 文档（§4.3）。希望这台手机也能管理账号的用户，批准时勾上即可。

### 4.11 本机 Droppen 安装与 `/link` 弹窗

**本机 Droppen 安装不用通行密钥**，也不需要：

- 它不在 droppen.app 的源上，没有可用的 WebAuthn；本机页面 `http://127.0.0.1:8700` 的 RP ID 也对不上 droppen.app。
- 它经本机登录（聚合器文档 §4）加入，可运行 AI，默认不可管理账号。别的 host 的 `MK` 照旧由 admin 投递，缺钥时自动索取（聚合器文档 §3.3）。
- 它控制别的设备时，与用通行密钥加入的手机一样，按聚合器文档 §3.4 的规则：任一 admin 7 天内作过证即可。
- 通行密钥也不能把它加进来：`Passkey(slot)` 只能加入 `flags = 0` 的设备，而可运行 AI 只能在注册证明之后由 admin 签（聚合器文档 §3.2 签名规则第 5 条）。

**`/link` 弹窗**（聚合器文档 §4.3 第 3 步）：

- 弹窗在 droppen.app 的源上，是顶层窗口，可以直接调 WebAuthn（不受跨源 iframe 的权限策略限制）。登录页与 §7.2 相同，通行密钥优先。
- 同意页要由 **admin** 设备签（`AddDevice3{host}` 只能由 admin 签）。弹窗所在的浏览器还不是 admin 时，在「新账户 / 扫码或口令配对 / 恢复词」之外多一条「**用通行密钥解锁**」（只有账户有当前槽时出现）：
  - 它与恢复词一样先待定。在手机上输入两位数，并勾上「也让它管理账号」批准，弹窗才继续到同意页。
  - 没勾管理权的批准不够签同意页；弹窗照实说明「这台浏览器只能远程控制，要在能管理账号的设备上添加电脑」。
  - 24 小时的路径下 attempt 会先过期，而且自行激活的设备不可管理账号，所以这条路只能用批准完成；本机页面显示「批准后再点一次登录」。不另造捷径。
- 同意页、注册码交接、上链、交钥一概不变。弹窗不能设 COOP（聚合器文档 §4.4），WebAuthn 也不需要它。

**Tauri 应用**（聚合器文档 §7.3，以后）：见 §5.4。

## 5. 支持矩阵、探测与降级

资料截至 2026-10-17，来源与日期见 §12。WebAuthn Level 3 已于 2026-08-25 成为 W3C 推荐标准 [1]。

### 5.1 矩阵

| 平台 / 提供方 | 通行密钥登录 | PRF（`get()`） | PRF（`create()`） | 说明 |
|---|---|---|---|---|
| Safari 18+（iOS/iPadOS 18、macOS 15）+ iCloud 钥匙串 | 能 | 能 | 能 | Safari 18.0 加入 PRF [2][3]。扫码（hybrid）：18.0–18.1 没有，之后一度与本机结果不同，Apple 工程师说 iOS 18.4 / macOS 15.4 修好 [7][8]；有一份 2026 年的报告仍不一致，未证实 |
| Safari + 安全钥匙 | 能 | 26.4 起 | 26.4 起 | WebKit bug 311099：USB/NFC 钥匙曾返回未解密的 hmac-secret，修复在 STP 241 确认，哪个正式版带上未确认 [5][6] |
| Chrome / Edge + Google 密码管理工具（Android、桌面） | 能 | 能 | 能 | PRF 自 Chrome 116 [10]；桌面 132+ [4]。Chrome 自己的「配置文件」认证器不给 PRF（关了保存通行密钥、或被扩展接管时）[3][4] |
| Chrome 132+（macOS）+ iCloud 钥匙串 | 能 | 能 | 能 | [4] |
| Windows Hello（Windows 11 24H2/25H2，2026 年 2 月更新 KB5077181 起） | 能 | Chrome/Edge 146+、Firefox 148+ | Chrome/Edge 147+ | Windows 10 没有；微软没有文档，用户报告不一；创建时要带 PRF 输入，Windows 才建出支持 PRF 的凭据 [3][11][12] |
| Microsoft 密码管理器 | 能 | 不能（`NotAllowedError`） | 能 | [3] |
| Firefox 桌面 | 能 | 135 起 Windows/Linux，139 起所有桌面平台 | 能 | [9][13] |
| Firefox Android 149+ | 能 | 来源冲突 | 能 | [9][14] |
| 1Password | 能 | 能 | 能 | 2024 年 7 月起 beta [15]；2026 年 8 月实测可用 [3]；正式版状态未经 1Password 确认 |
| Proton Pass | 能 | 能 | 能 | [3] |
| Bitwarden（作为提供方） | 能 | 部分（iOS 上没有） | 部分 | [3] |
| Samsung Pass | 能 | 能 | 不能 | [3] |
| Dashlane、NordPass、KeePassXC（正式版） | 能 | 不能 | 不能 | [3] |
| YubiKey 5（固件 5.2+ 有 hmac-secret；5.8+ 才能在创建时求 PRF） | 能 | 能 | 5.8+ | 否则创建后紧接一次 `get()` [16][17] |
| Tauri 的 webview（WKWebView、WebView2、Android WebView） | 页面源不在 droppen.app 时不能 | 未确认 | 未确认 | 见 §5.4 |

MDN 的兼容数据把 Safari 的 `get()` PRF 标为不支持，与其它所有来源相反，像是数据错误 [9]。

### 5.2 探测

| 要知道的 | 怎么问 | 可靠性 |
|---|---|---|
| 浏览器有没有 WebAuthn | `window.PublicKeyCredential` | 可靠 |
| 有没有平台认证器 | `isUserVerifyingPlatformAuthenticatorAvailable()`（Chrome 67、Firefox 60、Safari 13 起）[9] | 可靠 |
| 能不能自动填充 | `isConditionalMediationAvailable()`（Chrome 108、Firefox 119、Safari 16 起；Android WebView 恒为 false）[9] | 可靠 |
| 浏览器支不支持 PRF | `getClientCapabilities()["extension:prf"]`（Chrome/Edge 133、Firefox 135、Safari 17.4 起）[9][18] | **只反映浏览器，不反映认证器**：Chrome 对 Bitwarden、Dashlane 报支持而实际没有结果 [3] |
| 这把凭据支不支持 PRF | 创建时的 `getClientExtensionResults().prf.enabled` [19] | 只说明凭据支持；同一把凭据换个浏览器或经扫码，结果仍可能没有 |
| 这次能不能真的解锁 | 开启或解锁那次 `get()` 有没有 `prf.results.first` | **唯一的准数** |
| Signal API | `PublicKeyCredential.signalUnknownCredential` 等是否存在（Chrome/Edge 132、Safari 26 起；Firefox 没有）[9][23][24] | 存在即调用；Safari 的能力位不可靠 [25]，所以按函数存在与否判断 |

规则：

- Hub 里的 `prf` 只是「最近一次所见」，用来决定界面上默认显示什么，**从不**决定能不能试。
- 每次解锁都以 `PAK = 链上的槽` 收尾核对（§4.6 第 3 步）。同一把通行密钥在不同平台求出不同的 PRF（扫码的跨设备方式偶有此事）时，核对失败，界面说「这台设备上的 {provider} 解不开，请换一台设备，或扫码配对」，不留任何半成品。

### 5.3 降级

| 情况 | 登录 | 解锁 |
|---|---|---|
| 没有 WebAuthn | 密码表单为默认，通行密钥按钮不显示 | 只有扫码配对与恢复词 |
| 有 WebAuthn、没有平台认证器（例如 Windows 10 没开 Hello、Linux） | 通行密钥按钮照常：浏览器会提供安全钥匙或手机扫码 | 取决于那把钥匙或那部手机（§5.1） |
| 这次没有 PRF 结果 | — | 「这把通行密钥在这台设备上不能解锁」，并给出扫码配对与恢复词；Hub 的 `prf` 标为 `no` |
| 账户有槽但槽已作废 | 照常 | §4.7「没人重开时」 |
| 提供方删掉了凭据（Google 密码管理工具 PIN 重置会删光所有通行密钥 [41]） | `unknown_passkey` → 隐藏；用密码或邮件登录 | 那个槽从此无用；admin 设备上提示「这把通行密钥似乎已不存在，删除它？」（连续 3 次 `unknown_passkey` 时） |

### 5.4 Tauri 应用

**webview 里不行**：

- Tauri 的页面源是 `tauri://localhost` 或 `https://tauri.localhost`。用 RP ID `droppen.app` 调 WebAuthn，在 WebView2 和 Android WebView 里都是 `SecurityError` [28][30]。
- WKWebView 只在页面源属于关联域（entitlement `webcredentials:droppen.app` 加上 apple-app-site-association）时能用 [26]。那等于要在 App 里加载 droppen.app 托管的页面，正好丢掉「界面随签名 App 分发、不经 Hub 投递」的高保障性（聚合器文档 §7.3）。浏览器专用的 entitlement 不适用于我们 [27]。
- Android WebView 经 androidx.webkit 1.12+ 能开 WebAuthn，但同样要求源匹配，且不支持自动填充 [31]；webview 里的 PRF 各平台都未确认。

**建议**：

1. **桌面版不在应用内调用 WebAuthn。** 登录走系统浏览器（聚合器文档 §4.9 的浏览器交回），通行密钥在系统浏览器里原生可用，第一层完整可用。桌面版可运行 AI，只能经注册证明由 admin 加入，通行密钥的第二层本来就加不进它（§4.11）。
2. **移动版（以后）**：手机是普通设备或 admin，不运行 AI，可以用通行密钥登录和解锁。那时写原生插件：
   - Apple：AuthenticationServices 的 PRF 输入与输出（iOS 18 / macOS 15 起）[29]，加关联域 `webcredentials:droppen.app`；
   - Android：Credential Manager，选项与应答都是 WebAuthn JSON；origin 是 `android:apk-key-hash:<签名哈希>`，Digital Asset Links 要有 `handle_all_urls` 与 `get_login_creds` [33][34]；
   - （Windows 若以后需要：`webauthn.dll`，创建时 `bEnablePrf`，断言时带 hmac-secret 的 salt [11]，Windows Hello 需要 2026 年 2 月的更新。）
   - Hub 把这些原生 origin 加进合法 origin 列表。Apple 原生调用写进 `clientDataJSON` 的 origin 以实测为准。
3. 现成插件不够用：`tauri-plugin-webauthn` 不支持 macOS/iOS，也没写 PRF [35]；Tauri 的跟踪 issue #7926 仍开着 [28]。要自己写 Swift、Kotlin（必要时 Rust）插件，放在开源仓库。

## 6. 威胁模型更新

### 6.1 各方能做什么

与架构文档 §7 同口径，只列变化。

| 攻击者 | 能做 | 不能做 | 缓解 |
|---|---|---|---|
| Hub 或运营方 | 看到 §6.2 的元数据；拒绝通行密钥登录（拒绝服务）；给新设备错的链或 salt（只会失败） | 拿到 PRF 输出、`PAK` 私钥或 `MK`：它只校验断言、只存公钥与打不开的 `pk/` 包；伪造 `Passkey(slot)` 的签名；用自己的 `AK` 包住真实 `PAK` 骗新设备（证明绑定 `AK`）；把自己的钥塞进 `pk/`（发送方认证 + 链上提交） | 证明、提交、发送方认证；新设备只在 §4.6 第 3 步全成立后才钉创世 |
| **网页前端投递**（剩余缺口，`docs/web-integrity.md`） | **范围变大**：今天恶意 JS 只能使用**已在链上的设备**的设备钥；开了第二层后，用户在**任何**浏览器上对 droppen.app 做一次通行密钥手势，恶意 JS 都能顺带求 PRF，解出所有 host 的 `MK`（全部历史），并以新设备身份签待定加入 | 跳过待定：控制仍要批准或 24 小时，且全体告警；拿到管理权或 `UK` | 开启文案写明「不要在别人的设备上用它解锁」；需要高保障的人不开第二层；可复现构建与公开哈希 |
| 同步提供方（Apple、Google、1Password 这些公司本身） | 知道你在 droppen.app 有一把通行密钥（RP、邮箱作 `user.name`、创建时间） | 使用它：同步型通行密钥由提供方端到端加密同步 | — |
| **掌握你同步账户的人**（新） | 在自己的设备上用这把通行密钥：登录 Hub；求 PRF 得到 `PAK`；签待定加入 | 诚实 Hub 下 24 小时内读到任何历史或 `MK`、开通道；悄无声息：加入、自行激活都全体告警；拿到管理权：即使 24 小时后激活，也只是普通设备，你的 admin 随时能移除它 | 待定 + 两位数批准 + 24 小时 + 否决即作废槽；告警文案引导先保护同步账户、删除并重建通行密钥 |
| 掌握你同步账户的人 + 恶意 Hub | 立即读全部历史（Hub 给流与包） | 24 小时内控制电脑（每个验证者用自己的时钟）；让关着的浏览器把激活当历史收下（压后）；拿到管理权或 `UK` | 与「偷到恢复词 + 登录密码 + 恶意 Hub」相比，拿不到管理权；否决 + 删除通行密钥；怀疑 Hub 时在电脑上 `agent re-enroll` |
| 与你共享这把通行密钥的人（iOS「共享的密码和通行密钥」、1Password 共享保险库） | 与上一行相同；Apple 的共享群组里每个成员都能用、改、删它，退出群组也收不回已共享的 [40] | — | 开启文案明确禁止共享；Hub 看不出是否共享，只能靠告警 |
| 本机恶意程序（另一台电脑上的） | 以 droppen.app 的名义调平台认证器：Windows 上的桌面程序经 `webauthn.dll` 调用时，系统不核对程序与 RP ID 的关系（浏览器正是这样调用的）；能直接访问 USB 安全钥匙的程序也可以发 CTAP 请求。用户确认一次就拿到 PRF | 跳过待定与告警 | 与「那台电脑被攻破」同级；待定与批准照样兜底 |
| 钓鱼站 | 骗用户在 `droppen-app.com` 上点一下 | 拿到断言或 PRF 输出：凭据绑在 RP ID `droppen.app` 上，浏览器不会为别的站调用它 | §3.8：Turnstile 搬到独立域名、子域禁用 WebAuthn、Hub 只收精确的 origin |
| 恶意中继 | 不变：只见密文帧 | 不变 | 通行密钥与中继毫无交集 |
| 被盗设备 | 不变 | 不变 | 它若算过 PRF，移除时槽自动作废（§4.7） |
| 偷到登录密码 | 不变（使用服务、登记待定设备） | 加通行密钥或改密码（要 admin）；读或删数据 | 账户可以去掉密码，只留通行密钥 |
| 偷到邮箱 | 邮件找回登录，得到登录会话 | 加通行密钥、设密码、读数据、进链 | 找回只给登录会话；Turnstile；通知信 |
| 认证器被克隆（非同步型的安全钥匙） | 用克隆品登录 | — | 计数回退即拒绝并告警 |

### 6.2 云端能看到什么（追加到架构文档 §2）

| 类别 | 云端可见（明文） | 加密或根本不在云端 |
|---|---|---|
| 登录凭据 | 每把通行密钥的凭据 id、公钥、算法、AAGUID（提供方）、BE/BS（是否同步）、传输方式、计数、标签、创建与上次使用时间、每次登录的 IP、登录方式 | 私钥（在提供方或安全钥匙里） |
| 解锁 | 哪把通行密钥开了解锁（链上的槽、`PAK` 公钥、`cred_hash`、`salt_nonce`、`grants`、是否允许自行激活）；`pk/m/` 包的名字（含 host id 与 `MK` 轮次）、大小、时间；待定加入、批准、自行激活的时间 | PRF 输出、`PAK` 私钥、`twin` 私钥、`MK` |

### 6.3 开启解锁时的文案

`{provider}` 取 AAGUID 对应的提供方名（「iCloud 钥匙串」「Google 密码管理工具」「1Password」）；`{account}` 取对应的账户名（「Apple 账户」「Google 账户」「1Password 账户」）；认不出时分别用「这个通行密钥」「同步这个通行密钥的账户」。

**zh-CN**

> **用这个通行密钥解锁新设备？**
>
> 开启后，新手机或新浏览器只要用「{provider}」里的这个通行密钥（Face ID / 触控 ID），就能加入你的账户、使用你的电脑，不用扫码，也不用恢复词。
>
> **代价：能使用这个通行密钥的人，就能解锁你的数据。** 它跟着你的 {account} 同步，所以掌握了你的 {account}（再加上一台受信设备的锁屏密码）的人，也可能做到。这相当于把一把备用钥匙交给你的 {account} 保管。Droppen 从不保管任何钥匙；这个选项是唯一的例外，所以默认关闭。
>
> 开启后你仍然受到保护：
> - 用通行密钥加入的新设备先「等待批准」：要在你的另一台设备上输入它屏幕上的两位数才生效。没人批准时，它要等 24 小时才能自行激活。
> - 这期间你所有的设备和电脑都会收到提醒，任何一台能管理账号的设备都能拒绝它。
> - 等待期间，Droppen 不会把你的数据交给它，它也控制不了你的电脑。
> - 通行密钥只能让新设备**使用**你的电脑，不能让它**管理**账号（加删设备、添加电脑）。管理权只能由你已有的设备授予，或者用恢复词恢复——所以请继续保管好恢复词。
> - Droppen 的服务器拿不到能解锁的东西，只保存它打不开的密文。
>
> 不要把这个通行密钥放进共享群组，也不要在别人的设备上用它解锁：群组里的人、那台设备上的程序，都能借它解锁。
>
> 扫码配对和恢复词照常可用，仍然是最后的退路。
>
> ☐ 我明白：掌握我 {account} 的人可能解锁我的数据。
>
> 【用 Face ID 开启】　【暂不开启】

**en**

> **Unlock new devices with this passkey?**
>
> When this is on, a new phone or browser can join your account and use your computers with this passkey from {provider} (Face ID / Touch ID) — no QR code, no recovery words.
>
> **The cost: anyone who can use this passkey can unlock your data.** It syncs with your {account}, so someone who takes over your {account} (and knows the passcode of one of your trusted devices) could do it too. In effect you hand a spare key to your {account}. Droppen never holds your keys; this option is the one exception, so it is off by default.
>
> With it on, you are still protected:
> - A device that joins with a passkey waits for approval. It counts only after you enter the two-digit number it shows on another of your devices. If nobody approves it, it can activate itself only after 24 hours.
> - Meanwhile all your devices and computers are alerted, and any device that manages your account can refuse it.
> - While it waits, Droppen gives it none of your data, and it cannot control your computers.
> - A passkey only lets a new device **use** your computers, never **manage** your account (add or remove devices, add computers). Only your existing devices can grant that, or your recovery words can restore it — so keep your recovery words safe.
> - Droppen's servers never get anything that unlocks your data; they only keep ciphertext they cannot open.
>
> Do not put this passkey in a shared group, and do not unlock with it on someone else's device: people in the group, or software on that device, could unlock with it.
>
> Pairing with a QR code and your recovery words keep working and remain the last resort.
>
> ☐ I understand that someone who controls my {account} could unlock my data.
>
> [Turn on with Face ID]　[Not now]

**安全钥匙（BE = 0）的变体**：把「代价」一段换成——zh-CN：「**代价：拿到这把安全钥匙、又知道它的 PIN 的人，就能解锁你的数据。** 它不同步，没有第三方保管；请像保管恢复词一样保管它。」en：「**The cost: anyone who has this security key and knows its PIN can unlock your data.** It does not sync, so no third party holds it; keep it as safe as your recovery words.」勾选项相应改为「我明白：拿到这把钥匙的人可能解锁我的数据。」/「I understand that whoever has this key could unlock my data.」

繁体中文与日文按同样的结构翻译（云端界面四种语言齐）。

### 6.4 其它文案

| 场合 | zh-CN | en |
|---|---|---|
| 新设备待定（新设备上） | 在你的另一台设备上输入 **37** 来批准这台设备。没有别的设备？明天 14:02 之后回到这里，再用 Face ID 激活。 | Enter **37** on another of your devices to approve this one. No other device? Come back after 14:02 tomorrow and activate it with Face ID. |
| 告警条（admin 设备上） | 一台新设备正用通行密钥「{provider}」申请加入（{time}）。是你吗？ 【输入数字批准】【不是我】 | A new device is asking to join with your {provider} passkey ({time}). Was this you? [Enter number to approve] [Not me] |
| 批准页的勾选项 | ☐ 也让它管理账号（加删设备、添加电脑） | ☐ Also let it manage your account (add or remove devices, add computers) |
| 拒绝确认 | 这台设备会被移出账户，这个通行密钥的解锁会停用。如果不是你，有人可能掌握了你的 {account}：请先修改它的密码、检查受信设备，再从 {provider} 里删除这个通行密钥，之后可以重新创建。 | This device will be removed and unlocking with this passkey will be turned off. If this wasn't you, someone may control your {account}: change its password and review its trusted devices first, then delete this passkey from {provider}. You can create a new one afterwards. |
| 作废横幅 | 通行密钥「{provider}」的解锁已暂停：被移除的设备曾用它解锁。在这台设备上用 Face ID 重新开启。 【重新开启】【以后】 | Unlocking with your {provider} passkey is paused because a removed device had used it. Turn it back on here with Face ID. [Turn on again] [Later] |
| 解锁暂停（新设备上） | 这个账户的通行密钥解锁已暂停。请在另一台设备上扫码配对，或输入恢复词。 | Passkey unlock is paused for this account. Pair from another device, or enter your recovery words. |
| 不支持 PRF | 这个通行密钥可以登录，但在这台设备上不能解锁新设备。 | This passkey can sign you in, but it cannot unlock new devices here. |
| 电脑日志与推送 | 账户变更：设备 {id} 用通行密钥申请加入（待定）／自行激活（不可管理账号） | Account change: device {id} asked to join with a passkey (pending) / activated itself (cannot manage the account) |

## 7. 界面

### 7.1 注册（通行密钥优先）

```
注册 Droppen
┌──────────────────────────────┐
│ 邮箱                          │
└──────────────────────────────┘
[ Turnstile ]
[■ 创建通行密钥并注册 ]          ← 主按钮
改用密码注册 · 已有账户？登录
```

- 浏览器没有 WebAuthn（§5.2）时，默认显示密码表单。
- 之后是「查收邮件」→ 创世 → 恢复词（默认照旧，可跳过）→ **新的一页**：「也用这个通行密钥解锁新设备吗？（可选）」，点开是 §6.3 的说明；「以后再说」是同样大的按钮。

### 7.2 登录

```
登录 Droppen
┌──────────────────────────────┐
│ 邮箱              ⌄ 通行密钥  │   ← autocomplete="username webauthn"
└──────────────────────────────┘
[■ 使用通行密钥登录 ]
用密码登录 · 用邮件登录 · 注册
```

- 「用密码登录」展开今天的密码表单；「用邮件登录」走 §3.9。
- 登录后：这台浏览器已在链上 → 照旧进应用；还不在 → 「这台设备还没有钥匙」页，按可用性依次列出：**用 Face ID 解锁**（有当前槽时，排第一）、扫码或口令配对、恢复词。

### 7.3 设置 → 「我的设备」→ 「登录与解锁」一节

```
登录与解锁
通行密钥
  iCloud 钥匙串      已同步 · 10-17 创建 · 今天 09:12 用过      解锁：开      [改名] [删除]
  YubiKey 5         仅此钥匙 · 10-17 创建                      解锁：关      [改名] [删除]
  Windows Hello     仅此设备 · 10-12 创建                      解锁：不支持  [改名] [删除]
  [+ 添加通行密钥]
密码            已设置                                                       [修改] [删除]
上次登录         通行密钥 · iCloud 钥匙串 · 今天 09:12 · 东京
```

- 放在聚合器文档 §3.7「我的设备」里，设备列表之下。
- 「解锁」一列四种状态：开 / 关 / **暂停**（作废后，带「重新开启」）/ 不支持。
- 只有 admin 设备能操作这一节；其它设备只读，并写明「在能管理账号的设备上修改」。
- 待定设备横幅（§4.6）与今天的恢复横幅合成一条「待定设备」横幅。
- 删除前确认页列出：会断开的登录、会作废的解锁、靠它待定的设备。

### 7.4 新手机约 3 次点按

| # | 在哪 | 做什么 |
|---|---|---|
| 1 | 新手机，droppen.app | 点「使用通行密钥登录」→ 系统面板 Face ID |
| 2 | 新手机 | 「这台设备还没有钥匙」页点「用 Face ID 解锁」→ Face ID → 屏幕显示 37 |
| 3 | 你的 Mac（或任意一台 admin 设备，推送直达） | 告警条「新设备申请加入」→ 输入 37 → 批准 |
| — | 新手机 | 约 1 秒内变为活跃，列表与历史出现 |

新手机上是两次点按、两次 Face ID，另一台设备上一次批准。丢光设备时第 3 步换成「明天 14:02 之后回来再 Face ID 一次」。

合成一次手势（登录与解锁一起）需要在登录前就知道 salt，见 §9 未决问题 1。

### 7.5 批准页

```
批准新设备？
iPhone · Safari · 用通行密钥「iCloud 钥匙串」申请 · 2 分钟前
设备指纹 3F2A 9C10 …
输入它屏幕上的两位数： [ _ _ ]
☐ 也让它管理账号（加删设备、添加电脑）
[批准]   [不是我]
```

恢复词加入的批准也改用这一页（同样输入两位数），见 §10 待定第 2 条。

### 7.6 `/link` 弹窗

- 登录页同 §7.2。
- 「这台浏览器还不是 admin」页多一条「用通行密钥解锁」，后续与 §7.4 相同；只有批准时给了管理权，弹窗才继续到同意页（§4.11）。

## 8. 里程碑（按 PR 拆）

> 本节原是内部的 PR 计划（排期、测试环境、验收记录），不在公开仓库。代码注释里的编号指：K1 通行密钥登录与注册（§3.1–3.5、§3.8）；K2 管理通行密钥、只有通行密钥的账户、邮件找回登录（§3.6、§3.9）；K3 派生与证明的原语（§4.2）；K4 开启解锁、新设备加入、两位数批准（§4.3、§4.5、§4.6）；K5 24 小时自行激活、槽作废与重开（§4.6、§4.7）；K6 `/link` 弹窗（§4.11、§7.6）。

测试里的 WebAuthn 一律用 Chrome DevTools Protocol 的 `WebAuthn` 域挂**虚拟认证器**（`page.cdp(…)`），或 `rh-devkit` 的软件通行密钥。

**虚拟认证器的用法**

- 平台型同步通行密钥：`WebAuthn.addVirtualAuthenticator {protocol: "ctap2", ctap2Version: "ctap2_1", transport: "internal", hasResidentKey: true, hasUserVerification: true, isUserVerified: true, hasPrf: true, defaultBackupEligibility: true, defaultBackupState: true}`。
- 创建时就能求 PRF：再加 `hasHmacSecretMc: true`；不支持 PRF 的认证器（模拟 Microsoft 密码管理器或 Windows 10）：`hasPrf: false`。
- 负面用例：`WebAuthn.setResponseOverrideBits {isBogusSignature | isBadUV | isBadUP}`；`isUserVerified: false`。
- **模拟「同步到新设备」**：同一页面保留认证器，用 `Storage.clearDataForOrigin`（IndexedDB、localStorage、Cache Storage、Service Worker）清掉站点数据，即一台「新浏览器」。K1 先验证另一种做法——`WebAuthn.getCredentials` 导出、在另一个页面的认证器上 `addCredential` 导入——PRF 输出是否不变（CDP 的 `Credential` 不带 hmac-secret 的种子，多半会变）；变了就只用清数据的做法。

### K1 怎么落地的（2026-10-18）

> Hub 一侧的实现笔记（验证器、限流桶、迁移与回滚、本地验收）不在公开仓库。与本文不同、客户端看得到的决定：

- **Turnstile 的域名没有搬**：§3.8 第 1 条不做；第 2 条照做——应用自己的主机回 `publickey-credentials-get=(self), publickey-credentials-create=(self)`，Hub 应答的其它主机（Turnstile 页、子域、被 308 的名字）和中继的每个应答都回 `=()`。注册仍在选项那步检查 Turnstile，Hub 没有配置时照常放行。
- **通行密钥登录有自己的限流**，不与 `/v1/login` 共用（§3.9 的表原写「同一个桶」），也不被密码登录的退避挡住；失败的断言不进密码的任何计数。
- **注册的 `user.id`** 由挑战在 Hub 的密钥下派生，对别人是随机的，与邮箱、账户 id 无关；所以 `POST /v1/signup/passkey` 不必回传它。
- **计数回退**：设备绑定的通行密钥（BE = 0）计数不升即拒绝；同步型（BE = 1）的计数由提供方维护、有的会回退，暂时放行并记日志。
- **K1 就会产生没有密码的账户**（用通行密钥注册的）。密码登录这种账户时照算一次假哈希、回 `bad_login`（§3.6）。
- 测试环境的已知情况：虚拟认证器加 `hasHmacSecretMc: true`、创建时又带 PRF 输入，会让 Chromium 153 崩溃，只用 `hasPrf: true` 时创建就能给出 PRF 结果；`WebAuthn.getCredentials` 导出、在另一个认证器上 `addCredential` 导入后 PRF **没有结果**，所以模拟「同步到新设备」只能用清站点数据的做法。

### K2 与「新设备用通行密钥加入」怎么落地的（2026-10-22）

> 账户设置页（`/account`，`W/features/cloud/account-settings.tsx`）、Hub 的登录方式接口（`docs/hub-api/http.md`「The account's ways to sign in」）、新设备加入（`W/lib/hub/passkey-join.ts`、`W/features/cloud/passkey-join.tsx`、`CloudAccount.enablePasskeyJoin` / `joinWithPasskey` / `pairDevice`）。与本文不同、客户端看得到的决定：

- **添加的选项是自己的路由** `POST /v1/passkeys/add/options`（设备会话），不是 `POST /v1/passkeys/options {purpose: "add"}`：那条路由不登录，放在同一个路由上就得在一条路由里认两种鉴权。挑战绑定（账户, 设备），与 §3.4 相同。
- **任何组合都行，至少留一种**：密码可以设、改、删；通行密钥可以加、改名、删。最后一种登录方式删不掉（`409 last_credential`）；一个链上的槽还在用的通行密钥也删不掉（`409 unlock_on`：页面先签 `PasskeySlotRemove` 再删）。删除时**先查证明、再查槽**：证明不对一律 `403`，不透露槽的情况；证明对了才回 `409 unlock_on`，并带一个 `grant`（新的挑战用途 5 `Removal`，绑定（账户, 设备, 通行密钥），两分钟、只用一次），页面签完 `PasskeySlotRemove` 后带着它再删一次，不必让人再证明一次。查槽到删除一直拿着账户的链，中间插不进一条点名它的 `PasskeySlotSet`。
- **增减登录方式要再证明一次是本人**（§3.4 B、§3.6 原本只要 admin 的设备会话）：添加、删除通行密钥，设、改、删密码，都要一次新的通行密钥断言（新的挑战用途 4 `Reauth`，绑定（账户, 设备），`POST /v1/passkeys/reauth/options` 发，登录的挑战不算数），或者——账户有密码时——当前密码（每台设备 10 分钟 3 次、一天 10 次，每账户 10 分钟 6 次、一天 20 次，先算设备的：一台借来的设备用不完整个账户的次数）。只管密码不够：借到一台开着的完整设备的人，可以先加一把自己的通行密钥，再用它改密码。所以用通行密钥注册的账户可以在设置页设一个密码（用通行密钥证明），用密码注册的账户加第一把通行密钥要输当前密码，之后两种都能登录。改名不要证明。没有通知信（邮件外发现在只会发验证信，见 Hub 实现笔记）；但每一次变化——加、改名、删通行密钥，设、改、删密码——都在做这件事的同一个事务里记一行（`sign_in_changes`：哪种变化、哪台设备、通行密钥的名字），提交后推给账户的每台设备（控制消息 `sign_in_changed`），`GET /v1/me` 列出最近 30 天的（`sign_in_changes`，最多 50 条）。账户的每台完整设备（做这件事的那台除外）在页顶看到一条：哪种变化、哪台设备、何时，「是我操作的」才消失；一个浏览器第一次读到之前的变化算历史，不再提示。页面上「哪个槽点名哪把通行密钥」用页面自己对凭据 id 求的哈希，不用 Hub 给的 `cred_hash`；刚创建的那把用认证器给的 `rawId`，不用 Hub 回显的凭据 id。
- **槽只让新设备「申请」加入**：页面登记的槽一律 `selfActivate: false`，也不写 `pk/m/…` 包（§4.3 的封装、`GET /v1/keys` 对待定会话的扣留都还没做）。新设备用通行密钥的 PRF 派生出 `PAK`，核对链上的槽与证明后才钉住创世，然后以 `Passkey(slot)` 签待定的 `AddDevice3{flags: 0}`。任何一台完整设备都能「不是我：移除」。**24 小时自行激活（§4.6 5b）不做**：没有包，自行激活的设备也拿不到钥匙。
- **不用两位数，用配对批准**（§4.6 第 4 步、5a 步改了）：两位数是公开数据的哈希模 100，拿到这把通行密钥的人可以磨出一个同数的身份去顶替待定的那台（一个槽同时只有一个待定加入，新的顶掉旧的），而「连错 3 次」只锁一个浏览器，不锁待定设备。现在待定的新设备显示的就是普通配对页（打开时就在 8 位数字码一格，也可以换成二维码），全是它自己的钥；完整设备在告警条里点「批准」展开普通的配对（同「添加浏览器或手机」）。CPace 或二维码里的秘密认证了新设备屏幕上的钥，批准的设备只在这对钥**等于**链上那台待定设备的钥、且它此刻仍在待定时，才签 `[ApproveDevice, SetAdmin{true}]`；否则什么都不签（钥不同：「显示这个码的设备不是申请加入的那一台」；被更新的加入顶替或已被移除：「已不在等待加入……什么都没有批准」，确认框开着时链变了也立刻这样说）。按 id 直接「批准」（恢复词那种）对通行密钥的加入一律拒绝。没有引入新协议：配对的代码原样复用。
- **Hub 给的 id 与自己的钥对不上就是分叉**：新浏览器登记后，链上若有一条记录用了它的 id、钥却不是它自己的（例如 Hub 把一台待定设备的 id 发给了它），这台浏览器立刻按分叉告警停下，绝不当成「等待批准」，屏幕上也不会出现从链上记录算出的任何东西。
- **「不是我：移除」先警告**（§4.6 5c）：先展开说明——不是你，就是别人能用这把通行密钥，多半是通过它同步所在的账户；移除这台设备会拦下它、也会让这个槽作废，但通行密钥本身仍能登录；先保护那个账户，再删除并重建这把通行密钥。移除后告警条留一行「删除那个通行密钥」，直达「我的账户」里那把的删除确认。
- **批准即完整设备**：批准写 `[ApproveDevice, SetAdmin{true}]` 一批，用户钥随配对的投递（用途 3，用这次配对的秘密做 MAC）送到——按「所有浏览器、手机都是完整设备」（聚合器文档 §3.1，2026-10-20）。§4.10「通行密钥永不授予管理权」仍然成立：链只让通行密钥加入 `flags = 0` 的设备，管理权来自完成配对的那个人。
- **默认开，是有意改了 §4.1**：添加通行密钥时「新设备可用它加入」默认勾上（§4.1 原定默认关闭）。理由：批准绑定在配对认证过的钥上之后，单凭这把通行密钥什么也拿不到——没有人在另一台完整设备上与它的屏幕完成配对，它就永远进不来；原文的「默认关闭」针对的是带自行激活和包的完整第二层。开启时用的是创建那一下的 PRF（认证器给的话），否则再请求一次。
- **登录记住用的是哪把**：通行密钥登录后，这个标签页（sessionStorage）记下凭据 id，新设备加入时直接对这把求 PRF；没记下而链上只有一个槽时用那个槽的 salt，几个槽时先问一次是哪把（多一次手势）。
- 端到端测过：Chrome 155 的虚拟认证器（`hasPrf`）对本地 Hub——通行密钥注册、开「新设备可加入」、用通行密钥设密码、第二台设备用密码登录再用恢复词加入、清空第一台浏览器的站点数据后用同一把通行密钥登录并加入（它显示普通配对页）、另一台设备在告警条里点「批准」展开配对、输入新设备显示的 8 位数字码并核对指纹后批准、新设备从配对的投递里拿到钥匙成为完整设备、另一台设备添加第二把（创建时求 PRF）并删掉第一把（先查证明，再撤槽，再凭 `grant` 删），新设备的页顶随即各出现一条登录方式变化的提示。iOS 模拟器的 Safari 能打开页面，但通行密钥的系统面板点不了（没有 UI 自动化），没测到真机的 Face ID 与 iCloud 钥匙串的 PRF。

## 9. 风险与未决问题

**风险**

| 风险 | 缓解 |
|---|---|
| 同步账户被接管（SIM 劫持、账户恢复流程被社工） | 默认关闭；待定 + 批准 + 24 小时 + 全体告警；只加入普通设备；拒绝即作废；文案引导先保护同步账户 |
| 恶意前端在任何一次通行密钥手势里顺带求 PRF（前端投递这个缺口的范围变大） | 文案「不要在别人的设备上解锁」；高保障用户不开；可复现构建 |
| 子域调用 WebAuthn | Turnstile 搬到独立域名；子域一律 `Permissions-Policy`；中继不回 HTML；测试 |
| 用户在告警疲劳下顺手批准 | 必须输入两位数，3 次错即锁；管理权默认不勾；告警文案写明「不是你就拒绝」 |
| 提供方之间 PRF 行为不一致（不同平台求出不同的 PRF、创建时不给 PRF、扫码不传 PRF、Windows Hello 报告不一） | 每次解锁都用 `PAK = 链上的槽` 核对，不一致即如实说明；不在客户端缓存任何 PRF 结果；创建时总带 PRF 输入 |
| 用户以为删除了通行密钥就安全了，其实同步账户里还在 | 删除确认页给出去提供方删除的路径；有 Signal API 的平台自动隐藏 |
| 链条目的发版风险 | 随 v3 一起定义、一起过门槛（聚合器文档 §3.5） |
| 通行密钥注册留下孤儿凭据 | Turnstile 与邮箱占用在创建前检查；失败时 `signalUnknownCredential` |
| 只有通行密钥的账户全丢 | 邮件找回登录；已在链上的设备不需要登录；最后才是新创世 |
| 丢光 admin、又没有恢复词：能用一周、管不了 | 开启文案要求保留恢复词 |
| 钥环条目随槽数 × host 数 × `MK` 轮次增长 | 至多 8 个槽；回收规则同 `mk/`；上限公式加槽一项（§4.3） |

**未决问题**

1. **一次手势**：新设备上「登录 + 解锁」能否合成一次 Face ID？难点是登录前不知道是哪把凭据，也就不知道 salt。两条路：
   - 每把凭据用固定 salt，作废时**重建通行密钥**而不是换 salt（同一 `user.id` 在 iCloud 钥匙串里会覆盖旧的）；
   - 先填邮箱，让 Hub 回 `allowCredentials` 和各自的 salt（要给不存在的邮箱回伪造的凭据 id，防枚举）。

   第一版都不做：两次 Face ID 的步骤更清楚，换 salt 的重开也更便宜。
2. **24 小时是否太短**：同步账户比纸上的恢复词更容易被远程攻破，而小偷可以挑你睡觉的时候。备选是 48 或 72 小时，代价是丢光设备的人要等更久。
3. **`user.name` 用邮箱**会让提供方知道「这个邮箱在 droppen.app 有账户」。备选是让用户自取一个显示名；代价是多个账户时难以分辨。
4. **UI 文档的「文档钥」**（聚合器文档 §3.3）定下后，通行密钥包是否也封它：建议封，规则同 `MK`。
5. **Tauri 移动版**：原生插件的工作量与时机，见 §5.4。

## 10. 已定与待定

**已定**（2026-10-17）

| # | 决定 |
|---|---|
| 1 | 第二层做：可选，默认关闭，按把开启，开启时勾选 §6.3 的那句话 |
| 2 | 方案 (c)：链上独立的 `PAK`（`PasskeySlotSet`），只能以 `Passkey(slot)` 签 `flags = 0` 的待定 `AddDevice3` 与 24 小时后的 `PasskeyActivate`；另一台 admin 输入两位数即可批准 |
| 3 | 通行密钥永不授予管理权：`grants`、`flags` 必须为 0；管理权只由已有 admin 授予（批准时可勾）或由恢复词恢复 |
| 4 | 待定期间什么都读不到：Hub 不给流、页与 `pk/` 包 |
| 5 | Turnstile 页搬到独立的可注册域名（K1 没有搬，见 §8）；所有 droppen.app 子域禁用 WebAuthn |
| 6 | （随链 v3 已定）`UK` 只给 admin，所以通行密钥包按 host 封 `MK`；普通设备的控制按 7 天新鲜度（聚合器文档 §3.3、§3.4） |

**待定**（附建议）

| # | 问题 | 建议 |
|---|---|---|
| 1 | 封装方式 | 封给 `PAK` 的 X25519 孪生钥（与恢复包相同），不用对称 KEK：轮换不要手势，只有槽作废才要一次 Face ID |
| 2 | 恢复词的批准也输两位数 | 是 |
| 3 | 自行激活的延迟 | 24 小时，与恢复相同；备选 48 小时 |
| 4 | 只有通行密钥的账户 | 允许；删除密码要先有通行密钥；不能删掉最后一个登录凭据 |
| 5 | 邮件找回登录 | 做（K2），只给登录会话 |
| 6 | `user.name` | 邮箱 |
| 7 | 「立即生效」的高级选项 | 不提供 |
| 8 | Tauri 上的通行密钥 | 桌面版不在应用内调用 WebAuthn，登录走系统浏览器；移动版再写原生插件 |

## 11. 图

图在 `docs/passkeys/`，制图纸样式：一种墨，细线，直角，连线是直线、最多一个弯；颜色只表示状态。

1. `signin.svg`：用通行密钥注册与登录 Hub（第一层）。
2. `unlock.svg`：admin 开启解锁、新设备登录与解锁、两位数批准、24 小时自行激活（第二层）。

## 12. 资料来源

检索于 2026-10-17；括号里是资料本身的日期，早于半年的已注明。

1. W3C, Web Authentication Level 3（推荐标准，2026-08-25）：https://www.w3.org/TR/webauthn-3/
2. WebKit, Features in Safari 18.0（2024-09-16）：https://webkit.org/blog/15865/webkit-features-in-safari-18-0/
3. Corbado, Passkeys & WebAuthn PRF（2026-09-29 更新，数据 2026-08）：https://www.corbado.com/blog/passkeys-prf-webauthn
4. Authenticator support table（无日期，条目到 2026-06）：https://mera.category.xyz/authenticator-support/
5. WebKit, Features in Safari 26.4（2026-03-24）：https://webkit.org/blog/17862/webkit-features-for-safari-26-4/
6. WebKit bug 311099（2026-03 至 04）：https://bugs.webkit.org/show_bug.cgi?id=311099
7. Apple Developer Forums 764730（2024–2025，早于半年）：https://developer.apple.com/forums/thread/764730
8. Apple Developer Forums 774112（2025-02 至 04，早于半年）：https://developer.apple.com/forums/thread/774112
9. MDN browser-compat-data（`api/CredentialsContainer.json`、`api/PublicKeyCredential.json`，主分支）：https://github.com/mdn/browser-compat-data
10. blink-dev, Intent to Ship: WebAuthn PRF（2023，早于半年）：https://groups.google.com/a/chromium.org/g/blink-dev/c/iTNOgLwD2bI/m/Oz1C7oEYAgAJ
11. Microsoft, `webauthn.h`（API v9）：https://github.com/microsoft/webauthn/blob/master/webauthn.h
12. Bitwarden 社区，Encryption PRF via Windows Hello（2026-03 至 09）：https://community.bitwarden.com/t/encryption-prf-via-windows-hello-passkey/94236?page=2
13. MDN, Firefox 139 release notes（2025-05-27，早于半年）：https://developer.mozilla.org/en-US/docs/Mozilla/Firefox/Releases/139
14. Bugzilla 1958716（Firefox 149，2026-03-24 发布）：https://bugzilla.mozilla.org/show_bug.cgi?id=1958716
15. 1Password 博客（2024-07）与社区（2025-03 至 05），均早于半年：https://1password.com/blog/encrypt-data-saved-passkeys 、https://www.1password.community/1password-at-home-31/prf-support-21443
16. Yubico, PRF support on security keys（无日期）：https://developers.yubico.com/Passkeys/Passkey_concepts/Security_key_capabilities/Pseudo-Random_Function_PRF_Support.html
17. Yubico, Developer's Guide to PRF（「截至 2025 年中」）：https://developers.yubico.com/WebAuthn/Concepts/PRF_Extension/Developers_Guide_to_PRF.html
18. MDN, `getClientCapabilities()`（2026-02-09）：https://developer.mozilla.org/en-US/docs/Web/API/PublicKeyCredential/getClientCapabilities_static
19. MDN, WebAuthn extensions（2026-09-17）：https://developer.mozilla.org/en-US/docs/Web/API/Web_Authentication_API/WebAuthn_extensions
20. passkeys.dev, Device support（2026-09-21）：https://passkeys.dev/device-support/
21. web.dev, Related Origin Requests（2024-08-22，引用 2026-01 的状态）：https://web.dev/articles/webauthn-related-origin-requests
22. Firefox 源码，`nsIWebAuthnRelatedOriginFetcher.idl`：https://searchfox.org/firefox-main/source/dom/webauthn/nsIWebAuthnRelatedOriginFetcher.idl
23. Chrome, WebAuthn Signal API（正文 2024-11）：https://developer.chrome.com/docs/identity/webauthn-signal-api
24. WebKit, Features in Safari 26.0（2025-09）：https://webkit.org/blog/17333/webkit-features-in-safari-26-0/
25. Corbado, WebAuthn client capabilities benchmark（2026 年一季度）：https://www.corbado.com/passkey-benchmark-2026/webauthn-client-capabilities.md
26. Apple Developer Forums 726943（2023-03，早于半年）：https://developer.apple.com/forums/thread/726943
27. Apple Developer Forums 829169（2026-06）：https://developer.apple.com/forums/thread/829169
28. Tauri issue #7926（仍开着，最后评论 2026-04-07）：https://github.com/tauri-apps/tauri/issues/7926
29. Apple, `ASAuthorizationPublicKeyCredentialPRFRegistrationInput`：https://developer.apple.com/documentation/authenticationservices/asauthorizationpublickeycredentialprfregistrationinput-swift.struct
30. tavrn-web PR #99（2026-10-12）：https://github.com/just-tavrn/tavrn-web/pull/99
31. Android, Credential Manager in WebView（2026-02-26）：https://developer.android.com/identity/sign-in/credential-manager-webview
32. passkeys.dev, Android reference（2026-08-29）：https://passkeys.dev/docs/reference/android/
33. Android, Credential Manager prerequisites（2026-02-26）：https://developer.android.com/identity/credential-manager/prerequisites
34. androidx.credentials release notes：https://developer.android.com/jetpack/androidx/releases/credentials
35. `profiidev/tauri-plugin-webauthn`（npm 0.2.0，2025-05）：https://github.com/profiidev/tauri-plugin-webauthn
36. Bitwarden, Relying party PRF（无日期，© 2026）：https://contributing.bitwarden.com/architecture/deep-dives/passkeys/implementations/relying-party/prf
37. Bitwarden SDK, `webauthn_prf`：https://sdk-api-docs.bitwarden.com/bitwarden_core/key_management/webauthn_prf/index.html
38. 1Password 社区，Use of PRF extension（2024-01，早于半年）：https://www.1password.community/1password-at-home-31/use-of-prf-extension-20642
39. Tim Cappalli, Passkeys and PRF: a warning（2026-02-27）：https://blog.timcappalli.me/p/passkeys-prf-warning/
40. Apple, Share passwords and passkeys with a group：https://support.apple.com/guide/passwords/share-with-a-group-mchlc00a3602/mac
41. Google, Google Password Manager PIN：https://support.google.com/chrome/answer/16608973
42. Yubico, User handle：https://developers.yubico.com/WebAuthn/WebAuthn_Developer_Guide/User_Handle.html

上表中第 22、32 条只作旁证，正文未直接引用。
