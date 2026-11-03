/*
 * The landing page's words. As everywhere in the app (`src/i18n`), the keys
 * are the English text, so English needs no table. zh-CN is the page's first
 * language, written as native copy rather than translated from the English;
 * zh-TW follows the app's Taiwan wording (工作階段, 轉送, 資料, 裝置) and ja
 * its Japanese (直結, 中継経由, デバイス, 対応待ち).
 *
 * The voice is a product page's for teams that adopt it: plain, precise and
 * restrained, never chatty (运行, not 跑; 继续工作, not 接着干; 无法解密, not
 * 读不了), and every action says what it does and to what (免费注册, 回答 AI 的
 * 提问, 打开电脑上的终端). Every headline says what the reader gets (their
 * computer's AI on their phone, every computer in one list, the agent's
 * question answered from anywhere). The composition is the product page's:
 * a small caps line names each section, the headline says what the reader
 * gets, one sentence of support follows, and every product screenshot gets a
 * one-line caption for the thing on it the words are about. Droppen Hub,
 * relays, ciphertext and p50s come after the benefit, as its support, never as
 * the opening; the hero names none of them. End-to-end encryption is said in
 * one section near the end, and the rest of it is /security's. "We" / "our servers" are Droppen Hub and the
 * Droppen relays. A device is a 设备 / 裝置 / デバイス; the copy says 你的电脑
 * only for the reader's own physical computer, and never 机器.
 *
 * Whatever is about the path (passing ciphertext on, the detour, the
 * fallback, a round trip) is a Droppen relay (docs/relay-and-plans.md
 * §2.14): "Droppen relay" / "Droppen 中继" / "Droppen 中繼" / "Droppen 中継",
 * or the bare word in a path tag, and no relay's region is named. Droppen
 * Hub forwards nothing: it finds your computers and keeps the encrypted
 * copies, and is "Droppen Hub" in every language.
 *
 * Loaded with the landing chunk and added to i18next without overwriting, so
 * a key the app already translates ("online", "direct") keeps the app's
 * wording.
 *
 * Every claim here is one the build makes good on (docs/cross-machine-
 * architecture.md §0, §2, §5, §7, §9; the devices dashboard and its
 * 「需要你」 list in features/cloud/my-devices.tsx and machines/needs-you.ts;
 * the notifications in crates/droppen-server/src/push; a new session in
 * features/sessions/new-session-view.tsx; the files in features/files;
 * scheduled prompts in crates/droppen-server/src/schedule; the offline window, the plan's
 * retention, in crates/droppen-agent/src/streams/retention.rs): numbers are the measured
 * p50s, nothing says "unhackable" or "audited", and "open source" only ever
 * names the client (AGPL-3.0), never Droppen Hub or Droppen as a whole.
 */

import { downloadJa, downloadZhCN, downloadZhTW } from "./download-copy";
import { pricingJa, pricingZhCN, pricingZhTW } from "./pricing-copy";

type Bundle = Record<string, string>;

const zhCN: Bundle = {
  // Diagram cards: every row says something about the product.
  "Droppen relay": "Droppen 中继",
  "relay": "中继",
  "encrypted copy": "加密副本",

  "Linux server": "Linux 服务器",

  // Page
  "Droppen — your computer's AI, on your phone": "Droppen — 电脑上的 AI，手机上接着用",
  "Claude Code and Codex keep running on your own Macs and Linux machines. From your phone or a browser, see every session across them, answer their questions, steer them and open a terminal. End-to-end encrypted, with an open-source client.":
    "Claude Code 和 Codex 仍在你自己的 Mac 和 Linux 电脑上运行。在手机或浏览器中查看所有电脑上的会话，回答提问、发送指令、打开终端。端到端加密，客户端开源。",
  "On this page": "本页",
  "Site navigation": "网站导航",
  "Security": "安全",
  "Client source code": "客户端源代码",
  "Third-party notices": "第三方软件声明",
  "How it works": "工作原理",
  "Open app": "打开应用",
  "Sign in": "登录",

  // Hero
  "Your computer's AI, from your phone.": "电脑上的 AI，手机上接着用",
  "Claude Code and Codex keep running on your own computers. From a browser on your phone or another device, answer their questions, steer them and open a terminal on the computer.":
    "Claude Code 和 Codex 仍在你自己的电脑上运行。在手机或任意设备的浏览器中，随时查看进度、回复提问、发送指令，并直接调出本地终端。",
  "Create a free account": "免费注册",
  "Sign up on the web": "在网页上免费注册",
  "Open web app": "打开网页版",
  "Join the web waitlist": "加入网页版等候名单",
  "Run Droppen on a Linux server": "Linux 服务器上运行 Droppen",
  "Linux and other platforms": "Linux 及其他平台",
  "Invite-only for now: sign-up needs an invite code.": "目前邀请制，注册需要邀请码。",
  "Sign-up is closed for now.": "目前暂停注册。",
  "Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer":
    "Mac 上的 Droppen：Claude Code 和 Codex 的会话列表，以及一个等你回答的会话",
  "Droppen with three panes: two sessions and a terminal": "Mac 上的 Droppen 分成三栏：两个会话和一个终端",
  "Phone": "移动端",
  "Droppen on a phone: what needs you, and a session's question": "手机上的 Droppen：「需要你」清单，以及一个会话的提问",
  "Full install instructions": "完整安装说明",

  // Your code, for your eyes only (the one section on encryption, near the end)
  "End-to-end encrypted": "端到端加密",
  "Droppen can't read your code.": "你的代码，Droppen 无法读取",
  "Encrypted on your computer and decrypted only on your devices. Droppen Hub stores only encrypted copies and Droppen relays forward only ciphertext; neither holds a key that can decrypt them.":
    "内容在你的电脑上加密，只在你的设备上解密。Droppen Hub 保存的只是加密副本，Droppen 中继转发的只是加密数据，两者都没有解密所需的密钥。",
  "Only devices you approve can join": "只有你批准的设备才能加入",
  "A new phone or browser can join only when a device you already have approves it. If one is lost, remove it from another phone or browser of yours: it can't decrypt anything written afterwards, and what was already synced to it can't be recalled.":
    "新手机或新浏览器，须经你已有的设备批准才能加入。设备丢失时，可在你的其他手机或浏览器上将它移除：此后写入的内容它无法解密，移除前已同步到它上面的内容无法收回。",
  "Open-source client": "客户端开源",
  "How the encryption works": "加密设计说明",
  "The droppen command-line program, the Mac app and the web app are open source under AGPL-3.0. Droppen Hub is not open source.":
    "droppen 命令行程序、Mac 应用和网页应用均以 AGPL-3.0 许可证开源。Droppen Hub 不开源。",
  "Read the source": "查看源代码",

  // Typing from your phone stays snappy (the direct path)
  "Keystrokes go straight to your computer when your phone can reach it, and through a Droppen relay when it can't; if the direct path drops, the relay takes over within 1.5 s and resends unconfirmed keystrokes in order, so none are lost.":
    "手机能直接连到你的电脑时，按键经直连送达；连不上时由 Droppen 中继转发。直连中断后，中继在 1.5 秒内接管，并按原顺序重发尚未确认的按键，不会丢失。",
  "direct": "直连",

  // The network diagram
  "The direct and relay paths between your phone and your computer": "手机与电脑之间的直连和中继路径",
  "Your phone reaches your computer directly when it can; when it cannot, it goes through a Droppen relay. The relay takes over within 1.5 s and unconfirmed keys are resent in order.":
    "手机能直连电脑时直连；连不上时经 Droppen 中继。中继在断线 1.5 秒内接管，未确认的按键按原顺序重发。",
  "Connection path · direct first": "连接路径 · 直连优先",
  "P2P / direct first": "P2P / 直连优先",
  "direct first": "直连优先",
  "Any network": "任意网络",
  "Fallback · ciphertext only": "连不上才用 · 只转发密文",
  "The session runs here": "会话在这台电脑上运行",
  "Direct · {{ms}}": "直连 · {{ms}}",
  "Relay · {{ms}}": "中继 · {{ms}}",
  "Switches within 1.5 s": "断线 1.5 秒内切换",
  "Unconfirmed keys resend in order": "未确认的按键按原顺序重发",

  // What you get
  "Every computer, one list": "所有电脑，一个列表",
  "Home and work computers on one account, with every session that is waiting for an answer, has unread messages or is running in one list.":
    "多台电脑绑定同一账户。所有正在运行、等待回复或有未读更新的会话，汇总在同一个列表中一览无余。",
  "Answer and steer": "回答提问，中途干预",
  "Answer the AI's questions, add a message while it works, or stop the current run if it goes the wrong way.":
    "回答 AI 的提问；在它生成中随时追加补充指令，若发现偏离预期可立即中止。",
  "Start a session from your phone": "在手机上新建会话",
  "Choose Claude Code or Codex, a computer and a folder, optionally attach a screenshot, and send.": "选择 Claude Code 或 Codex、电脑和文件夹，可附上截图，然后发送。",
  "Browse your files": "浏览项目文件",
  "Browse the folders on your computer and preview files from your phone.": "在手机上浏览电脑上的文件夹，并预览文件内容。",
  "Run prompts on a schedule": "按计划定时执行指令",
  "Have a prompt run automatically on the computer you choose, for example every morning or every hour on weekdays.":
    "让发给 AI 的 Prompt 按预设计划在指定的电脑上自动运行，例如每个工作日早晨定时触发。",

  // The beta's caveat, on /security.
  "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen 目前是测试版。加密设计经过了内部安全评审，还没有做过外部审计。在浏览器里使用时，你需要信任当次加载的页面代码：这份代码能使用你的私钥，也能把它们复制出去。",

  // Footer
  "Your AI keeps working on your computer. When it needs you, answer from your phone.": "AI 在你的电脑上继续工作，需要你时，在手机上即可回应",

  // The product page's composition: the proof line, the product as one
  // ensemble, the three screens, the network, the questions, the plans.
  "Free to start": "免费开始",
  "No credit card": "无需信用卡",
  "Product facts": "产品信息",
  "Product interface": "产品界面",
  "What it actually looks like.": "它到底长什么样。",
  "No new workflow to learn. The sessions, terminal and projects you know, carried on from another screen.":
    "不用学新的工作流。你熟悉的会话、终端和项目，换一块屏幕继续。",
  "Queue": "队列",
  "Every computer, one queue.": "每台电脑，都在同一个队列里。",
  "Your home Mac, the Linux machine at work and the server in the rack, on one account. Open it and see which one needs you, instead of signing in to each.":
    "家里的 Mac、公司的 Linux 电脑、机架上的服务器，都在同一个账户里。打开就能看到哪一台需要你，不用逐台登录。",
  "Amber is “waiting for you”; green is “working”.": "琥珀色是「等你回答」，绿色是「工作中」。",
  "Unread, running and waiting for an answer, sorted by state.": "未读、运行中、等你回答，按状态排序。",
  "Every device's state in one column; an amber row left alone stops the AI there.":
    "每台设备的状态排成一列；琥珀色的一行没人处理，那台电脑上的 AI 就会停下来等你。",
  "Waiting for you / working": "等你回答 / 工作中",
  "Split": "分屏",
  "Code on the left, terminal on the right.": "左边代码，右边终端。",
  "When you need to watch two tasks, put a session and a terminal side by side. Changes, test output and errors in one window, with nothing copied and pasted.":
    "需要同时盯两件事时，把一个会话和终端并排。改动、测试输出和报错都在同一个窗口里，不用来回复制粘贴。",
  "Two independent sessions that do not interrupt each other.": "两个独立的会话，互不打断。",
  "The terminal is a full session, not a read-only log.": "终端是完整的会话，不是只读日志。",
  "The terminal, diffs and test results stay in one view; the mouse does not fly between windows.":
    "终端、diff 和测试结果都在同一视图里，鼠标不用在窗口间飞来飞去。",
  "The terminal is right there": "终端就在旁边",
  "Leave the computer, take over on your phone.": "离开电脑，在手机上接手。",
  "Open {{app}} in your phone's browser and the same sessions are there. The amber “needs you” row comes first; open it and answer.":
    "在手机浏览器里打开 {{app}}，还是那些会话。琥珀色的「需要你」排在最前，点开就能回答。",
  "Answer questions, add instructions, stop a run.": "回答问题，补充指令，中止运行。",
  "With the computer off, the last {{free}} days stay readable ({{pro}} on Pro).": "电脑关机后，最近 {{free}} 天的内容仍可查看（Pro 为 {{pro}} 天）。",
  "Only what needs you": "只处理需要你的",
  "The phone is not a shrunken desktop: the conclusion first, the detail after.": "手机不是缩小的桌面：结论先行，细节随后。",
  "Why Droppen": "为什么选 Droppen",
  "Leaving your computer is not leaving the work.": "离开电脑，不是离开工作。",
  "The AI keeps running on your computer; you only step in for the few seconds that need you.":
    "AI 继续在你的电脑上运行，你只需在需要你的那几秒里介入。",
  "Readable with the lid closed": "合上盖子也能读",
  "While the computer is online, an encrypted copy is synced. Shut down, asleep or offline, your phone can still read the last {{free}} days ({{pro}} on Pro).":
    "电脑在线时同步加密副本。关机、休眠或断网后，手机仍可查看最近 {{free}} 天（Pro 为 {{pro}} 天）的内容。",
  "Built for real networks": "为真实网络而做",
  "A weak network does not stop the work.": "网络不稳，工作不断。",
  "Direct median round trip": "直连往返中位数",
  "Through a Droppen relay": "经 Droppen 中继",
  "Keys lost": "丢失按键",
  "Resent in order after a drop": "断线后按原顺序重发",
  "The key boundary of end-to-end encryption": "端到端加密的密钥边界",
  "Your computer and your phone each hold a key. Droppen Hub stores only encrypted copies, and relays forward only ciphertext; neither has the key.":
    "你的电脑和手机各持一把密钥。Droppen Hub 只保存加密副本，中继只转发加密数据，两者都没有密钥。",
  "Key boundary · end-to-end encryption": "密钥边界 · 端到端加密",
  "Your computer": "你的电脑",
  "Your phone": "你的手机",
  "Encrypted here": "在此加密",
  "Decrypted here": "在此解密",
  "Only encrypted copies": "只有加密副本",
  "Only ciphertext": "只有加密数据",
  "End-to-end encrypted · ciphertext": "端到端加密 · 加密数据",
  "The key stays on this device": "密钥留在这台设备上",
  "No key": "没有密钥",
  "Keys are negotiated between your devices and never pass through Droppen Hub or a relay.":
    "密钥只在你的设备之间协商，从不经过 Droppen Hub 或中继。",
  "The key is made on your computer and never leaves your devices. Everything Droppen keeps or passes on is already sealed.":
    "密钥在你的电脑上生成，从不离开你的设备。Droppen 保存或转发的所有内容，都已经密封。",
  "Beta · the encryption design has had internal review.": "测试版 · 加密设计经过内部评审。",
  "Start free.": "免费开始。",
  "Using it locally needs no account. To go across devices, free is enough to start.":
    "在本地使用无需账户。要跨设备使用，免费方案就够开始。",
  "Local only": "仅本地",
  "On your own computer, with no account.": "在你自己的电脑上使用，无需账户。",
  "1 device that runs AI": "1 台运行 AI 的设备",
  "No cross-device access": "无法跨设备访问",
  "No cloud copy": "没有云端副本",
  "Where cross-device starts.": "跨设备使用的起点。",
  "{{n}} devices that run AI": "{{n}} 台运行 AI 的设备",
  "{{devices}} devices, phones and browsers included": "共 {{devices}} 台设备（含手机和浏览器）",
  "{{free}} days of offline copy · {{gb}} GB relay": "{{free}} 天离线副本 · {{gb}} GB 中继流量",
  "{{pro}} devices that run AI · {{devices}} devices": "{{pro}} 台运行 AI 的设备 · 共 {{devices}} 台设备",
  "{{pro}} days of offline copy · {{gb}} GB relay": "{{pro}} 天离线副本 · {{gb}} GB 中继流量",
  "Compare all plans": "比较所有方案",
  "Recommended": "推荐",
  "USD / month": "美元 / 月",
  "For people who move between devices every day.": "给每天在多台设备间切换的人。",
  "Billed by use beyond that": "超出后按用量计费",
  "Choose Pro": "选择 Pro",
  "${{yearly}} a year, about {{saving}}% off · prices in US dollars": "年付 ${{yearly}}，约省 {{saving}}% · 价格以美元计",
  "Questions and answers": "常见问题",
  "Before you start.": "开始之前。",
  "Do I have to keep my computer on?": "电脑需要一直开着吗？",
  "No. The AI runs on your computer, but you can leave it. While it is offline, your phone can still read the last {{free}} days ({{pro}} on Pro) of synced sessions; sending messages and using the terminal wait until it is back online.":
    "不用。AI 在你的电脑上运行，但你可以离开。它离线时，手机仍可查看最近 {{free}} 天内同步过的会话（Pro 为 {{pro}} 天）；发送消息和使用终端，要等它重新上线。",
  "Is there a Windows client?": "有 Windows 客户端吗？",
  "Droppen's web app opens in a Windows browser, but the AI has to run on a Mac or Linux computer. A Windows client has not been released.":
    "Droppen 的网页应用可以在 Windows 浏览器中打开，但 AI 必须在 Mac 或 Linux 电脑上运行。Windows 客户端尚未发布。",
  "Will my code be uploaded?": "我的代码会上传吗？",
  "Session content is encrypted on your computer before it leaves. Droppen Hub keeps encrypted copies, and relays forward only ciphertext; neither has the key. In a browser, you trust the page code as it was loaded that time.":
    "会话内容在离开你的电脑前就已加密。Droppen Hub 保存加密副本，中继只转发加密数据，两者都没有密钥。在浏览器里，你需要信任当次加载的页面代码。",
  "Is the free plan enough?": "免费方案够用吗？",
  "If you have up to {{hosts}} computers that run AI and need {{free}} days of offline reading, free is enough. Choose Pro for more devices, {{pro}} days offline or more relay transfer.":
    "如果你最多有 {{hosts}} 台运行 AI 的电脑，且只需要 {{free}} 天的离线阅读，免费方案就够。需要更多设备、{{pro}} 天离线或更多中继流量，就选 Pro。",
  "Product": "产品",
  "Resources": "资源",
  "Legal": "法律",
  "Interface": "界面",
  "Encryption design": "加密设计",
  "Back to top": "回到顶部",
  "Free to start. Install once, then carry on from any of your devices.": "免费开始。安装一次，之后在哪台设备上都能继续。",
  "Your AI, from your phone. End-to-end encrypted, open-source client.": "你的 AI，手机上接着用。端到端加密，客户端开源。",
  "macOS 13 or later · Linux · Windows in the browser · iOS and Android": "macOS 13 或更高 · Linux · Windows 用浏览器 · iOS 和 Android",
  "macOS and Linux run Droppen natively · Windows, iPad, iPhone and Android in the browser · one account, and your sessions follow":
    "macOS 和 Linux 原生运行 Droppen · Windows、iPad、iPhone 和 Android 用浏览器 · 一个账户，会话跟着你走",
};

const zhTW: Bundle = {
  // Diagram cards: every row says something about the product.
  "Droppen relay": "Droppen 中繼",
  "relay": "中繼",
  "encrypted copy": "加密副本",

  "Linux server": "Linux 伺服器",

  "Droppen — your computer's AI, on your phone": "Droppen — 電腦上的 AI，手機上接著用",
  "Claude Code and Codex keep running on your own Macs and Linux machines. From your phone or a browser, see every session across them, answer their questions, steer them and open a terminal. End-to-end encrypted, with an open-source client.":
    "Claude Code 和 Codex 仍在你自己的 Mac 和 Linux 電腦上執行。在手機或瀏覽器中查看所有電腦上的工作階段，回答提問、傳送指令、開啟終端機。端對端加密，客戶端開源。",
  "On this page": "本頁",
  "Site navigation": "網站導覽",
  "Security": "安全",
  "Client source code": "客戶端原始碼",
  "Third-party notices": "第三方軟體聲明",
  "How it works": "運作方式",
  "Open app": "打開應用",
  "Sign in": "登入",

  "Your computer's AI, from your phone.": "電腦上的 AI，手機上接著用",
  "Claude Code and Codex keep running on your own computers. From a browser on your phone or another device, answer their questions, steer them and open a terminal on the computer.":
    "Claude Code 和 Codex 仍在你自己的電腦上執行。在手機或其他裝置的瀏覽器中，回答它們的提問、傳送指令、開啟電腦上的終端機。",
  "Create a free account": "免費註冊",
  "Sign up on the web": "在網頁上免費註冊",
  "Open web app": "開啟網頁版",
  "Join the web waitlist": "加入網頁版等候名單",
  "Run Droppen on a Linux server": "在 Linux 伺服器上執行 Droppen",
  "Linux and other platforms": "Linux 及其他平台",
  "Invite-only for now: sign-up needs an invite code.": "目前採邀請制，註冊需要邀請碼。",
  "Sign-up is closed for now.": "目前暫停註冊。",
  "Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer":
    "Mac 上的 Droppen：Claude Code 和 Codex 的工作階段清單，以及一個等你回答的工作階段",
  "Droppen with three panes: two sessions and a terminal": "Mac 上的 Droppen 分成三欄：兩個工作階段和一個終端機",
  "Phone": "手機",
  "Droppen on a phone: what needs you, and a session's question": "手機上的 Droppen：「需要你」清單，以及一個工作階段的提問",
  "Full install instructions": "完整安裝說明",

  "End-to-end encrypted": "端對端加密",
  "Droppen can't read your code.": "你的程式碼，Droppen 無法讀取",
  "Encrypted on your computer and decrypted only on your devices. Droppen Hub stores only encrypted copies and Droppen relays forward only ciphertext; neither holds a key that can decrypt them.":
    "內容在你的電腦上加密，只在你的裝置上解密。Droppen Hub 儲存的只是加密副本，Droppen 中繼轉送的只是加密資料，兩者都沒有解密所需的金鑰。",
  "Only devices you approve can join": "只有你核准的裝置才能加入",
  "A new phone or browser can join only when a device you already have approves it. If one is lost, remove it from another phone or browser of yours: it can't decrypt anything written afterwards, and what was already synced to it can't be recalled.":
    "新手機或新瀏覽器，須經你已有的裝置核准才能加入。裝置遺失時，可在你的其他手機或瀏覽器上將它移除：此後寫入的內容它無法解密，移除前已同步到它上面的內容無法收回。",
  "Open-source client": "客戶端開源",
  "How the encryption works": "加密設計說明",
  "The droppen command-line program, the Mac app and the web app are open source under AGPL-3.0. Droppen Hub is not open source.":
    "droppen 命令列程式、Mac 應用程式和網頁應用程式均以 AGPL-3.0 授權開源。Droppen Hub 不開源。",
  "Read the source": "查看原始碼",

  "Keystrokes go straight to your computer when your phone can reach it, and through a Droppen relay when it can't; if the direct path drops, the relay takes over within 1.5 s and resends unconfirmed keystrokes in order, so none are lost.":
    "手機能直接連到你的電腦時，按鍵經直連送達；連不上時由 Droppen 中繼轉送。直連中斷後，中繼在 1.5 秒內接手，並按原順序重送尚未確認的按鍵，不會遺失。",
  "direct": "直連",

  // The network diagram
  "The direct and relay paths between your phone and your computer": "手機與電腦之間的直連和中繼路徑",
  "Your phone reaches your computer directly when it can; when it cannot, it goes through a Droppen relay. The relay takes over within 1.5 s and unconfirmed keys are resent in order.":
    "手機能直連電腦時直連；連不上時經 Droppen 中繼。中繼在斷線 1.5 秒內接手，未確認的按鍵按原順序重送。",
  "Connection path · direct first": "連線路徑 · 直連優先",
  "P2P / direct first": "P2P / 直連優先",
  "direct first": "直連優先",
  "Any network": "任意網路",
  "Fallback · ciphertext only": "連不上才用 · 只轉發密文",
  "The session runs here": "工作階段在這台電腦上執行",
  "Direct · {{ms}}": "直連 · {{ms}}",
  "Relay · {{ms}}": "中繼 · {{ms}}",
  "Switches within 1.5 s": "斷線 1.5 秒內切換",
  "Unconfirmed keys resend in order": "未確認的按鍵按原順序重送",

  "Every computer, one list": "所有電腦，一個清單",
  "Home and work computers on one account, with every session that is waiting for an answer, has unread messages or is running in one list.":
    "家裡和公司的電腦登入同一個帳戶，等你回答、有未讀訊息和正在執行的工作階段匯整在同一個清單中。",
  "Answer and steer": "回答提問，中途介入",
  "Answer the AI's questions, add a message while it works, or stop the current run if it goes the wrong way.": "回答 AI 的提問，在它執行時追加訊息，方向不對時可以停止目前的執行。",
  "Start a session from your phone": "在手機上新增工作階段",
  "Choose Claude Code or Codex, a computer and a folder, optionally attach a screenshot, and send.": "選擇 Claude Code 或 Codex、電腦和資料夾，可附上截圖，然後送出。",
  "Browse your files": "瀏覽專案檔案",
  "Browse the folders on your computer and preview files from your phone.": "在手機上瀏覽電腦上的資料夾，並預覽檔案內容。",
  "Run prompts on a schedule": "排程任務",
  "Have a prompt run automatically on the computer you choose, for example every morning or every hour on weekdays.":
    "讓一個給 AI 的指令按排程在指定的電腦上自動執行，例如每天早上或工作日每小時。",

  // The beta's caveat, on /security.
  "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen 目前是測試版。加密設計經過內部安全審查，還沒有做過外部稽核。在瀏覽器裡使用時，你需要信任當次載入的頁面程式碼：這份程式碼能使用你的私密金鑰，也能把它們複製出去。",

  "Your AI keeps working on your computer. When it needs you, answer from your phone.": "AI 在你的電腦上繼續工作，需要你時，在手機上即可回應",

  "Free to start": "免費開始",
  "No credit card": "無需信用卡",
  "Product facts": "產品資訊",
  "Product interface": "產品介面",
  "What it actually looks like.": "它實際長什麼樣。",
  "No new workflow to learn. The sessions, terminal and projects you know, carried on from another screen.":
    "不用學新的工作流程。你熟悉的工作階段、終端機和專案，換一塊螢幕繼續。",
  "Queue": "佇列",
  "Every computer, one queue.": "每台電腦，都在同一個佇列裡。",
  "Your home Mac, the Linux machine at work and the server in the rack, on one account. Open it and see which one needs you, instead of signing in to each.":
    "家裡的 Mac、公司的 Linux 電腦、機架上的伺服器，都在同一個帳戶裡。打開就能看到哪一台需要你，不用逐台登入。",
  "Amber is “waiting for you”; green is “working”.": "琥珀色是「等你回答」，綠色是「工作中」。",
  "Unread, running and waiting for an answer, sorted by state.": "未讀、執行中、等你回答，依狀態排序。",
  "Every device's state in one column; an amber row left alone stops the AI there.":
    "每台裝置的狀態排成一欄；琥珀色的一列沒人處理，那台電腦上的 AI 就會停下來等你。",
  "Waiting for you / working": "等你回答 / 工作中",
  "Split": "分割畫面",
  "Code on the left, terminal on the right.": "左邊程式碼，右邊終端機。",
  "When you need to watch two tasks, put a session and a terminal side by side. Changes, test output and errors in one window, with nothing copied and pasted.":
    "需要同時盯兩件事時，把一個工作階段和終端機並排。變更、測試輸出和錯誤都在同一個視窗裡，不用來回複製貼上。",
  "Two independent sessions that do not interrupt each other.": "兩個獨立的工作階段，互不干擾。",
  "The terminal is a full session, not a read-only log.": "終端機是完整的工作階段，不是唯讀日誌。",
  "The terminal, diffs and test results stay in one view; the mouse does not fly between windows.":
    "終端機、diff 和測試結果都在同一個檢視裡，滑鼠不用在視窗間飛來飛去。",
  "The terminal is right there": "終端機就在旁邊",
  "Leave the computer, take over on your phone.": "離開電腦，在手機上接手。",
  "Open {{app}} in your phone's browser and the same sessions are there. The amber “needs you” row comes first; open it and answer.":
    "在手機瀏覽器裡打開 {{app}}，還是那些工作階段。琥珀色的「需要你」排在最前，點開就能回答。",
  "Answer questions, add instructions, stop a run.": "回答問題，補充指令，中止執行。",
  "With the computer off, the last {{free}} days stay readable ({{pro}} on Pro).": "電腦關機後，最近 {{free}} 天的內容仍可查看（Pro 為 {{pro}} 天）。",
  "Only what needs you": "只處理需要你的",
  "The phone is not a shrunken desktop: the conclusion first, the detail after.": "手機不是縮小的桌面：結論先行，細節隨後。",
  "Why Droppen": "為什麼選 Droppen",
  "Leaving your computer is not leaving the work.": "離開電腦，不是離開工作。",
  "The AI keeps running on your computer; you only step in for the few seconds that need you.":
    "AI 繼續在你的電腦上執行，你只需在需要你的那幾秒裡介入。",
  "Readable with the lid closed": "闔上蓋子也能讀",
  "While the computer is online, an encrypted copy is synced. Shut down, asleep or offline, your phone can still read the last {{free}} days ({{pro}} on Pro).":
    "電腦在線時同步加密副本。關機、休眠或斷網後，手機仍可查看最近 {{free}} 天（Pro 為 {{pro}} 天）的內容。",
  "Built for real networks": "為真實網路而做",
  "A weak network does not stop the work.": "網路不穩，工作不斷。",
  "Direct median round trip": "直連往返中位數",
  "Through a Droppen relay": "經 Droppen 中繼",
  "Keys lost": "遺失按鍵",
  "Resent in order after a drop": "斷線後按原順序重送",
  "The key boundary of end-to-end encryption": "端對端加密的金鑰邊界",
  "Your computer and your phone each hold a key. Droppen Hub stores only encrypted copies, and relays forward only ciphertext; neither has the key.":
    "你的電腦和手機各持一把金鑰。Droppen Hub 只儲存加密副本，中繼只轉送加密資料，兩者都沒有金鑰。",
  "Key boundary · end-to-end encryption": "金鑰邊界 · 端對端加密",
  "Your computer": "你的電腦",
  "Your phone": "你的手機",
  "Encrypted here": "在此加密",
  "Decrypted here": "在此解密",
  "Only encrypted copies": "只有加密副本",
  "Only ciphertext": "只有加密資料",
  "End-to-end encrypted · ciphertext": "端對端加密 · 加密資料",
  "The key stays on this device": "金鑰留在這台裝置上",
  "No key": "沒有金鑰",
  "Keys are negotiated between your devices and never pass through Droppen Hub or a relay.":
    "金鑰只在你的裝置之間協商，從不經過 Droppen Hub 或中繼。",
  "The key is made on your computer and never leaves your devices. Everything Droppen keeps or passes on is already sealed.":
    "金鑰在你的電腦上產生，從不離開你的裝置。Droppen 儲存或轉送的所有內容，都已經密封。",
  "Beta · the encryption design has had internal review.": "測試版 · 加密設計經過內部審查。",
  "Start free.": "免費開始。",
  "Using it locally needs no account. To go across devices, free is enough to start.":
    "在本地使用無需帳戶。要跨裝置使用，免費方案就夠開始。",
  "Local only": "僅本地",
  "On your own computer, with no account.": "在你自己的電腦上使用，無需帳戶。",
  "1 device that runs AI": "1 台執行 AI 的裝置",
  "No cross-device access": "無法跨裝置存取",
  "No cloud copy": "沒有雲端副本",
  "Where cross-device starts.": "跨裝置使用的起點。",
  "{{n}} devices that run AI": "{{n}} 台執行 AI 的裝置",
  "{{devices}} devices, phones and browsers included": "共 {{devices}} 台裝置（含手機和瀏覽器）",
  "{{free}} days of offline copy · {{gb}} GB relay": "{{free}} 天離線副本 · {{gb}} GB 中繼流量",
  "{{pro}} devices that run AI · {{devices}} devices": "{{pro}} 台執行 AI 的裝置 · 共 {{devices}} 台裝置",
  "{{pro}} days of offline copy · {{gb}} GB relay": "{{pro}} 天離線副本 · {{gb}} GB 中繼流量",
  "Compare all plans": "比較所有方案",
  "Recommended": "推薦",
  "USD / month": "美元 / 月",
  "For people who move between devices every day.": "給每天在多台裝置間切換的人。",
  "Billed by use beyond that": "超出後依用量計費",
  "Choose Pro": "選擇 Pro",
  "${{yearly}} a year, about {{saving}}% off · prices in US dollars": "年付 ${{yearly}}，約省 {{saving}}% · 價格以美元計",
  "Questions and answers": "常見問題",
  "Before you start.": "開始之前。",
  "Do I have to keep my computer on?": "電腦需要一直開著嗎？",
  "No. The AI runs on your computer, but you can leave it. While it is offline, your phone can still read the last {{free}} days ({{pro}} on Pro) of synced sessions; sending messages and using the terminal wait until it is back online.":
    "不用。AI 在你的電腦上執行，但你可以離開。它離線時，手機仍可查看最近 {{free}} 天內同步過的工作階段（Pro 為 {{pro}} 天）；傳送訊息和使用終端機，要等它重新上線。",
  "Is there a Windows client?": "有 Windows 客戶端嗎？",
  "Droppen's web app opens in a Windows browser, but the AI has to run on a Mac or Linux computer. A Windows client has not been released.":
    "Droppen 的網頁應用程式可以在 Windows 瀏覽器中開啟，但 AI 必須在 Mac 或 Linux 電腦上執行。Windows 客戶端尚未發布。",
  "Will my code be uploaded?": "我的程式碼會上傳嗎？",
  "Session content is encrypted on your computer before it leaves. Droppen Hub keeps encrypted copies, and relays forward only ciphertext; neither has the key. In a browser, you trust the page code as it was loaded that time.":
    "工作階段內容在離開你的電腦前就已加密。Droppen Hub 儲存加密副本，中繼只轉送加密資料，兩者都沒有金鑰。在瀏覽器裡，你需要信任當次載入的頁面程式碼。",
  "Is the free plan enough?": "免費方案夠用嗎？",
  "If you have up to {{hosts}} computers that run AI and need {{free}} days of offline reading, free is enough. Choose Pro for more devices, {{pro}} days offline or more relay transfer.":
    "如果你最多有 {{hosts}} 台執行 AI 的電腦，且只需要 {{free}} 天的離線閱讀，免費方案就夠。需要更多裝置、{{pro}} 天離線或更多中繼流量，就選 Pro。",
  "Product": "產品",
  "Resources": "資源",
  "Legal": "法律",
  "Interface": "介面",
  "Encryption design": "加密設計",
  "Back to top": "回到頂部",
  "Free to start. Install once, then carry on from any of your devices.": "免費開始。安裝一次，之後在哪台裝置上都能繼續。",
  "Your AI, from your phone. End-to-end encrypted, open-source client.": "你的 AI，手機上接著用。端對端加密，客戶端開源。",
  "macOS 13 or later · Linux · Windows in the browser · iOS and Android": "macOS 13 或以上 · Linux · Windows 用瀏覽器 · iOS 和 Android",
  "macOS and Linux run Droppen natively · Windows, iPad, iPhone and Android in the browser · one account, and your sessions follow":
    "macOS 和 Linux 原生執行 Droppen · Windows、iPad、iPhone 和 Android 用瀏覽器 · 一個帳戶，工作階段跟著你走",
};

const ja: Bundle = {
  // Diagram cards: every row says something about the product.
  "Droppen relay": "Droppen 中継",
  "relay": "中継",
  "encrypted copy": "暗号化コピー",

  "Linux server": "Linux サーバー",

  "Droppen — your computer's AI, on your phone": "Droppen — コンピュータの AI を、スマホで続きから",
  "Claude Code and Codex keep running on your own Macs and Linux machines. From your phone or a browser, see every session across them, answer their questions, steer them and open a terminal. End-to-end encrypted, with an open-source client.":
    "Claude Code と Codex は自分の Mac や Linux で動いたまま。スマホやブラウザから全コンピュータのセッションを確認し、質問に答え、指示を送り、ターミナルを開けます。エンドツーエンド暗号化、クライアントはオープンソース。",
  "On this page": "このページ",
  "Site navigation": "サイトナビゲーション",
  "Security": "セキュリティ",
  "Client source code": "クライアントのソースコード",
  "Third-party notices": "サードパーティ通知",
  "How it works": "しくみ",
  "Open app": "アプリを開く",
  "Sign in": "ログイン",

  "Your computer's AI, from your phone.": "コンピュータの AI を、スマホで続きから。",
  "Claude Code and Codex keep running on your own computers. From a browser on your phone or another device, answer their questions, steer them and open a terminal on the computer.":
    "Claude Code と Codex は、自分のコンピュータで動いたまま。スマホやほかのデバイスのブラウザから、質問に答え、指示を送り、そのコンピュータのターミナルを開けます。",
  "Create a free account": "無料で登録",
  "Sign up on the web": "Web で登録",
  "Open web app": "Web アプリを開く",
  "Join the web waitlist": "Web ウェイトリストに登録",
  "Run Droppen on a Linux server": "Linux サーバーで Droppen を実行",
  "Linux and other platforms": "Linux とほかのプラットフォーム",
  "Invite-only for now: sign-up needs an invite code.": "いまは招待制です。登録には招待コードが必要です。",
  "Sign-up is closed for now.": "現在、新規登録を停止しています。",
  "Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer":
    "Mac の Droppen：Claude Code と Codex のセッション一覧と、回答を待つセッション",
  "Droppen with three panes: two sessions and a terminal": "Mac の Droppen を 3 ペインに：2 つのセッションとターミナル",
  "Phone": "スマホ",
  "Droppen on a phone: what needs you, and a session's question": "スマホの Droppen：「対応待ち」リストとセッションの質問",
  "Full install instructions": "インストール手順をすべて見る",

  "End-to-end encrypted": "エンドツーエンド暗号化",
  "Droppen can't read your code.": "あなたのコードを、Droppen は読めません。",
  "Encrypted on your computer and decrypted only on your devices. Droppen Hub stores only encrypted copies and Droppen relays forward only ciphertext; neither holds a key that can decrypt them.":
    "暗号化はあなたのコンピュータで、復号はあなたのデバイスでだけ。Droppen Hub が保存するのは暗号化コピーだけ、Droppen 中継が転送するのは暗号文だけで、どちらも復号に必要な鍵を持っていません。",
  "Only devices you approve can join": "加われるのは、承認したデバイスだけ",
  "A new phone or browser can join only when a device you already have approves it. If one is lost, remove it from another phone or browser of yours: it can't decrypt anything written afterwards, and what was already synced to it can't be recalled.":
    "新しいスマホやブラウザは、手持ちのデバイスが承認したときだけ加われます。なくしたときは、ほかのスマホやブラウザから外せます。外した後に書かれたものは復号できませんが、それまでに同期された内容は取り戻せません。",
  "Open-source client": "クライアントはオープンソース",
  "How the encryption works": "暗号化の仕組み",
  "The droppen command-line program, the Mac app and the web app are open source under AGPL-3.0. Droppen Hub is not open source.":
    "droppen コマンドラインプログラム、Mac アプリ、Web アプリは AGPL-3.0 のオープンソースです。Droppen Hub はオープンソースではありません。",
  "Read the source": "ソースを見る",

  "Keystrokes go straight to your computer when your phone can reach it, and through a Droppen relay when it can't; if the direct path drops, the relay takes over within 1.5 s and resends unconfirmed keystrokes in order, so none are lost.":
    "スマホからコンピュータに直結できればキー入力は直接届き、できなければ Droppen 中継が転送します。直結が切れても 1.5 秒以内に中継が引き継ぎ、未確認のキー入力を順番どおりに再送するので、失われません。",
  "direct": "直結",

  // The network diagram
  "The direct and relay paths between your phone and your computer": "スマホとコンピュータの直結・中継経路",
  "Your phone reaches your computer directly when it can; when it cannot, it goes through a Droppen relay. The relay takes over within 1.5 s and unconfirmed keys are resent in order.":
    "スマホは直結できるときはコンピュータへ直接つながり、できないときは Droppen 中継を経由します。中継は切断から 1.5 秒以内に切り替わり、未確認のキーは順番どおりに再送されます。",
  "Connection path · direct first": "接続経路 · 直結優先",
  "P2P / direct first": "P2P / 直結優先",
  "direct first": "直結優先",
  "Any network": "任意のネットワーク",
  "Fallback · ciphertext only": "直結できない時だけ · 暗号文だけを転送",
  "The session runs here": "セッションはこのコンピュータで動きます",
  "Direct · {{ms}}": "直結 · {{ms}}",
  "Relay · {{ms}}": "中継 · {{ms}}",
  "Switches within 1.5 s": "切断から 1.5 秒以内に切り替え",
  "Unconfirmed keys resend in order": "未確認のキーは順番どおりに再送",

  "Every computer, one list": "すべてのコンピュータを、ひとつのリストで",
  "Home and work computers on one account, with every session that is waiting for an answer, has unread messages or is running in one list.":
    "自宅と職場のコンピュータをひとつのアカウントに。回答待ち、未読あり、実行中のセッションがひとつのリストにまとまります。",
  "Answer and steer": "回答と軌道修正",
  "Answer the AI's questions, add a message while it works, or stop the current run if it goes the wrong way.": "AI の質問に答える、作業中にメッセージを追加する、方向が違えば実行を止める。",
  "Start a session from your phone": "スマホからセッションを開始",
  "Choose Claude Code or Codex, a computer and a folder, optionally attach a screenshot, and send.":
    "Claude Code か Codex、コンピュータとフォルダを選び、必要ならスクリーンショットを添えて送信。",
  "Browse your files": "ファイルを閲覧",
  "Browse the folders on your computer and preview files from your phone.": "スマホからコンピュータのフォルダを開き、ファイルの中身をプレビューできます。",
  "Run prompts on a schedule": "スケジュール実行",
  "Have a prompt run automatically on the computer you choose, for example every morning or every hour on weekdays.":
    "AI への指示を、選んだコンピュータで決まった時間に自動実行。たとえば毎朝、平日は毎時など。",

  // The beta's caveat, on /security.
  "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen はベータ版です。暗号設計は社内のセキュリティレビューを受けていますが、外部監査はまだです。ブラウザで使う場合は、そのとき読み込んだページのコードを信頼することになります。そのコードは秘密鍵を使えるうえ、持ち出すこともできます。",

  "Your AI keeps working on your computer. When it needs you, answer from your phone.": "AI はコンピュータで作業を続け、呼ばれたらスマホで応えられます。",

  "Free to start": "無料で始められる",
  "No credit card": "クレジットカード不要",
  "Product facts": "製品情報",
  "Product interface": "製品画面",
  "What it actually looks like.": "実際の画面はこうなっています。",
  "No new workflow to learn. The sessions, terminal and projects you know, carried on from another screen.":
    "新しいワークフローを覚える必要はありません。使い慣れたセッション、ターミナル、プロジェクトを、別の画面からそのまま続けられます。",
  "Queue": "キュー",
  "Every computer, one queue.": "すべてのコンピュータが、ひとつのキューに。",
  "Your home Mac, the Linux machine at work and the server in the rack, on one account. Open it and see which one needs you, instead of signing in to each.":
    "自宅の Mac、職場の Linux、ラックのサーバーをひとつのアカウントに。開けば、どれがあなたを待っているかが一度に分かり、1 台ずつログインする必要はありません。",
  "Amber is “waiting for you”; green is “working”.": "琥珀色が「あなた待ち」、緑が「作業中」。",
  "Unread, running and waiting for an answer, sorted by state.": "未読、実行中、回答待ちを状態ごとに並べます。",
  "Every device's state in one column; an amber row left alone stops the AI there.":
    "すべてのデバイスの状態が一列に。琥珀色の行を放置すると、そのコンピュータの AI はあなたを待って止まります。",
  "Waiting for you / working": "回答待ち / 作業中",
  "Split": "分割",
  "Code on the left, terminal on the right.": "左にコード、右にターミナル。",
  "When you need to watch two tasks, put a session and a terminal side by side. Changes, test output and errors in one window, with nothing copied and pasted.":
    "2 つの作業を見たいときは、セッションとターミナルを並べます。変更、テスト出力、エラーが 1 つのウィンドウに収まり、コピー＆ペーストの往復は要りません。",
  "Two independent sessions that do not interrupt each other.": "互いに邪魔をしない、独立した 2 つのセッション。",
  "The terminal is a full session, not a read-only log.": "ターミナルは読み取り専用のログではなく、完全なセッションです。",
  "The terminal, diffs and test results stay in one view; the mouse does not fly between windows.":
    "ターミナル、diff、テスト結果が 1 つの画面に。ウィンドウ間でマウスを行き来させません。",
  "The terminal is right there": "ターミナルはすぐ隣",
  "Leave the computer, take over on your phone.": "コンピュータを離れて、スマホで引き継ぐ。",
  "Open {{app}} in your phone's browser and the same sessions are there. The amber “needs you” row comes first; open it and answer.":
    "スマホのブラウザで {{app}} を開けば、同じセッションがそこに。「対応待ち」の琥珀色の行が先頭にあり、開いて答えられます。",
  "Answer questions, add instructions, stop a run.": "質問に答える、指示を足す、実行を止める。",
  "With the computer off, the last {{free}} days stay readable ({{pro}} on Pro).": "コンピュータの電源が切れていても、直近 {{free}} 日分は読めます（Pro は {{pro}} 日）。",
  "Only what needs you": "あなたが必要なものだけ",
  "The phone is not a shrunken desktop: the conclusion first, the detail after.": "スマホは縮小したデスクトップではありません。まず結論、次に詳細。",
  "Why Droppen": "Droppen を選ぶ理由",
  "Leaving your computer is not leaving the work.": "コンピュータを離れても、仕事を離れることにはなりません。",
  "The AI keeps running on your computer; you only step in for the few seconds that need you.":
    "AI はコンピュータで動き続け、あなたは必要な数秒だけ手を貸します。",
  "Readable with the lid closed": "フタを閉じても読める",
  "While the computer is online, an encrypted copy is synced. Shut down, asleep or offline, your phone can still read the last {{free}} days ({{pro}} on Pro).":
    "コンピュータがオンラインの間に暗号化コピーを同期。電源オフ、スリープ、オフラインでも、直近 {{free}} 日分（Pro は {{pro}} 日）をスマホで読めます。",
  "Built for real networks": "現実のネットワークのために",
  "A weak network does not stop the work.": "ネットワークが不安定でも、仕事は止まりません。",
  "Direct median round trip": "直結の往復中央値",
  "Through a Droppen relay": "Droppen 中継経由",
  "Keys lost": "失われたキー",
  "Resent in order after a drop": "切断後も順番どおりに再送",
  "The key boundary of end-to-end encryption": "エンドツーエンド暗号化の鍵の境界",
  "Your computer and your phone each hold a key. Droppen Hub stores only encrypted copies, and relays forward only ciphertext; neither has the key.":
    "あなたのコンピュータとスマホがそれぞれ鍵を持ちます。Droppen Hub は暗号化コピーだけを保存し、中継は暗号文だけを転送します。どちらも鍵を持ちません。",
  "Key boundary · end-to-end encryption": "鍵の境界 · エンドツーエンド暗号化",
  "Your computer": "あなたのコンピュータ",
  "Your phone": "あなたのスマホ",
  "Encrypted here": "ここで暗号化",
  "Decrypted here": "ここで復号",
  "Only encrypted copies": "暗号化コピーだけ",
  "Only ciphertext": "暗号文だけ",
  "End-to-end encrypted · ciphertext": "エンドツーエンド暗号化 · 暗号文",
  "The key stays on this device": "鍵はこのデバイスにとどまります",
  "No key": "鍵なし",
  "Keys are negotiated between your devices and never pass through Droppen Hub or a relay.":
    "鍵はあなたのデバイス間で交換され、Droppen Hub や中継を経由しません。",
  "The key is made on your computer and never leaves your devices. Everything Droppen keeps or passes on is already sealed.":
    "鍵はあなたのコンピュータで作られ、デバイスの外に出ません。Droppen が保存・転送するものは、すべて密封済みです。",
  "Beta · the encryption design has had internal review.": "ベータ版 · 暗号設計は社内レビュー済み。",
  "Start free.": "無料で始めましょう。",
  "Using it locally needs no account. To go across devices, free is enough to start.":
    "ローカルで使うならアカウント不要。デバイスをまたぐなら、無料プランから始められます。",
  "Local only": "ローカルのみ",
  "On your own computer, with no account.": "自分のコンピュータだけで、アカウント不要。",
  "1 device that runs AI": "AI を動かすデバイス 1 台",
  "No cross-device access": "デバイス間のアクセスなし",
  "No cloud copy": "クラウドコピーなし",
  "Where cross-device starts.": "デバイスをまたぐのはここから。",
  "{{n}} devices that run AI": "AI を動かすデバイス {{n}} 台",
  "{{devices}} devices, phones and browsers included": "デバイス {{devices}} 台（スマホ・ブラウザを含む）",
  "{{free}} days of offline copy · {{gb}} GB relay": "オフラインコピー {{free}} 日 · 中継 {{gb}} GB",
  "{{pro}} devices that run AI · {{devices}} devices": "AI を動かすデバイス {{pro}} 台 · デバイス {{devices}} 台",
  "{{pro}} days of offline copy · {{gb}} GB relay": "オフラインコピー {{pro}} 日 · 中継 {{gb}} GB",
  "Compare all plans": "すべてのプランを比較",
  "Recommended": "おすすめ",
  "USD / month": "米ドル / 月",
  "For people who move between devices every day.": "毎日デバイスをまたいで動く人に。",
  "Billed by use beyond that": "超過分は従量課金",
  "Choose Pro": "Pro を選ぶ",
  "${{yearly}} a year, about {{saving}}% off · prices in US dollars": "年額 ${{yearly}}、約 {{saving}}% お得 · 料金は米ドル",
  "Questions and answers": "よくある質問",
  "Before you start.": "始める前に。",
  "Do I have to keep my computer on?": "コンピュータはつけっぱなしにする必要がありますか？",
  "No. The AI runs on your computer, but you can leave it. While it is offline, your phone can still read the last {{free}} days ({{pro}} on Pro) of synced sessions; sending messages and using the terminal wait until it is back online.":
    "いいえ。AI はあなたのコンピュータで動きますが、離れていても大丈夫です。オフラインの間も、スマホで直近 {{free}} 日分（Pro は {{pro}} 日）の同期済みセッションを読めます。送信とターミナルは、オンラインに戻るまで待ちます。",
  "Is there a Windows client?": "Windows クライアントはありますか？",
  "Droppen's web app opens in a Windows browser, but the AI has to run on a Mac or Linux computer. A Windows client has not been released.":
    "Droppen の Web アプリは Windows のブラウザで開けますが、AI は Mac か Linux のコンピュータで動かす必要があります。Windows クライアントは未リリースです。",
  "Will my code be uploaded?": "コードはアップロードされますか？",
  "Session content is encrypted on your computer before it leaves. Droppen Hub keeps encrypted copies, and relays forward only ciphertext; neither has the key. In a browser, you trust the page code as it was loaded that time.":
    "セッションの内容は、コンピュータを出る前に暗号化されます。Droppen Hub は暗号化コピーを保存し、中継は暗号文だけを転送します。どちらも鍵を持ちません。ブラウザでは、そのとき読み込まれたページのコードを信頼することになります。",
  "Is the free plan enough?": "無料プランで足りますか？",
  "If you have up to {{hosts}} computers that run AI and need {{free}} days of offline reading, free is enough. Choose Pro for more devices, {{pro}} days offline or more relay transfer.":
    "AI を動かすコンピュータが {{hosts}} 台までで、オフライン閲覧が {{free}} 日分あれば無料プランで足ります。デバイスを増やす、{{pro}} 日分にする、中継の転送量を増やすなら Pro を。",
  "Product": "製品",
  "Resources": "リソース",
  "Legal": "法務",
  "Interface": "画面",
  "Encryption design": "暗号設計",
  "Back to top": "ページ上部へ",
  "Free to start. Install once, then carry on from any of your devices.": "無料で始められます。一度インストールすれば、どのデバイスからでも続けられます。",
  "Your AI, from your phone. End-to-end encrypted, open-source client.": "あなたの AI を、スマホで続きから。エンドツーエンド暗号化、クライアントはオープンソース。",
  "macOS 13 or later · Linux · Windows in the browser · iOS and Android": "macOS 13 以降 · Linux · Windows はブラウザ · iOS と Android",
  "macOS and Linux run Droppen natively · Windows, iPad, iPhone and Android in the browser · one account, and your sessions follow":
    "macOS と Linux はネイティブ、Windows・iPad・iPhone・Android はブラウザで。ひとつのアカウントで、セッションがついてきます。",
};

/** The landing page's tables, by i18next language. */
export const landingBundles: Record<"zh-CN" | "zh-TW" | "ja", Bundle> = {
  "zh-CN": { ...zhCN, ...downloadZhCN, ...pricingZhCN },
  "zh-TW": { ...zhTW, ...downloadZhTW, ...pricingZhTW },
  ja: { ...ja, ...downloadJa, ...pricingJa },
};
