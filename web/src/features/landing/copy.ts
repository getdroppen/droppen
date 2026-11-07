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
 * question answered from anywhere). Each section's headline carries its point;
 * supporting prose appears only where it adds a fact, and product screenshots
 * use framing instead of repeating themselves in captions.
 * Droppen Hub, relays, ciphertext and p50s come after the benefit, as its
 * support, never as the opening; the hero names none of them. End-to-end
 * encryption is said in one section near the end, and the rest of it is
 * /security's. "We" / "our servers" are Droppen Hub and the Droppen relays. A device is a 设备 / 裝置 / デバイス; the copy says 你的电脑
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

import i18n from "@/i18n";
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
  "Claude Code and Codex keep running on your own Macs and Linux machines. From your phone or a browser, see every session across them, answer their questions, send instructions and open a terminal. End-to-end encrypted, with an open-source client.":
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
  "Claude Code and Codex keep running on your own computers. From your phone or another device, check progress, answer questions, send instructions and open a terminal.":
    "Claude Code 和 Codex 仍在你自己的电脑上运行。在手机或其他设备上，就能查看进度、回答问题、发送指令、打开终端。",
  "Create a free account": "免费注册",
  "Sign up on the web": "在网页上注册",
  "Open web app": "打开网页版",
  "Join the web waitlist": "加入网页版等候名单",
  "Run Droppen on a Linux server": "Linux 服务器上运行 Droppen",
  "Linux and other platforms": "Linux 及其他平台",
  "Invite-only for now: sign-up needs an invite code.": "目前邀请制，注册需要邀请码。",
  "Sign-up is closed for now.": "目前暂停注册。",
  "Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer":
    "Mac 上的 Droppen：Claude Code 和 Codex 的会话列表，以及一个等你回答的会话",
  "Droppen with three panes: two sessions and a terminal": "Mac 上的 Droppen 分成三栏：两个会话和一个终端",
  "Droppen on a phone: what needs you, and a session's question": "手机上的 Droppen：「需要你」清单，以及一个会话的提问",
  "Full install instructions": "完整安装说明",

  // Your code, for your eyes only (the one section on encryption, near the end)
  "End-to-end encrypted": "端到端加密",
  "Security and encryption": "安全与加密",
  "Encrypted on your computer and decrypted only on your devices. Droppen Hub stores only encrypted copies and Droppen relays forward only encrypted data; neither holds a key that can decrypt them.":
    "内容在你的电脑上加密，只在你的设备上解密。Droppen Hub 保存的只是加密副本，Droppen 中继转发的只是加密数据，两者都没有解密所需的密钥。",
  "Only devices you approve can join": "只有你批准的设备才能加入",
  "A new phone or browser can join only when a device you already have approves it. If one is lost, remove it from another phone or browser of yours: it can't decrypt anything written afterwards, and what was already synced to it can't be recalled.":
    "新手机或新浏览器，需要你已有的设备批准才能加入。设备丢了，可以在你的其他手机或浏览器上把它移除：之后写的内容它解不开，已经同步到它上面的内容也收不回来。",
  "Open-source client": "客户端开源",
  "How the encryption works": "加密原理",
  "The droppen command-line program, the Mac app and the web app are open source under AGPL-3.0. Droppen Hub is not open source.":
    "droppen 命令行程序、Mac 应用和网页应用都是开源的，许可证是 AGPL-3.0。Droppen Hub 不开源。",
  "Read the source": "查看源代码",

  // Typing from your phone stays snappy (the direct path)
  "direct": "直连",

  // The network diagram
  "How it connects": "连接方式",
  "Direct · {{ms}}": "直连 · {{ms}}",
  "Relay · {{ms}}": "中继 · {{ms}}",
  "Switches within 1.5 s": "1.5 秒内自动切换",
  "Keys you type are sent again in order": "输入的按键会按原顺序重发",
  "Switching · 1.5 s": "切换 · 1.5 秒",

  // What you get
  "Every session in one list": "所有会话，一个列表",
  "All your computers are on one account. Every session that is running, waiting for an answer or has unread messages is in one list.":
    "所有电脑都在同一个账户里。正在运行、等待回答或有未读消息的会话，全都列在一起。",
  "Answer questions": "回答提问",
  "Answer the AI's questions, add instructions while it works, or stop it if it goes off track.":
    "回答 AI 的提问，在工作过程中随时补充要求，发现不对就停下来。",
  "Start a session from your phone": "在手机上新建会话",
  "Choose Claude Code or Codex, a computer and a folder, attach a screenshot if you want, and send.":
    "选择 Claude Code 或 Codex、一台电脑和一个文件夹，需要的话附上截图，然后发送。",
  "Browse files": "浏览文件",
  "Browse the folders on your computer and open files from your phone.": "在手机上查看电脑里的文件夹，打开文件。",
  "Scheduled tasks": "定时任务",
  "Run an instruction automatically on the computer you choose, for example every morning or every hour on weekdays.":
    "让你选的电脑按计划自动执行一条指令，比如每天早上，或工作日每小时一次。",

  // The beta's caveat, on /security.
  "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen 目前是测试版。加密设计经过了内部安全评审，还没有做过外部审计。在浏览器里使用时，你需要信任当次加载的页面代码：这份代码能使用你的私钥，也能把它们复制出去。",

  // Footer
  "Your AI keeps working on your computer. When it needs you, answer from your phone.": "AI 在你的电脑上继续工作，需要你时，在手机上即可回应",

  // The product page's composition: the proof line, the product as one
  // ensemble of three screens, the network, the questions, the plans.
  "Free plan": "免费版",
  "Product facts": "产品信息",
  "Choose a screen": "选择界面",
  "Screen {{n}} of {{total}}": "第 {{n}} 个界面，共 {{total}} 个",
  "Screen {{n}} of {{total}}: {{title}}": "第 {{n}} 个界面，共 {{total}} 个：{{title}}",
  "All computers, one list.": "所有电脑，一个列表。",
  "Your home Mac, the Linux machine at work and the server in the rack, on one account. Open it and see which one needs you, instead of signing in to each.":
    "家里的 Mac、公司的 Linux 电脑、机架上的服务器，都在同一个账户里。打开就能看到哪一台需要你，不用逐台登录。",
  "Code on the left, terminal on the right.": "左边代码，右边终端。",
  "When you need to watch two tasks, put a session and a terminal side by side. Changes, test output and errors in one window, with nothing copied and pasted.":
    "需要同时盯两件事时，把一个会话和终端并排。改动、测试输出和报错都在同一个窗口里，不用来回复制粘贴。",
  "Leave the computer, take over on your phone.": "离开电脑，在手机上接手。",
  "Open {{app}} in your phone's browser and the same sessions are there. The row marked “needs you” comes first; open it and answer.":
    "在手机浏览器里打开 {{app}}，还是那些会话。标着「需要你」的那一行排在最前，点开就能回答。",
  "What you can do from your phone.": "在手机上能做什么",
  "Read offline": "离线也能读",
  "When your computer is online, an encrypted copy is saved to your phone. If it is shut down, asleep or offline, you can still read the last {{free}} days ({{pro}} on Pro).":
    "电脑在线时，手机上会保存一份加密副本。电脑关机、休眠或断网时，你仍能查看最近 {{free}} 天（Pro 为 {{pro}} 天）的内容。",
  "Works when the connection is bad.": "网络不好也能用",
  "End-to-end encryption": "端到端加密",
  "Encrypted data only": "只有加密数据",
  "The key stays on this device": "密钥留在这台设备上",
  "Keys are exchanged only between your devices and never pass through Droppen Hub or a relay.":
    "密钥只在你的设备之间交换，不经过 Droppen Hub 或中继。",
  "Keys": "密钥",
  "Droppen Hub · encrypted copy": "Droppen Hub · 加密副本",
  "Decrypted on your phone": "在手机上解密",
  "Beta · the encryption design has had internal review.": "测试版 · 加密方式经过内部评审。",
  "Local only": "仅本地",
  "On your own computer, with no account.": "在你自己的电脑上使用，无需账户。",
  "1 device that runs AI": "1 台运行 AI 的设备",
  "No access from other devices": "不能跨设备使用",
  "Nothing saved to the cloud": "不保存到云端",
  "{{n}} devices that run AI": "{{n}} 台运行 AI 的设备",
  "{{devices}} devices, phones and browsers included": "共 {{devices}} 台设备（含手机和浏览器）",
  "{{free}} days of offline access · {{gb}} GB relay": "{{free}} 天离线查看 · {{gb}} GB 中继流量",
  "{{pro}} devices that run AI · {{devices}} devices": "{{pro}} 台运行 AI 的设备 · 共 {{devices}} 台设备",
  "{{pro}} days of offline access · {{gb}} GB relay": "{{pro}} 天离线查看 · {{gb}} GB 中继流量",
  "Compare all plans": "比较所有方案",
  "Recommended": "推荐",
  "USD / month": "美元 / 月",
  "Extra usage is billed": "超出部分按用量计费",
  "Choose Pro": "选择 Pro",
  "${{yearly}} a year, about {{saving}}% off · prices in US dollars": "年付 ${{yearly}}，约省 {{saving}}% · 价格以美元计",
  "Do I have to keep my computer on?": "电脑需要一直开着吗？",
  "No. The AI runs on your computer, but you can leave it. While it is offline, your phone can still read the last {{free}} days ({{pro}} on Pro) of synced sessions; sending messages and using the terminal wait until it is back online.":
    "不用。AI 在你的电脑上运行，但你可以离开。它离线时，手机仍可查看最近 {{free}} 天内同步过的会话（Pro 为 {{pro}} 天）；发送消息和使用终端，要等它重新联网。",
  "Is there a Windows client?": "有 Windows 客户端吗？",
  "Droppen's web app opens in a Windows browser, but the AI has to run on a Mac or Linux computer. A Windows client has not been released.":
    "Droppen 的网页应用可以在 Windows 浏览器中打开，但 AI 必须在 Mac 或 Linux 电脑上运行。Windows 客户端尚未发布。",
  "Will my code be uploaded?": "我的代码会上传吗？",
  "Session content is encrypted on your computer before it leaves. Droppen Hub keeps encrypted copies, and relays forward only encrypted data; neither has the key. In a browser, you trust the page code as it was loaded that time.":
    "会话内容在离开你的电脑前就已加密。Droppen Hub 保存加密副本，中继只转发加密数据，两者都没有密钥。在浏览器里，你需要信任当次加载的页面代码。",
  "Is the free plan enough?": "免费版够用吗？",
  "If you have up to {{hosts}} computers that run AI and need {{free}} days of offline access, the free plan is enough. Choose Pro for more devices, {{pro}} days offline or more relay traffic.":
    "如果你最多有 {{hosts}} 台运行 AI 的电脑，且只需要 {{free}} 天的离线查看，免费版就够。需要更多设备、{{pro}} 天离线或更多中继流量，就选 Pro。",
  "Product": "产品",
  "Resources": "资源",
  "Legal": "法律",
  "Interface": "界面",
  "Encryption design": "加密原理",
  "Back to top": "回到顶部",
  "Your AI, from your phone. End-to-end encrypted, open-source client.": "你的 AI，手机上接着用。端到端加密，客户端开源。",
  "macOS 13 or later · Linux · Windows in the browser · iOS and Android": "macOS 13 或更高 · Linux · Windows 用浏览器 · iOS 和 Android",
};

const zhTW: Bundle = {
  // Diagram cards: every row says something about the product.
  "Droppen relay": "Droppen 中繼",
  "relay": "中繼",
  "encrypted copy": "加密副本",

  "Linux server": "Linux 伺服器",

  "Droppen — your computer's AI, on your phone": "Droppen — 電腦上的 AI，手機上接著用",
  "Claude Code and Codex keep running on your own Macs and Linux machines. From your phone or a browser, see every session across them, answer their questions, send instructions and open a terminal. End-to-end encrypted, with an open-source client.":
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
  "Claude Code and Codex keep running on your own computers. From your phone or another device, check progress, answer questions, send instructions and open a terminal.":
    "Claude Code 和 Codex 仍在你自己的電腦上執行。在手機或其他裝置上，就能查看進度、回答問題、傳送指令、開啟終端機。",
  "Create a free account": "免費註冊",
  "Sign up on the web": "在網頁上註冊",
  "Open web app": "開啟網頁版",
  "Join the web waitlist": "加入網頁版等候名單",
  "Run Droppen on a Linux server": "在 Linux 伺服器上執行 Droppen",
  "Linux and other platforms": "Linux 及其他平台",
  "Invite-only for now: sign-up needs an invite code.": "目前採邀請制，註冊需要邀請碼。",
  "Sign-up is closed for now.": "目前暫停註冊。",
  "Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer":
    "Mac 上的 Droppen：Claude Code 和 Codex 的工作階段清單，以及一個等你回答的工作階段",
  "Droppen with three panes: two sessions and a terminal": "Mac 上的 Droppen 分成三欄：兩個工作階段和一個終端機",
  "Droppen on a phone: what needs you, and a session's question": "手機上的 Droppen：「需要你」清單，以及一個工作階段的提問",
  "Full install instructions": "完整安裝說明",

  "End-to-end encrypted": "端對端加密",
  "Security and encryption": "安全與加密",
  "Encrypted on your computer and decrypted only on your devices. Droppen Hub stores only encrypted copies and Droppen relays forward only encrypted data; neither holds a key that can decrypt them.":
    "內容在你的電腦上加密，只在你的裝置上解密。Droppen Hub 儲存的只是加密副本，Droppen 中繼轉送的只是加密資料，兩者都沒有解密所需的金鑰。",
  "Only devices you approve can join": "只有你核准的裝置才能加入",
  "A new phone or browser can join only when a device you already have approves it. If one is lost, remove it from another phone or browser of yours: it can't decrypt anything written afterwards, and what was already synced to it can't be recalled.":
    "新手機或新瀏覽器，需要你已有的裝置核准才能加入。裝置丟了，可以在你的其他手機或瀏覽器上將它移除：之後寫入的內容它解不開，已經同步到它上面的內容也收不回來。",
  "Open-source client": "客戶端開源",
  "How the encryption works": "加密原理",
  "The droppen command-line program, the Mac app and the web app are open source under AGPL-3.0. Droppen Hub is not open source.":
    "droppen 命令列程式、Mac 應用程式和網頁應用程式都是開源的，授權條款是 AGPL-3.0。Droppen Hub 不開源。",
  "Read the source": "查看原始碼",

  "direct": "直連",

  // The network diagram
  "How it connects": "連線方式",
  "Direct · {{ms}}": "直連 · {{ms}}",
  "Relay · {{ms}}": "中繼 · {{ms}}",
  "Switches within 1.5 s": "1.5 秒內自動切換",
  "Keys you type are sent again in order": "輸入的按鍵會按原順序重送",
  "Switching · 1.5 s": "切換 · 1.5 秒",
  "Every session in one list": "所有工作階段，一個清單",
  "All your computers are on one account. Every session that is running, waiting for an answer or has unread messages is in one list.":
    "所有電腦都在同一個帳戶裡。正在執行、等待回答或有未讀訊息的工作階段，全都列在一起。",
  "Answer questions": "回答提問",
  "Answer the AI's questions, add instructions while it works, or stop it if it goes off track.":
    "回答 AI 的提問，在執行過程中隨時補充要求，發現不對就停下來。",
  "Start a session from your phone": "在手機上新增工作階段",
  "Choose Claude Code or Codex, a computer and a folder, attach a screenshot if you want, and send.":
    "選擇 Claude Code 或 Codex、一台電腦和一個資料夾，需要的話附上截圖，然後送出。",
  "Browse files": "瀏覽檔案",
  "Browse the folders on your computer and open files from your phone.": "在手機上查看電腦裡的資料夾，開啟檔案。",
  "Scheduled tasks": "排程任務",
  "Run an instruction automatically on the computer you choose, for example every morning or every hour on weekdays.":
    "讓你選的電腦依排程自動執行一條指令，例如每天早上，或工作日每小時一次。",

  // The beta's caveat, on /security.
  "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen 目前是測試版。加密設計經過內部安全審查，還沒有做過外部稽核。在瀏覽器裡使用時，你需要信任當次載入的頁面程式碼：這份程式碼能使用你的私密金鑰，也能把它們複製出去。",

  "Your AI keeps working on your computer. When it needs you, answer from your phone.": "AI 在你的電腦上繼續工作，需要你時，在手機上即可回應",

  "Free plan": "免費版",
  "Product facts": "產品資訊",
  "Choose a screen": "選擇介面",
  "Screen {{n}} of {{total}}": "第 {{n}} 個介面，共 {{total}} 個",
  "Screen {{n}} of {{total}}: {{title}}": "第 {{n}} 個介面，共 {{total}} 個：{{title}}",
  "All computers, one list.": "所有電腦，一個清單。",
  "Your home Mac, the Linux machine at work and the server in the rack, on one account. Open it and see which one needs you, instead of signing in to each.":
    "家裡的 Mac、公司的 Linux 電腦、機架上的伺服器，都在同一個帳戶裡。打開就能看到哪一台需要你，不用逐台登入。",
  "Code on the left, terminal on the right.": "左邊程式碼，右邊終端機。",
  "When you need to watch two tasks, put a session and a terminal side by side. Changes, test output and errors in one window, with nothing copied and pasted.":
    "需要同時盯兩件事時，把一個工作階段和終端機並排。變更、測試輸出和錯誤都在同一個視窗裡，不用來回複製貼上。",
  "Leave the computer, take over on your phone.": "離開電腦，在手機上接手。",
  "Open {{app}} in your phone's browser and the same sessions are there. The row marked “needs you” comes first; open it and answer.":
    "在手機瀏覽器裡開啟 {{app}}，還是那些工作階段。標著「需要你」的那一列排在最前，點開就能回答。",
  "What you can do from your phone.": "在手機上能做什麼",
  "Read offline": "離線也能讀",
  "When your computer is online, an encrypted copy is saved to your phone. If it is shut down, asleep or offline, you can still read the last {{free}} days ({{pro}} on Pro).":
    "電腦在線時，手機會保存一份加密副本。電腦關機、休眠或斷網時，你仍能查看最近 {{free}} 天（Pro 為 {{pro}} 天）的內容。",
  "Works when the connection is bad.": "網路不好也能用",
  "End-to-end encryption": "端對端加密",
  "Encrypted data only": "只有加密資料",
  "The key stays on this device": "金鑰留在這台裝置上",
  "Keys are exchanged only between your devices and never pass through Droppen Hub or a relay.":
    "金鑰只在你的裝置之間交換，不經過 Droppen Hub 或中繼。",
  "Keys": "金鑰",
  "Droppen Hub · encrypted copy": "Droppen Hub · 加密副本",
  "Decrypted on your phone": "在手機上解密",
  "Beta · the encryption design has had internal review.": "測試版 · 加密方式經過內部審查。",
  "Local only": "僅本地",
  "On your own computer, with no account.": "在你自己的電腦上使用，無需帳戶。",
  "1 device that runs AI": "1 台執行 AI 的裝置",
  "No access from other devices": "無法跨裝置使用",
  "Nothing saved to the cloud": "不儲存到雲端",
  "{{n}} devices that run AI": "{{n}} 台執行 AI 的裝置",
  "{{devices}} devices, phones and browsers included": "共 {{devices}} 台裝置（含手機和瀏覽器）",
  "{{free}} days of offline access · {{gb}} GB relay": "{{free}} 天離線查看 · {{gb}} GB 中繼流量",
  "{{pro}} devices that run AI · {{devices}} devices": "{{pro}} 台執行 AI 的裝置 · 共 {{devices}} 台裝置",
  "{{pro}} days of offline access · {{gb}} GB relay": "{{pro}} 天離線查看 · {{gb}} GB 中繼流量",
  "Compare all plans": "比較所有方案",
  "Recommended": "推薦",
  "USD / month": "美元 / 月",
  "Extra usage is billed": "超出部分按用量計費",
  "Choose Pro": "選擇 Pro",
  "${{yearly}} a year, about {{saving}}% off · prices in US dollars": "年付 ${{yearly}}，約省 {{saving}}% · 價格以美元計",
  "Do I have to keep my computer on?": "電腦需要一直開著嗎？",
  "No. The AI runs on your computer, but you can leave it. While it is offline, your phone can still read the last {{free}} days ({{pro}} on Pro) of synced sessions; sending messages and using the terminal wait until it is back online.":
    "不用。AI 在你的電腦上執行，但你可以離開。它離線時，手機仍可查看最近 {{free}} 天內同步過的工作階段（Pro 為 {{pro}} 天）；傳送訊息和使用終端機，要等它重新連上網路。",
  "Is there a Windows client?": "有 Windows 客戶端嗎？",
  "Droppen's web app opens in a Windows browser, but the AI has to run on a Mac or Linux computer. A Windows client has not been released.":
    "Droppen 的網頁應用程式可以在 Windows 瀏覽器中開啟，但 AI 必須在 Mac 或 Linux 電腦上執行。Windows 客戶端尚未發布。",
  "Will my code be uploaded?": "我的程式碼會上傳嗎？",
  "Session content is encrypted on your computer before it leaves. Droppen Hub keeps encrypted copies, and relays forward only encrypted data; neither has the key. In a browser, you trust the page code as it was loaded that time.":
    "工作階段內容在離開你的電腦前就已加密。Droppen Hub 儲存加密副本，中繼只轉送加密資料，兩者都沒有金鑰。在瀏覽器裡，你需要信任當次載入的頁面程式碼。",
  "Is the free plan enough?": "免費版夠用嗎？",
  "If you have up to {{hosts}} computers that run AI and need {{free}} days of offline access, the free plan is enough. Choose Pro for more devices, {{pro}} days offline or more relay traffic.":
    "如果你最多有 {{hosts}} 台執行 AI 的電腦，且只需要 {{free}} 天的離線查看，免費版就夠。需要更多裝置、{{pro}} 天離線或更多中繼流量，就選 Pro。",
  "Product": "產品",
  "Resources": "資源",
  "Legal": "法律",
  "Interface": "介面",
  "Encryption design": "加密原理",
  "Back to top": "回到頂部",
  "Your AI, from your phone. End-to-end encrypted, open-source client.": "你的 AI，手機上接著用。端對端加密，客戶端開源。",
  "macOS 13 or later · Linux · Windows in the browser · iOS and Android": "macOS 13 或以上 · Linux · Windows 用瀏覽器 · iOS 和 Android",
};

const ja: Bundle = {
  // Diagram cards: every row says something about the product.
  "Droppen relay": "Droppen 中継",
  "relay": "中継",
  "encrypted copy": "暗号化コピー",

  "Linux server": "Linux サーバー",

  "Droppen — your computer's AI, on your phone": "Droppen — コンピュータの AI を、スマホで続きから",
  "Claude Code and Codex keep running on your own Macs and Linux machines. From your phone or a browser, see every session across them, answer their questions, send instructions and open a terminal. End-to-end encrypted, with an open-source client.":
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
  "Claude Code and Codex keep running on your own computers. From your phone or another device, check progress, answer questions, send instructions and open a terminal.":
    "Claude Code と Codex は、自分のコンピュータで動いたまま。スマホやほかのデバイスから、進捗の確認、質問への回答、指示の送信、ターミナルを開くことができます。",
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
  "Droppen on a phone: what needs you, and a session's question": "スマホの Droppen：「対応待ち」リストとセッションの質問",
  "Full install instructions": "インストール手順をすべて見る",

  "End-to-end encrypted": "エンドツーエンド暗号化",
  "Security and encryption": "セキュリティと暗号化",
  "Encrypted on your computer and decrypted only on your devices. Droppen Hub stores only encrypted copies and Droppen relays forward only encrypted data; neither holds a key that can decrypt them.":
    "暗号化はあなたのコンピュータで、復号はあなたのデバイスでだけ。Droppen Hub が保存するのは暗号化コピーだけ、Droppen 中継が転送するのは暗号化データだけで、どちらも復号に必要な鍵を持っていません。",
  "Only devices you approve can join": "加われるのは、承認したデバイスだけ",
  "A new phone or browser can join only when a device you already have approves it. If one is lost, remove it from another phone or browser of yours: it can't decrypt anything written afterwards, and what was already synced to it can't be recalled.":
    "新しいスマホやブラウザは、手持ちのデバイスが承認したときだけ加われます。なくしたときは、ほかのスマホやブラウザから外せます。外した後に書かれたものは復号できませんが、それまでに同期された内容は取り戻せません。",
  "Open-source client": "クライアントはオープンソース",
  "How the encryption works": "暗号化の仕組み",
  "The droppen command-line program, the Mac app and the web app are open source under AGPL-3.0. Droppen Hub is not open source.":
    "droppen コマンドラインプログラム、Mac アプリ、Web アプリは AGPL-3.0 のオープンソースです。Droppen Hub はオープンソースではありません。",
  "Read the source": "ソースを見る",

  "direct": "直結",

  // The network diagram
  "How it connects": "接続方法",
  "Direct · {{ms}}": "直結 · {{ms}}",
  "Relay · {{ms}}": "中継 · {{ms}}",
  "Switches within 1.5 s": "1.5 秒以内に自動で切り替え",
  "Keys you type are sent again in order": "入力したキーは順番どおりに再送されます",
  "Switching · 1.5 s": "切り替え · 1.5 秒",
  "Every session in one list": "すべてのセッションを、ひとつのリストに",
  "All your computers are on one account. Every session that is running, waiting for an answer or has unread messages is in one list.":
    "すべてのコンピュータを同じアカウントに登録できます。実行中、回答待ち、未読のあるセッションが、ひとつのリストに並びます。",
  "Answer questions": "質問に答える",
  "Answer the AI's questions, add instructions while it works, or stop it if it goes off track.":
    "AI の質問に答えたり、作業中に指示を追加したり、おかしくなったら止めたりできます。",
  "Start a session from your phone": "スマホからセッションを開始",
  "Choose Claude Code or Codex, a computer and a folder, attach a screenshot if you want, and send.":
    "Claude Code か Codex、コンピュータ、フォルダを選び、必要ならスクリーンショットを添えて送信します。",
  "Browse files": "ファイルを見る",
  "Browse the folders on your computer and open files from your phone.": "スマホからコンピュータのフォルダを見て、ファイルを開けます。",
  "Scheduled tasks": "定時実行",
  "Run an instruction automatically on the computer you choose, for example every morning or every hour on weekdays.":
    "選んだコンピュータで、決めた時間に自動で指示を実行できます。たとえば毎朝、平日は 1 時間ごとなど。",

  // The beta's caveat, on /security.
  "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen はベータ版です。暗号設計は社内のセキュリティレビューを受けていますが、外部監査はまだです。ブラウザで使う場合は、そのとき読み込んだページのコードを信頼することになります。そのコードは秘密鍵を使えるうえ、持ち出すこともできます。",

  "Your AI keeps working on your computer. When it needs you, answer from your phone.": "AI はコンピュータで作業を続け、呼ばれたらスマホで応えられます。",

  "Free plan": "無料プラン",
  "Product facts": "製品情報",
  "Choose a screen": "画面を選ぶ",
  "Screen {{n}} of {{total}}": "{{total}} 画面中 {{n}} 画面目",
  "Screen {{n}} of {{total}}: {{title}}": "{{total}} 画面中 {{n}} 画面目：{{title}}",
  "All computers, one list.": "すべてのコンピュータを、ひとつのリストで。",
  "Your home Mac, the Linux machine at work and the server in the rack, on one account. Open it and see which one needs you, instead of signing in to each.":
    "自宅の Mac、職場の Linux、ラックのサーバーをひとつのアカウントに。開けば、どれがあなたを待っているかが一度に分かり、1 台ずつログインする必要はありません。",
  "Code on the left, terminal on the right.": "左にコード、右にターミナル。",
  "When you need to watch two tasks, put a session and a terminal side by side. Changes, test output and errors in one window, with nothing copied and pasted.":
    "2 つの作業を見たいときは、セッションとターミナルを並べます。変更、テスト出力、エラーが 1 つのウィンドウに収まり、コピー＆ペーストの往復は要りません。",
  "Leave the computer, take over on your phone.": "コンピュータを離れて、スマホで引き継ぐ。",
  "Open {{app}} in your phone's browser and the same sessions are there. The row marked “needs you” comes first; open it and answer.":
    "スマホのブラウザで {{app}} を開けば、同じセッションがそこに。「対応が必要」と書かれた行が先頭にあり、開いて答えられます。",
  "What you can do from your phone.": "スマートフォンでできること",
  "Read offline": "オフラインでも読めます",
  "When your computer is online, an encrypted copy is saved to your phone. If it is shut down, asleep or offline, you can still read the last {{free}} days ({{pro}} on Pro).":
    "コンピュータがオンラインのとき、スマートフォンに暗号化したコピーが保存されます。電源が切れていても、スリープ中でも、オフラインでも、直近 {{free}} 日分（Pro は {{pro}} 日分）を読めます。",
  "Works when the connection is bad.": "通信が不安定でも使えます",
  "End-to-end encryption": "エンドツーエンド暗号化",
  "Encrypted data only": "暗号化データだけ",
  "The key stays on this device": "鍵はこのデバイスにとどまります",
  "Keys are exchanged only between your devices and never pass through Droppen Hub or a relay.":
    "鍵はあなたのデバイス間でだけ交換され、Droppen Hub や中継を経由しません。",
  "Keys": "鍵",
  "Droppen Hub · encrypted copy": "Droppen Hub · 暗号化コピー",
  "Decrypted on your phone": "スマホで復号",
  "Beta · the encryption design has had internal review.": "ベータ版 · 暗号化の方式は社内レビュー済み。",
  "Local only": "ローカルのみ",
  "On your own computer, with no account.": "自分のコンピュータだけで、アカウント不要。",
  "1 device that runs AI": "AI を動かすデバイス 1 台",
  "No access from other devices": "ほかのデバイスから使えない",
  "Nothing saved to the cloud": "クラウドに保存しない",
  "{{n}} devices that run AI": "AI を動かすデバイス {{n}} 台",
  "{{devices}} devices, phones and browsers included": "デバイス {{devices}} 台（スマホ・ブラウザを含む）",
  "{{free}} days of offline access · {{gb}} GB relay": "オフライン閲覧 {{free}} 日 · 中継 {{gb}} GB",
  "{{pro}} devices that run AI · {{devices}} devices": "AI を動かすデバイス {{pro}} 台 · デバイス {{devices}} 台",
  "{{pro}} days of offline access · {{gb}} GB relay": "オフライン閲覧 {{pro}} 日 · 中継 {{gb}} GB",
  "Compare all plans": "すべてのプランを比較",
  "Recommended": "おすすめ",
  "USD / month": "米ドル / 月",
  "Extra usage is billed": "超過分は従量課金",
  "Choose Pro": "Pro を選ぶ",
  "${{yearly}} a year, about {{saving}}% off · prices in US dollars": "年額 ${{yearly}}、約 {{saving}}% お得 · 料金は米ドル",
  "Do I have to keep my computer on?": "コンピュータはつけっぱなしにする必要がありますか？",
  "No. The AI runs on your computer, but you can leave it. While it is offline, your phone can still read the last {{free}} days ({{pro}} on Pro) of synced sessions; sending messages and using the terminal wait until it is back online.":
    "いいえ。AI はあなたのコンピュータで動きますが、離れていても大丈夫です。オフラインの間も、スマホで直近 {{free}} 日分（Pro は {{pro}} 日）の同期済みセッションを読めます。送信とターミナルは、オンラインに戻るまで待ちます。",
  "Is there a Windows client?": "Windows クライアントはありますか？",
  "Droppen's web app opens in a Windows browser, but the AI has to run on a Mac or Linux computer. A Windows client has not been released.":
    "Droppen の Web アプリは Windows のブラウザで開けますが、AI は Mac か Linux のコンピュータで動かす必要があります。Windows クライアントは未リリースです。",
  "Will my code be uploaded?": "コードはアップロードされますか？",
  "Session content is encrypted on your computer before it leaves. Droppen Hub keeps encrypted copies, and relays forward only encrypted data; neither has the key. In a browser, you trust the page code as it was loaded that time.":
    "セッションの内容は、コンピュータを出る前に暗号化されます。Droppen Hub は暗号化コピーを保存し、中継は暗号化データだけを転送します。どちらも鍵を持ちません。ブラウザでは、そのとき読み込まれたページのコードを信頼することになります。",
  "Is the free plan enough?": "無料プランで足りますか？",
  "If you have up to {{hosts}} computers that run AI and need {{free}} days of offline access, the free plan is enough. Choose Pro for more devices, {{pro}} days offline or more relay traffic.":
    "AI を動かすコンピュータが {{hosts}} 台までで、オフライン閲覧が {{free}} 日分あれば無料プランで足ります。デバイスを増やす、{{pro}} 日分にする、中継の転送量を増やすなら Pro を。",
  "Product": "製品",
  "Resources": "リソース",
  "Legal": "法務",
  "Interface": "画面",
  "Encryption design": "暗号化の仕組み",
  "Back to top": "ページ上部へ",
  "Your AI, from your phone. End-to-end encrypted, open-source client.": "あなたの AI を、スマホで続きから。エンドツーエンド暗号化、クライアントはオープンソース。",
  "macOS 13 or later · Linux · Windows in the browser · iOS and Android": "macOS 13 以降 · Linux · Windows はブラウザ · iOS と Android",
};

/** The landing page's tables, by i18next language. */
export const landingBundles: Record<"zh-CN" | "zh-TW" | "ja", Bundle> = {
  "zh-CN": { ...zhCN, ...downloadZhCN, ...pricingZhCN },
  "zh-TW": { ...zhTW, ...downloadZhTW, ...pricingZhTW },
  ja: { ...ja, ...downloadJa, ...pricingJa },
};

// Added here, never overwriting: a key the app already translates keeps its wording. The words
// register themselves where they live, so a page that imports them cannot forget the merge.
for (const [lng, bundle] of Object.entries(landingBundles)) i18n.addResourceBundle(lng, "translation", bundle, true, false);
