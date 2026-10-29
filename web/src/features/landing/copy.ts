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
 * question answered from anywhere). The layout does the
 * explaining: no caps line over a headline, no caption for what a screenshot
 * or a diagram already shows, and a line under a headline only for what the
 * layout cannot show (how to get it, the numbers, what is encrypted), one
 * sentence or two. Droppen Hub, relays, ciphertext and p50s come after the
 * benefit, as its support, never as the opening; the hero names none of
 * them. End-to-end encryption is said in one section near the end, and the
 * rest of it is /security's. "We" / "our servers" are Droppen Hub and the
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
 * the notifications in server-rs/src/push; a new session in
 * features/sessions/new-session-view.tsx; the files in features/files;
 * scheduled prompts in server-rs/src/schedule; the offline window, the plan's
 * retention, in server-rs/crates/droppen-agent/src/streams/retention.rs): numbers are the measured
 * p50s, nothing says "unhackable" or "audited", and "open source" only ever
 * names the client (AGPL-3.0), never Droppen Hub or Droppen as a whole.
 */

import { downloadJa, downloadZhCN, downloadZhTW } from "./download-copy";
import { pricingJa, pricingZhCN, pricingZhTW } from "./pricing-copy";

type Bundle = Record<string, string>;

const zhCN: Bundle = {
  // Diagram cards: every row says something about the product.
  "your computer": "你的电脑",
  "your phone": "你的手机",
  "browser": "浏览器",
  "offline since {{time}}": "{{time}} 起离线",
  "Droppen can't decrypt it": "Droppen 无法解密",
  "done, tests pass": "改好了，测试通过",
  "Droppen relay": "Droppen 中继",
  "relay": "中继",
  "encrypted copy": "加密副本",
  "read-only": "只读",
  "darken the sign-in button": "把登录按钮改成深色",
  "hands the copy to your phone": "把副本交给你的手机",
  "encrypted locally": "本地加密",
  "decrypted locally": "本地解密",

  // The hero's figure: two computers' sessions, and your phone (hero-agents.tsx). The words a session's state shows are the app's own.
  "Linux server": "Linux 服务器",
  "working": "工作中",
  "asking you": "在问你",
  "done": "完成",
  "waiting for your answer": "等你回答",
  "not now": "以后再说",
  "send": "发送",

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
  "Get started": "开始使用",

  // Hero
  "Your computer's AI, from your phone.": "电脑上的 AI，手机上接着用",
  "Claude Code and Codex keep running on your own computers. From a browser on your phone or another device, answer their questions, steer them and open a terminal on the computer.":
    "Claude Code 和 Codex 仍在你自己的电脑上运行。在手机或任意设备的浏览器中，随时查看进度、回复提问、发送指令，并直接调出本地终端。",
  "Create a free account": "免费注册",
  "Sign up on the web": "Web 免费注册",
  "Open web app": "在 Web 中打开",
  "Join the web waitlist": "加入 Web 等候名单",
  "Run Droppen on a Linux server": "Linux 服务器上运行 Droppen",
  "Linux and other platforms": "Linux 及其他平台",
  "Invite-only for now: sign-up needs an invite code.": "目前邀请制，注册需要邀请码。",
  "Sign-up is closed for now.": "目前暂停注册。",
  "What Droppen looks like": "Droppen 的产品界面",
  "Sessions": "会话",
  "Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer":
    "Mac 上的 Droppen：Claude Code 和 Codex 的会话列表，以及一个等你回答的会话",
  "Side by side": "分屏",
  "Droppen with three panes: two sessions and a terminal": "Mac 上的 Droppen 分成三栏：两个会话和一个终端",
  "Terminal": "终端",
  "A terminal in Droppen": "Droppen 里的终端",
  "My devices": "我的设备",
  "my.droppen.app in a browser: three computers and the list of what needs you": "浏览器里的 my.droppen.app：三台电脑，以及「需要你」清单",
  "Phone": "移动端",
  "Droppen on a phone: what needs you, and a session's question": "手机上的 Droppen：「需要你」清单，以及一个会话的提问",
  "Add a device": "添加设备",
  "A new device shows a QR code, and a device you already have approves it": "新设备显示二维码，由你已有的设备扫码批准",
  "Three steps to set up, then carry on away from your desk.": "三步完成设置，离开电脑也能继续",
  "Install Droppen where Claude Code or Codex runs": "在电脑端安装客户端",
  "Sign in on that computer": "登录并关联设备",

  // How it works: three steps, each true of the build (first-host.tsx, add-computer.tsx, pairing-screen.tsx)
  "Install it where your AI runs": "在运行 AI 的电脑上安装",
  "The Mac app, or one install command on Linux. Sessions already on that computer appear in Droppen, with nothing to migrate.":
    "Mac 上安装应用，Linux 上执行一条安装命令。电脑上已有的会话会直接出现在 Droppen 中，无需迁移。",
  "Sign in from the Mac app. On a Linux server, run the command with a one-time code from a device that is already signed in. Add your other computers the same way.":
    "在 Mac 应用中点击登录；在 Linux 服务器上，运行由已登录设备生成、附带一次性注册码的命令。其他电脑以同样方式加入同一账户。",
  "Continue on your phone": "手机扫码即刻使用",
  "Open {{app}} in your phone's browser, then scan the QR code it shows with a device that is already signed in.":
    "在手机浏览器中打开 {{app}}，再用一台已登录的设备扫描手机上显示的二维码，确认加入。",

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
  "Typing from your phone stays responsive.": "直连快速响应，断连自动接管，手机输入不丢键",
  "When your phone can reach your computer directly, keystrokes go straight there; when it can't, a Droppen relay forwards them automatically. If the direct path drops, the relay takes over within 1.5 s and resends unconfirmed keystrokes in order, so none are lost.":
    "手机能直接连到你的电脑时，按键经直连送达；连不上时，自动改由 Droppen 中继转发。直连中断后，中继在 1.5 秒内接管，并按原顺序重发尚未确认的按键，不会丢失。",
  "browser on the same computer": "同一台电脑上的浏览器",
  "phone on 4G": "手机 · 4G 网络",
  "measured on": "实测场景",
  "measured median round trip · direct {{d}} · via a Droppen relay {{h}}": "实测往返延迟中位数 · 直连 {{d}} · 经 Droppen 中继 {{h}}",
  "direct": "直连",
  "lost": "断开",
  "direct path up": "直连中",
  "direct path lost: switched to the Droppen relay, resending unconfirmed keys in order": "直连中断：已切换到 Droppen 中继，按原顺序重发未确认的按键",
  "network back: direct again": "网络恢复：重新直连",
  "keys {{sent}} · delivered {{delivered}} · in flight {{flight}} · lost 0": "按键 {{sent}} · 送达 {{delivered}} · 在途 {{flight}} · 丢失 0",

  // Lid closed? (the offline copy)
  "Your sessions stay readable while your computer is offline.": "电脑合盖或离线，会话依然随时查阅",
  "While your computer is online, an encrypted copy of its sessions is kept on Droppen Hub in the cloud, and only your devices can decrypt it. When the computer is shut down, asleep or offline, your phone can still open the sessions updated in the last {{free}} days ({{pro}} on Pro); sending messages and using the terminal wait until that computer is back online.":
    "电脑在线时，会话的加密副本会保存到云端的 Droppen Hub，只有你的设备能解密。电脑关机、休眠或断网后，手机仍可查看最近 {{free}} 天内更新的会话（Pro 方案为 {{pro}} 天）；发送消息和使用终端，需等这台电脑重新上线。",

  // What you get
  "Away from your computer, handle all of this from your phone.": "不在电脑前，这些事也能在手机上处理",
  "Every computer, one list": "所有电脑，一个列表",
  "Home and work computers on one account, with every session that is waiting for an answer, has unread messages or is running in one list.":
    "多台电脑绑定同一账户。所有正在运行、等待回复或有未读更新的会话，汇总在同一个列表中一览无余。",
  "Answer and steer": "回答提问，中途干预",
  "Answer the AI's questions, add a message while it works, or stop the current run if it goes the wrong way.":
    "回答 AI 的提问；在它生成中随时追加补充指令，若发现偏离预期可立即中止。",
  "Notified when a session needs you": "需要你时，收到通知",
  "A notification when the AI asks a question, finishes a reply you haven't read, or fails.": "AI 向你提问、回复完成但你尚未查看，或运行失败时，都会通知你。",
  "Start a session from your phone": "在手机上新建会话",
  "Choose Claude Code or Codex, a computer and a folder, optionally attach a screenshot, and send.": "选择 Claude Code 或 Codex、电脑和文件夹，可附上截图，然后发送。",
  "Browse your files": "浏览项目文件",
  "Browse the folders on your computer and preview files from your phone.": "在手机上浏览电脑上的文件夹，并预览文件内容。",
  "Run prompts on a schedule": "按计划定时执行指令",
  "Have a prompt run automatically on the computer you choose, for example every morning or every hour on weekdays.":
    "让发给 AI 的 Prompt 按预设计划在指定的电脑上自动运行，例如每个工作日早晨定时触发。",

  // The beta's caveat, under that section
  "Droppen is in beta: its encryption design has had internal security reviews, not an external audit. In a browser, the page code loaded each time can use your private keys and, if tampered with, could copy them out; for higher assurance, use the Mac app.":
    "Droppen 目前是测试版：加密设计经过内部安全评审，尚未进行外部审计。在浏览器中，每次打开页面时加载的代码都能使用你的私钥，若被篡改，就可能把私钥复制出去；需要更高保障时，请使用 Mac 应用。",
  // The same caveat, whole, on /security.
  "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen 目前是测试版。加密设计经过了内部安全评审，还没有做过外部审计。在浏览器里使用时，你需要信任当次加载的页面代码：这份代码能使用你的私钥，也能把它们复制出去。",

  // Footer
  "Your AI keeps working on your computer. When it needs you, answer from your phone.": "AI 在你的电脑上继续工作，需要你时，在手机上即可回应",
};

const zhTW: Bundle = {
  // Diagram cards: every row says something about the product.
  "your computer": "你的電腦",
  "your phone": "你的手機",
  "browser": "瀏覽器",
  "offline since {{time}}": "{{time}} 起離線",
  "Droppen can't decrypt it": "Droppen 無法解密",
  "done, tests pass": "改好了，測試通過",
  "Droppen relay": "Droppen 中繼",
  "relay": "中繼",
  "encrypted copy": "加密副本",
  "read-only": "唯讀",
  "darken the sign-in button": "把登入按鈕改成深色",
  "hands the copy to your phone": "把副本交給你的手機",
  "encrypted locally": "本機加密",
  "decrypted locally": "本機解密",

  "Linux server": "Linux 伺服器",
  "working": "工作中",
  "asking you": "在問你",
  "done": "完成",
  "waiting for your answer": "等你回答",
  "not now": "以後再說",
  "send": "傳送",

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
  "Get started": "開始使用",

  "Your computer's AI, from your phone.": "電腦上的 AI，手機上接著用",
  "Claude Code and Codex keep running on your own computers. From a browser on your phone or another device, answer their questions, steer them and open a terminal on the computer.":
    "Claude Code 和 Codex 仍在你自己的電腦上執行。在手機或其他裝置的瀏覽器中，回答它們的提問、傳送指令、開啟電腦上的終端機。",
  "Create a free account": "免費註冊",
  "Sign up on the web": "Web 免費註冊",
  "Open web app": "在 Web 中打開",
  "Join the web waitlist": "加入 Web 等候名單",
  "Run Droppen on a Linux server": "在 Linux 伺服器上執行 Droppen",
  "Linux and other platforms": "Linux 及其他平台",
  "Invite-only for now: sign-up needs an invite code.": "目前採邀請制，註冊需要邀請碼。",
  "Sign-up is closed for now.": "目前暫停註冊。",
  "What Droppen looks like": "Droppen 的產品介面",
  "Sessions": "工作階段",
  "Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer":
    "Mac 上的 Droppen：Claude Code 和 Codex 的工作階段清單，以及一個等你回答的工作階段",
  "Side by side": "多欄",
  "Droppen with three panes: two sessions and a terminal": "Mac 上的 Droppen 分成三欄：兩個工作階段和一個終端機",
  "Terminal": "終端機",
  "A terminal in Droppen": "Droppen 裡的終端機",
  "My devices": "我的裝置",
  "my.droppen.app in a browser: three computers and the list of what needs you": "瀏覽器裡的 my.droppen.app：三台電腦，以及「需要你」清單",
  "Phone": "手機",
  "Droppen on a phone: what needs you, and a session's question": "手機上的 Droppen：「需要你」清單，以及一個工作階段的提問",
  "Add a device": "新增裝置",
  "A new device shows a QR code, and a device you already have approves it": "新裝置顯示 QR 碼，由你已有的裝置掃碼核准",
  "Three steps to set up, then carry on away from your desk.": "三步完成設定，離開電腦也能繼續",
  "Install Droppen where Claude Code or Codex runs": "在執行 Claude Code 或 Codex 的電腦上安裝 Droppen",
  "Sign in on that computer": "在那台電腦上登入",

  "Install it where your AI runs": "裝在跑 AI 的電腦上",
  "The Mac app, or one install command on Linux. Sessions already on that computer appear in Droppen, with nothing to migrate.":
    "Mac 上安裝應用程式，Linux 上執行一行安裝指令。電腦上既有的工作階段會直接出現在 Droppen 中，無需遷移。",
  "Sign in from the Mac app. On a Linux server, run the command with a one-time code from a device that is already signed in. Add your other computers the same way.":
    "在 Mac 應用程式中點擊登入；在 Linux 伺服器上，執行由已登入裝置產生、附一次性代碼的指令。其他電腦以同樣方式加入同一帳戶。",
  "Continue on your phone": "在手機上接著用",
  "Open {{app}} in your phone's browser, then scan the QR code it shows with a device that is already signed in.":
    "在手機瀏覽器中打開 {{app}}，再用一台已登入的裝置掃描手機上顯示的 QR 碼，確認加入。",

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

  "Typing from your phone stays responsive.": "在手機上輸入，回應及時",
  "When your phone can reach your computer directly, keystrokes go straight there; when it can't, a Droppen relay forwards them automatically. If the direct path drops, the relay takes over within 1.5 s and resends unconfirmed keystrokes in order, so none are lost.":
    "手機能直接連到你的電腦時，按鍵經直連送達；連不上時，自動改由 Droppen 中繼轉送。直連中斷後，中繼在 1.5 秒內接手，並按原順序重送尚未確認的按鍵，不會遺失。",
  "browser on the same computer": "同一台電腦上的瀏覽器",
  "phone on 4G": "手機 · 4G 網路",
  "measured on": "實測情境",
  "measured median round trip · direct {{d}} · via a Droppen relay {{h}}": "實測往返延遲中位數 · 直連 {{d}} · 經 Droppen 中繼 {{h}}",
  "direct": "直連",
  "lost": "斷開",
  "direct path up": "直連中",
  "direct path lost: switched to the Droppen relay, resending unconfirmed keys in order": "直連中斷：已切換到 Droppen 中繼，按原順序重送未確認的按鍵",
  "network back: direct again": "網路恢復：重新直連",
  "keys {{sent}} · delivered {{delivered}} · in flight {{flight}} · lost 0": "按鍵 {{sent}} · 送達 {{delivered}} · 傳送中 {{flight}} · 遺失 0",

  "Your sessions stay readable while your computer is offline.": "電腦離線時，工作階段仍可查看",
  "While your computer is online, an encrypted copy of its sessions is kept on Droppen Hub in the cloud, and only your devices can decrypt it. When the computer is shut down, asleep or offline, your phone can still open the sessions updated in the last {{free}} days ({{pro}} on Pro); sending messages and using the terminal wait until that computer is back online.":
    "電腦在線時，工作階段的加密副本會儲存到雲端的 Droppen Hub，只有你的裝置能解密。電腦關機、休眠或斷網後，手機仍可查看最近 {{free}} 天內更新的工作階段（Pro 方案為 {{pro}} 天）；傳送訊息和使用終端機，需等這台電腦重新上線。",

  "Away from your computer, handle all of this from your phone.": "不在電腦前，這些事也能在手機上處理",
  "Every computer, one list": "所有電腦，一個清單",
  "Home and work computers on one account, with every session that is waiting for an answer, has unread messages or is running in one list.":
    "家裡和公司的電腦登入同一個帳戶，等你回答、有未讀訊息和正在執行的工作階段匯整在同一個清單中。",
  "Answer and steer": "回答提問，中途介入",
  "Answer the AI's questions, add a message while it works, or stop the current run if it goes the wrong way.": "回答 AI 的提問，在它執行時追加訊息，方向不對時可以停止目前的執行。",
  "Notified when a session needs you": "需要你時，收到通知",
  "A notification when the AI asks a question, finishes a reply you haven't read, or fails.": "AI 向你提問、回覆完成但你尚未查看，或執行失敗時，都會通知你。",
  "Start a session from your phone": "在手機上新增工作階段",
  "Choose Claude Code or Codex, a computer and a folder, optionally attach a screenshot, and send.": "選擇 Claude Code 或 Codex、電腦和資料夾，可附上截圖，然後送出。",
  "Browse your files": "瀏覽專案檔案",
  "Browse the folders on your computer and preview files from your phone.": "在手機上瀏覽電腦上的資料夾，並預覽檔案內容。",
  "Run prompts on a schedule": "排程任務",
  "Have a prompt run automatically on the computer you choose, for example every morning or every hour on weekdays.":
    "讓一個給 AI 的指令按排程在指定的電腦上自動執行，例如每天早上或工作日每小時。",

  "Droppen is in beta: its encryption design has had internal security reviews, not an external audit. In a browser, the page code loaded each time can use your private keys and, if tampered with, could copy them out; for higher assurance, use the Mac app.":
    "Droppen 目前是測試版：加密設計經過內部安全審查，尚未進行外部稽核。在瀏覽器中，每次開啟頁面時載入的程式碼都能使用你的私密金鑰，若遭竄改，就可能把私密金鑰複製出去；需要更高保障時，請使用 Mac 應用程式。",
  // The same caveat, whole, on /security.
  "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen 目前是測試版。加密設計經過內部安全審查，還沒有做過外部稽核。在瀏覽器裡使用時，你需要信任當次載入的頁面程式碼：這份程式碼能使用你的私密金鑰，也能把它們複製出去。",

  "Your AI keeps working on your computer. When it needs you, answer from your phone.": "AI 在你的電腦上繼續工作，需要你時，在手機上即可回應",
};

const ja: Bundle = {
  // Diagram cards: every row says something about the product.
  "your computer": "あなたのコンピュータ",
  "your phone": "あなたのスマホ",
  "browser": "ブラウザ",
  "offline since {{time}}": "{{time}} からオフライン",
  "Droppen can't decrypt it": "Droppen は復号できない",
  "done, tests pass": "完了、テスト通過",
  "Droppen relay": "Droppen 中継",
  "relay": "中継",
  "encrypted copy": "暗号化コピー",
  "read-only": "読み取り専用",
  "darken the sign-in button": "ログインボタンを濃い色にして",
  "hands the copy to your phone": "コピーをスマホへ渡す",
  "encrypted locally": "ローカルで暗号化",
  "decrypted locally": "ローカルで復号",

  "Linux server": "Linux サーバー",
  "working": "作業中",
  "asking you": "質問中",
  "done": "完了",
  "waiting for your answer": "回答待ち",
  "not now": "あとで",
  "send": "送信",

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
  "Get started": "はじめる",

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
  "What Droppen looks like": "Droppen の画面",
  "Sessions": "セッション",
  "Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer":
    "Mac の Droppen：Claude Code と Codex のセッション一覧と、回答を待つセッション",
  "Side by side": "並べて表示",
  "Droppen with three panes: two sessions and a terminal": "Mac の Droppen を 3 ペインに：2 つのセッションとターミナル",
  "Terminal": "ターミナル",
  "A terminal in Droppen": "Droppen のターミナル",
  "My devices": "マイデバイス",
  "my.droppen.app in a browser: three computers and the list of what needs you": "ブラウザの my.droppen.app：3 台のコンピュータと「対応待ち」リスト",
  "Phone": "スマホ",
  "Droppen on a phone: what needs you, and a session's question": "スマホの Droppen：「対応待ち」リストとセッションの質問",
  "Add a device": "デバイスを追加",
  "A new device shows a QR code, and a device you already have approves it": "新しいデバイスが QR コードを表示し、手持ちのデバイスが承認する",
  "Three steps to set up, then carry on away from your desk.": "3 ステップで設定完了。席を離れても続けられます。",
  "Install Droppen where Claude Code or Codex runs": "Claude Code か Codex が動くコンピュータに Droppen をインストール",
  "Sign in on that computer": "そのコンピュータでログイン",

  "Install it where your AI runs": "AI が動くコンピュータに入れる",
  "The Mac app, or one install command on Linux. Sessions already on that computer appear in Droppen, with nothing to migrate.":
    "Mac はアプリ、Linux はインストールコマンドを 1 行。そのコンピュータにあるセッションがそのまま Droppen に表示され、移行は不要です。",
  "Sign in from the Mac app. On a Linux server, run the command with a one-time code from a device that is already signed in. Add your other computers the same way.":
    "Mac アプリならログインをクリック。Linux サーバーでは、ログイン済みのデバイスで発行したワンタイムコード付きのコマンドを実行します。ほかのコンピュータも同じ手順で同じアカウントに追加できます。",
  "Continue on your phone": "スマホで続きから",
  "Open {{app}} in your phone's browser, then scan the QR code it shows with a device that is already signed in.":
    "スマホのブラウザで {{app}} を開き、表示された QR コードをログイン済みのデバイスで読み取ります。",

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

  "Typing from your phone stays responsive.": "スマホからの入力も、すぐに反映。",
  "When your phone can reach your computer directly, keystrokes go straight there; when it can't, a Droppen relay forwards them automatically. If the direct path drops, the relay takes over within 1.5 s and resends unconfirmed keystrokes in order, so none are lost.":
    "スマホからコンピュータに直結できればキー入力は直接届き、できなければ Droppen 中継が自動で転送します。直結が切れても 1.5 秒以内に中継が引き継ぎ、未確認のキー入力を順番どおりに再送するので、失われません。",
  "browser on the same computer": "同じコンピュータのブラウザ",
  "phone on 4G": "スマホ · 4G",
  "measured on": "計測した環境",
  "measured median round trip · direct {{d}} · via a Droppen relay {{h}}": "実測の往復遅延（中央値）· 直結 {{d}} · Droppen 中継経由 {{h}}",
  "direct": "直結",
  "lost": "切断",
  "direct path up": "直結中",
  "direct path lost: switched to the Droppen relay, resending unconfirmed keys in order": "直結が切断：Droppen 中継に切り替え、未確認のキーを順番どおりに再送",
  "network back: direct again": "ネットワーク復帰：再び直結",
  "keys {{sent}} · delivered {{delivered}} · in flight {{flight}} · lost 0": "キー {{sent}} · 到着 {{delivered}} · 送信中 {{flight}} · 消失 0",

  "Your sessions stay readable while your computer is offline.": "コンピュータがオフラインでも、セッションは読めます。",
  "While your computer is online, an encrypted copy of its sessions is kept on Droppen Hub in the cloud, and only your devices can decrypt it. When the computer is shut down, asleep or offline, your phone can still open the sessions updated in the last {{free}} days ({{pro}} on Pro); sending messages and using the terminal wait until that computer is back online.":
    "コンピュータがオンラインの間、セッションの暗号化コピーがクラウドの Droppen Hub に保存され、復号できるのはあなたのデバイスだけです。電源オフ、スリープ、オフラインのときも、スマホで直近 {{free}} 日間（Pro は {{pro}} 日間）に更新されたセッションを開けます。送信とターミナルは、そのコンピュータがオンラインに戻るまで待ちます。",

  "Away from your computer, handle all of this from your phone.": "コンピュータの前にいなくても、これらはスマホで対応できます。",
  "Every computer, one list": "すべてのコンピュータを、ひとつのリストで",
  "Home and work computers on one account, with every session that is waiting for an answer, has unread messages or is running in one list.":
    "自宅と職場のコンピュータをひとつのアカウントに。回答待ち、未読あり、実行中のセッションがひとつのリストにまとまります。",
  "Answer and steer": "回答と軌道修正",
  "Answer the AI's questions, add a message while it works, or stop the current run if it goes the wrong way.": "AI の質問に答える、作業中にメッセージを追加する、方向が違えば実行を止める。",
  "Notified when a session needs you": "呼ばれたら、通知が届く",
  "A notification when the AI asks a question, finishes a reply you haven't read, or fails.": "AI が質問したとき、まだ読んでいない返信が終わったとき、失敗したときに通知します。",
  "Start a session from your phone": "スマホからセッションを開始",
  "Choose Claude Code or Codex, a computer and a folder, optionally attach a screenshot, and send.":
    "Claude Code か Codex、コンピュータとフォルダを選び、必要ならスクリーンショットを添えて送信。",
  "Browse your files": "ファイルを閲覧",
  "Browse the folders on your computer and preview files from your phone.": "スマホからコンピュータのフォルダを開き、ファイルの中身をプレビューできます。",
  "Run prompts on a schedule": "スケジュール実行",
  "Have a prompt run automatically on the computer you choose, for example every morning or every hour on weekdays.":
    "AI への指示を、選んだコンピュータで決まった時間に自動実行。たとえば毎朝、平日は毎時など。",

  "Droppen is in beta: its encryption design has had internal security reviews, not an external audit. In a browser, the page code loaded each time can use your private keys and, if tampered with, could copy them out; for higher assurance, use the Mac app.":
    "Droppen はベータ版です。暗号設計は社内のセキュリティレビューを受けていますが、外部監査はまだです。ブラウザでは、ページを開くたびに読み込まれるコードが秘密鍵を使えるため、改ざんされれば秘密鍵を持ち出される可能性があります。より高い保証が必要な場合は Mac アプリをお使いください。",
  // The same caveat, whole, on /security.
  "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen はベータ版です。暗号設計は社内のセキュリティレビューを受けていますが、外部監査はまだです。ブラウザで使う場合は、そのとき読み込んだページのコードを信頼することになります。そのコードは秘密鍵を使えるうえ、持ち出すこともできます。",

  "Your AI keeps working on your computer. When it needs you, answer from your phone.": "AI はコンピュータで作業を続け、呼ばれたらスマホで応えられます。",
};

/** The landing page's tables, by i18next language. */
export const landingBundles: Record<"zh-CN" | "zh-TW" | "ja", Bundle> = {
  "zh-CN": { ...zhCN, ...downloadZhCN, ...pricingZhCN },
  "zh-TW": { ...zhTW, ...downloadZhTW, ...pricingZhTW },
  ja: { ...ja, ...downloadJa, ...pricingJa },
};
