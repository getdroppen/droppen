/*
 * The landing page's words. As everywhere in the app (`src/i18n`), the keys
 * are the English text, so English needs no table. zh-CN is the page's first
 * language, written as native copy rather than translated from the English;
 * zh-TW follows the app's Taiwan wording (工作階段, 轉送, 資料, 裝置) and ja
 * its Japanese (直結, 中継経由, デバイス, 対応待ち).
 *
 * The voice is a product page's, not a manual's: every headline says what the
 * reader gets (their computer's AI on their phone, every session in one list,
 * the agent's question answered from anywhere). The layout does the
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
 * architecture.md §0, §2, §5, §7, §9; the account
 * rotation in server-rs/src/accounts/rotation.rs; the devices dashboard and
 * its 「需要你」 list in features/cloud/my-devices.tsx; the notifications in
 * server-rs/src/push; a new session in features/sessions/new-session-view.tsx;
 * scheduled prompts in server-rs/src/schedule): numbers are the measured
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
  "computer off · {{time}}": "电脑已关机 · {{time}}",
  "Droppen can't read it": "Droppen 读不了",
  "done, tests pass": "改好了，测试通过",
  "Droppen relay": "Droppen 中继",
  "relay": "中继",
  "encrypted copy": "加密副本",
  "read-only": "只读",
  "darken the sign-in button": "把登录按钮改成深色",
  "passes the copy on": "把副本转给你",
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
  "Claude Code and Codex keep running on your own Macs and Linux machines; every session on all of them lands in one list on your phone or in any browser, to answer, steer or open a terminal. End-to-end encrypted, with an open-source client.":
    "Claude Code 和 Codex 照常在你自己的 Mac 和 Linux 上跑，所有电脑上的会话汇成一张清单，在手机或任何浏览器里回答、指挥、开终端。端到端加密，客户端开源。",
  "On this page": "本页",
  "Product": "产品",
  "Features": "功能",
  "Security": "安全",
  "Source code": "源代码",
  "Third-party licences": "第三方许可证",
  "How it works": "工作原理",
  "Open app": "打开应用",
  "Sign in": "登录",
  "Get started": "开始使用",

  // Hero
  "Your computer's AI, in your pocket.": "电脑上的 AI，手机上接着用。",
  "Claude Code and Codex keep running on your own computers. Answer them, steer them and open a terminal from your phone or any browser.":
    "Claude Code 和 Codex 照常在你自己的电脑上跑。在手机或任何浏览器里，回答它、指挥它、开终端。",
  "Start for free": "免费开始",
  "Public beta: free for {{hosts}} computers.": "公开测试版，{{hosts}} 台电脑以内免费。",
  "Invite-only for now: sign-up needs an invite code.": "目前邀请制，注册需要邀请码。",
  "Sign-up is closed for now.": "目前暂停注册。",
  "See the plans": "查看各个方案",
  "What Droppen looks like": "Droppen 长什么样",
  "Sessions": "会话",
  "Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer":
    "Mac 上的 Droppen：Claude Code 和 Codex 的会话列表，以及一个等你回答的会话",
  "Split screen": "分屏",
  "Droppen with three panes: two sessions and a terminal": "Droppen 的三栏分屏：两个会话和一个终端",
  "Terminal": "终端",
  "A terminal in Droppen": "Droppen 里的终端",
  "Cloud": "云端",
  "my.droppen.app: three computers and the list of what needs you": "my.droppen.app：三台电脑，以及「需要你」清单",
  "Phone": "手机",
  "Droppen on a phone: what needs you, and a session's question": "手机上的 Droppen：「需要你」清单，以及一个会话的提问",
  "Letting a new device in with a QR code": "扫码让新设备加入",
  "Three steps, then it goes where you go.": "三步装好，走到哪都能接手。",
  "Put Droppen where your AI runs": "把 Droppen 装在跑 AI 的电脑上",
  "Sign in on that computer": "在这台电脑上登录",

  // How it works: three steps, each true of the build (first-host.tsx, add-computer.tsx, pairing-screen.tsx)
  "Install it where your AI runs": "装在跑 AI 的电脑上",
  "The Mac app, or one command on Linux. Your sessions show up as they are.": "Mac 上装应用，Linux 上跑一条命令。已有的会话原样出现。",
  "One click in the Mac app, or a one-time code on a server. Add your other computers to the same account.": "Mac 应用里点一下，服务器上用一次性口令。其他电脑也加进同一个账户。",
  "Pick up on your phone": "在手机上接着用",
  "Open droppen.app on your phone, and let it in with a QR code from a device you already have.": "在手机上打开 droppen.app，用已有的设备扫码放行。",

  // Your code, for your eyes only (the one section on encryption, near the end)
  "End-to-end encrypted": "端到端加密",
  "Your code, for your eyes only.": "你的代码，只有你看得到。",
  "Encrypted on your computer, opened only on your devices. Droppen Hub only keeps encrypted copies and Droppen relays only pass ciphertext on: neither holds a key that can read them.":
    "在你的电脑上加密，只在你的设备上解开。Droppen Hub 只保管加密副本，Droppen 中继只转发密文，两者都没有能读懂内容的钥匙。",
  "Only devices you approve get in": "只有你批准的设备进得来",
  "A new phone or browser gets in only when a device you already have lets it in. Remove a lost one, and it can't open anything written afterwards.":
    "新手机、新浏览器，要你已有的设备放行才能加入。丢了就移除，之后写下的内容它一个字也解不开。",
  "An open-source client": "客户端开源",
  "The program on your computers, the Mac app and the web app are open source (AGPL-3.0). Droppen Hub is not open source.":
    "你电脑上的程序、Mac 应用和网页应用都以 AGPL-3.0 开源。Droppen Hub 不开源。",
  "Read the source": "查看源代码",

  // Typing from your phone stays snappy (the direct path)
  "Typing from your phone stays snappy.": "在手机上打字，照样跟手。",
  "Keystrokes go straight to your computer when your phone can reach it, and through a Droppen relay when it can't. If the direct path drops, the relay takes over within 1.5 s and no keystroke is lost.":
    "手机能直连电脑时，按键直接过去；连不通时走 Droppen 中继。直连一旦断开，中继在 1.5 秒内接手，一个按键都不丢。",
  "same computer": "同一台电脑",
  "on 4G": "4G 网络",
  "measured on": "实测场景",
  "measured p50 round trip · direct {{d}} · via a Droppen relay {{h}}": "实测往返 p50 · 直连 {{d}} · 经 Droppen 中继 {{h}}",
  "direct": "直连",
  "lost": "断开",
  "direct path up": "直连中",
  "direct path lost: back on the Droppen relay, unconfirmed keys sent again in order": "直连断开：退回 Droppen 中继，未确认的按键按顺序重发",
  "network back: direct again": "网络恢复：重新直连",
  "keys {{sent}} · delivered {{delivered}} · lost 0": "按键 {{sent}} · 送达 {{delivered}} · 丢失 0",
  "if the direct path drops, the Droppen relay takes over within 1.5 s and no keystroke is lost": "直连一旦断开，Droppen 中继在 1.5 秒内接手，一个按键都不丢",

  // Lid closed? (the offline copy)
  "Lid closed? Your sessions are still there.": "电脑合上了，会话照样能看。",
  "While your computer is on, it leaves an encrypted copy of your sessions with Droppen Hub. Close the lid or lose the network, and your phone still opens any session from the last 30 days; sending and terminals wait for the computer to come back.":
    "电脑开着时，会把会话加密存一份到 Droppen Hub。合上电脑、断了网，手机照样能打开最近 30 天的会话；发消息和用终端，要等电脑回来。",

  // What you get
  "Everything you would walk back to your desk for.": "本来要走回工位才能做的事，手机上都能做。",
  "Every session, one list": "所有会话，一个列表",
  "Home and work computers on one account, and one list of what needs you across all of them.": "家里和公司的电脑在同一个账户下，所有「需要你」的会话汇成一张清单。",
  "Answer it, steer it": "回答它，也能指挥它",
  "Answer its questions, add a follow-up while it works, or stop a turn going the wrong way.": "回答它的提问，在它忙时追加消息，或者停下跑偏的这一轮。",
  "Told when it needs you": "需要你时，第一时间知道",
  "A notification when an agent asks you something, finishes a turn you haven't read, or fails.": "agent 提问、跑完一轮而你还没看、或者运行失败时，都会通知你。",
  "Start one from your phone": "在手机上开新会话",
  "Pick the agent, the folder and the computer, attach a screenshot, and send.": "选好 agent、目录和电脑，附上截图，发出去。",
  "Hit a limit? Carry on": "额度用完？接着跑",
  "Add a few Claude accounts: when one hits its limit, a session can switch to another by itself.": "添加几个 Claude 账号，一个额度用完，会话能自动换到另一个。",
  "Prompts on a schedule": "定时任务",
  "Run a prompt every morning, or every hour on weekdays, on the computer you choose.": "让一条指令每天早上、或工作日每小时，在你选的电脑上自己跑。",

  // The beta's caveat, under that section
  "Droppen is in beta. Its encryption has had internal security reviews, not an external audit. In a browser, the page code as loaded can use your keys, and could copy them out.":
    "Droppen 目前是测试版。加密设计做过内部安全评审，还没有外部审计。在浏览器里，当次加载的页面代码能用你的私钥，也能把它们复制出去。",
  // The same caveat, whole, on /security.
  "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen 目前是测试版。加密设计经过了内部安全评审，还没有做过外部审计。在浏览器里使用时，你需要信任当次加载的页面代码：这份代码能使用你的私钥，也能把它们复制出去。",

  // Footer
  "Your AI keeps working, wherever you are.": "AI 接着干活，你在哪都能接手。",
};

const zhTW: Bundle = {
  // Diagram cards: every row says something about the product.
  "your computer": "你的電腦",
  "your phone": "你的手機",
  "browser": "瀏覽器",
  "computer off · {{time}}": "電腦已關機 · {{time}}",
  "Droppen can't read it": "Droppen 讀不了",
  "done, tests pass": "改好了，測試通過",
  "Droppen relay": "Droppen 中繼",
  "relay": "中繼",
  "encrypted copy": "加密副本",
  "read-only": "唯讀",
  "darken the sign-in button": "把登入按鈕改成深色",
  "passes the copy on": "把副本轉給你",
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
  "Claude Code and Codex keep running on your own Macs and Linux machines; every session on all of them lands in one list on your phone or in any browser, to answer, steer or open a terminal. End-to-end encrypted, with an open-source client.":
    "Claude Code 和 Codex 照常在你自己的 Mac 和 Linux 上跑，所有電腦上的工作階段匯成一張清單，在手機或任何瀏覽器裡回答、指揮、開終端機。端對端加密，客戶端開源。",
  "On this page": "本頁",
  "Product": "產品",
  "Features": "功能",
  "Security": "安全",
  "Source code": "原始碼",
  "Third-party licences": "第三方授權",
  "How it works": "運作方式",
  "Open app": "打開應用",
  "Sign in": "登入",
  "Get started": "開始使用",

  "Your computer's AI, in your pocket.": "電腦上的 AI，手機上接著用。",
  "Claude Code and Codex keep running on your own computers. Answer them, steer them and open a terminal from your phone or any browser.":
    "Claude Code 和 Codex 照常在你自己的電腦上跑。在手機或任何瀏覽器裡，回答它、指揮它、開終端機。",
  "Start for free": "免費開始",
  "Public beta: free for {{hosts}} computers.": "公開測試版，{{hosts}} 台電腦以內免費。",
  "Invite-only for now: sign-up needs an invite code.": "目前採邀請制，註冊需要邀請碼。",
  "Sign-up is closed for now.": "目前暫停註冊。",
  "See the plans": "查看各個方案",
  "What Droppen looks like": "Droppen 長什麼樣",
  "Sessions": "工作階段",
  "Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer":
    "Mac 上的 Droppen：Claude Code 和 Codex 的工作階段清單，以及一個等你回答的工作階段",
  "Split screen": "分割畫面",
  "Droppen with three panes: two sessions and a terminal": "Droppen 的三欄分割畫面：兩個工作階段和一個終端機",
  "Terminal": "終端機",
  "A terminal in Droppen": "Droppen 裡的終端機",
  "Cloud": "雲端",
  "my.droppen.app: three computers and the list of what needs you": "my.droppen.app：三台電腦，以及「需要你」清單",
  "Phone": "手機",
  "Droppen on a phone: what needs you, and a session's question": "手機上的 Droppen：「需要你」清單，以及一個工作階段的提問",
  "Letting a new device in with a QR code": "掃碼讓新裝置加入",
  "Three steps, then it goes where you go.": "三步裝好，走到哪都能接手。",
  "Put Droppen where your AI runs": "把 Droppen 裝在跑 AI 的電腦上",
  "Sign in on that computer": "在這台電腦上登入",

  "Install it where your AI runs": "裝在跑 AI 的電腦上",
  "The Mac app, or one command on Linux. Your sessions show up as they are.": "Mac 上裝應用程式，Linux 上執行一行指令。既有的工作階段原樣出現。",
  "One click in the Mac app, or a one-time code on a server. Add your other computers to the same account.": "Mac 應用程式裡點一下，伺服器上用一次性代碼。其他電腦也加進同一個帳戶。",
  "Pick up on your phone": "在手機上接著用",
  "Open droppen.app on your phone, and let it in with a QR code from a device you already have.": "在手機上打開 droppen.app，用已有的裝置掃碼放行。",

  "End-to-end encrypted": "端對端加密",
  "Your code, for your eyes only.": "你的程式碼，只有你看得到。",
  "Encrypted on your computer, opened only on your devices. Droppen Hub only keeps encrypted copies and Droppen relays only pass ciphertext on: neither holds a key that can read them.":
    "在你的電腦上加密，只在你的裝置上解開。Droppen Hub 只保管加密副本，Droppen 中繼只轉送密文，兩者都沒有能讀懂內容的金鑰。",
  "Only devices you approve get in": "只有你核准的裝置進得來",
  "A new phone or browser gets in only when a device you already have lets it in. Remove a lost one, and it can't open anything written afterwards.":
    "新手機、新瀏覽器，要你已有的裝置放行才能加入。掉了就移除，之後寫下的內容它一個字也解不開。",
  "An open-source client": "客戶端開源",
  "The program on your computers, the Mac app and the web app are open source (AGPL-3.0). Droppen Hub is not open source.":
    "你電腦上的程式、Mac 應用程式和網頁應用程式都以 AGPL-3.0 開源。Droppen Hub 不開源。",
  "Read the source": "查看原始碼",

  "Typing from your phone stays snappy.": "在手機上打字，照樣跟手。",
  "Keystrokes go straight to your computer when your phone can reach it, and through a Droppen relay when it can't. If the direct path drops, the relay takes over within 1.5 s and no keystroke is lost.":
    "手機能直連電腦時，按鍵直接過去；連不通時走 Droppen 中繼。直連一旦斷開，中繼在 1.5 秒內接手，一個按鍵都不丟。",
  "same computer": "同一台電腦",
  "on 4G": "4G 網路",
  "measured on": "實測情境",
  "measured p50 round trip · direct {{d}} · via a Droppen relay {{h}}": "實測往返 p50 · 直連 {{d}} · 經 Droppen 中繼 {{h}}",
  "direct": "直連",
  "lost": "斷開",
  "direct path up": "直連中",
  "direct path lost: back on the Droppen relay, unconfirmed keys sent again in order": "直連斷開：退回 Droppen 中繼，未確認的按鍵依序重送",
  "network back: direct again": "網路恢復：重新直連",
  "keys {{sent}} · delivered {{delivered}} · lost 0": "按鍵 {{sent}} · 送達 {{delivered}} · 遺失 0",
  "if the direct path drops, the Droppen relay takes over within 1.5 s and no keystroke is lost": "直連一旦斷開，Droppen 中繼在 1.5 秒內接手，一個按鍵都不丟",

  "Lid closed? Your sessions are still there.": "電腦闔上了，工作階段照樣能看。",
  "While your computer is on, it leaves an encrypted copy of your sessions with Droppen Hub. Close the lid or lose the network, and your phone still opens any session from the last 30 days; sending and terminals wait for the computer to come back.":
    "電腦開著時，會把工作階段加密存一份到 Droppen Hub。闔上電腦、斷了網，手機照樣能打開最近 30 天的工作階段；傳訊息和用終端機，要等電腦回來。",

  "Everything you would walk back to your desk for.": "原本要走回座位才能做的事，手機上都能做。",
  "Every session, one list": "所有工作階段，一個清單",
  "Home and work computers on one account, and one list of what needs you across all of them.": "家裡和公司的電腦在同一個帳戶下，所有「需要你」的工作階段匯成一張清單。",
  "Answer it, steer it": "回答它，也能指揮它",
  "Answer its questions, add a follow-up while it works, or stop a turn going the wrong way.": "回答它的提問，在它忙時追加訊息，或把跑偏的這一輪停下來。",
  "Told when it needs you": "需要你時，第一時間知道",
  "A notification when an agent asks you something, finishes a turn you haven't read, or fails.": "agent 提問、一輪跑完而你還沒看、或執行失敗時，都會通知你。",
  "Start one from your phone": "在手機上開新的工作階段",
  "Pick the agent, the folder and the computer, attach a screenshot, and send.": "選好 agent、資料夾和電腦，附上截圖，送出。",
  "Hit a limit? Carry on": "額度用完？接著跑",
  "Add a few Claude accounts: when one hits its limit, a session can switch to another by itself.": "加入幾個 Claude 帳號，一個額度用完，工作階段能自動換到另一個。",
  "Prompts on a schedule": "排程任務",
  "Run a prompt every morning, or every hour on weekdays, on the computer you choose.": "讓一個指令每天早上、或工作日每小時，在你選的電腦上自己跑。",

  "Droppen is in beta. Its encryption has had internal security reviews, not an external audit. In a browser, the page code as loaded can use your keys, and could copy them out.":
    "Droppen 目前是測試版。加密設計做過內部安全審查，還沒有外部稽核。在瀏覽器裡，當次載入的頁面程式碼能用你的私密金鑰，也能把它們複製出去。",
  // The same caveat, whole, on /security.
  "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen 目前是測試版。加密設計經過內部安全審查，還沒有做過外部稽核。在瀏覽器裡使用時，你需要信任當次載入的頁面程式碼：這份程式碼能使用你的私密金鑰，也能把它們複製出去。",

  "Your AI keeps working, wherever you are.": "AI 接著工作，你在哪都能接手。",
};

const ja: Bundle = {
  // Diagram cards: every row says something about the product.
  "your computer": "あなたのコンピュータ",
  "your phone": "あなたのスマホ",
  "browser": "ブラウザ",
  "computer off · {{time}}": "電源オフ · {{time}}",
  "Droppen can't read it": "Droppen には読めない",
  "done, tests pass": "完了、テスト通過",
  "Droppen relay": "Droppen 中継",
  "relay": "中継",
  "encrypted copy": "暗号化コピー",
  "read-only": "読み取り専用",
  "darken the sign-in button": "ログインボタンを濃い色にして",
  "passes the copy on": "コピーを手元へ渡す",
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
  "Claude Code and Codex keep running on your own Macs and Linux machines; every session on all of them lands in one list on your phone or in any browser, to answer, steer or open a terminal. End-to-end encrypted, with an open-source client.":
    "Claude Code と Codex は自分の Mac や Linux で動かしたまま。すべてのコンピュータのセッションがひとつのリストにまとまり、スマホやどのブラウザからでも答え、指示し、ターミナルを開けます。エンドツーエンド暗号化、クライアントはオープンソース。",
  "On this page": "このページ",
  "Product": "プロダクト",
  "Features": "機能",
  "Security": "セキュリティ",
  "Source code": "ソースコード",
  "Third-party licences": "サードパーティのライセンス",
  "How it works": "しくみ",
  "Open app": "アプリを開く",
  "Sign in": "ログイン",
  "Get started": "はじめる",

  "Your computer's AI, in your pocket.": "コンピュータの AI を、スマホで続きから。",
  "Claude Code and Codex keep running on your own computers. Answer them, steer them and open a terminal from your phone or any browser.":
    "Claude Code と Codex は、自分のコンピュータで動かしたまま。スマホやどのブラウザからでも、答えて、指示して、ターミナルも開けます。",
  "Start for free": "無料ではじめる",
  "Public beta: free for {{hosts}} computers.": "パブリックベータ。コンピュータ {{hosts}} 台まで無料。",
  "Invite-only for now: sign-up needs an invite code.": "いまは招待制です。登録には招待コードが必要です。",
  "Sign-up is closed for now.": "現在、新規登録を停止しています。",
  "See the plans": "プランを見る",
  "What Droppen looks like": "Droppen の画面",
  "Sessions": "セッション",
  "Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer":
    "Mac の Droppen：Claude Code と Codex のセッション一覧と、回答を待つセッション",
  "Split screen": "分割表示",
  "Droppen with three panes: two sessions and a terminal": "3 ペインの Droppen：2 つのセッションとターミナル",
  "Terminal": "ターミナル",
  "A terminal in Droppen": "Droppen のターミナル",
  "Cloud": "クラウド",
  "my.droppen.app: three computers and the list of what needs you": "my.droppen.app：3 台のコンピュータと「対応待ち」リスト",
  "Phone": "スマホ",
  "Droppen on a phone: what needs you, and a session's question": "スマホの Droppen：「対応待ち」リストとセッションの質問",
  "Letting a new device in with a QR code": "QR コードで新しいデバイスを迎え入れる",
  "Three steps, then it goes where you go.": "3 ステップで、どこからでも続きを。",
  "Put Droppen where your AI runs": "AI が動くコンピュータに Droppen を入れる",
  "Sign in on that computer": "そのコンピュータでログイン",

  "Install it where your AI runs": "AI が動くコンピュータに入れる",
  "The Mac app, or one command on Linux. Your sessions show up as they are.": "Mac はアプリ、Linux はコマンドを 1 行。いまあるセッションがそのまま表示されます。",
  "One click in the Mac app, or a one-time code on a server. Add your other computers to the same account.":
    "Mac アプリならクリックひとつ、サーバーならワンタイムコードで。ほかのコンピュータも同じアカウントに追加できます。",
  "Pick up on your phone": "スマホで続きから",
  "Open droppen.app on your phone, and let it in with a QR code from a device you already have.": "スマホで droppen.app を開き、手持ちのデバイスから QR コードで迎え入れます。",

  "End-to-end encrypted": "エンドツーエンド暗号化",
  "Your code, for your eyes only.": "あなたのコードは、あなたにしか見えない。",
  "Encrypted on your computer, opened only on your devices. Droppen Hub only keeps encrypted copies and Droppen relays only pass ciphertext on: neither holds a key that can read them.":
    "暗号化はあなたのコンピュータで、開けるのはあなたのデバイスだけ。Droppen Hub は暗号化コピーを預かるだけ、Droppen 中継は暗号文を渡すだけで、どちらも中身を読める鍵を持っていません。",
  "Only devices you approve get in": "入れるのは、承認したデバイスだけ",
  "A new phone or browser gets in only when a device you already have lets it in. Remove a lost one, and it can't open anything written afterwards.":
    "新しいスマホやブラウザは、手持ちのデバイスが迎え入れたときだけ加われます。なくしても外せば、その後に書かれたものは開けません。",
  "An open-source client": "クライアントはオープンソース",
  "The program on your computers, the Mac app and the web app are open source (AGPL-3.0). Droppen Hub is not open source.":
    "コンピュータ上のプログラム、Mac アプリ、Web アプリは AGPL-3.0 のオープンソース。Droppen Hub はオープンソースではありません。",
  "Read the source": "ソースを見る",

  "Typing from your phone stays snappy.": "スマホからの入力も、もたつかない。",
  "Keystrokes go straight to your computer when your phone can reach it, and through a Droppen relay when it can't. If the direct path drops, the relay takes over within 1.5 s and no keystroke is lost.":
    "スマホからコンピュータに直結できればキー入力はまっすぐ届き、できなければ Droppen 中継を通ります。直結が切れても 1.5 秒以内に中継が引き継ぎ、キー入力はひとつも失われません。",
  "same computer": "同じコンピュータ",
  "on 4G": "4G回線",
  "measured on": "計測した環境",
  "measured p50 round trip · direct {{d}} · via a Droppen relay {{h}}": "実測の往復 p50 · 直結 {{d}} · Droppen 中継経由 {{h}}",
  "direct": "直結",
  "lost": "切断",
  "direct path up": "直結中",
  "direct path lost: back on the Droppen relay, unconfirmed keys sent again in order": "直結が切断：Droppen 中継に戻り、未確認のキーを順番どおり再送",
  "network back: direct again": "ネットワーク復帰：再び直結",
  "keys {{sent}} · delivered {{delivered}} · lost 0": "キー {{sent}} · 到着 {{delivered}} · 欠落 0",
  "if the direct path drops, the Droppen relay takes over within 1.5 s and no keystroke is lost": "直結が切れても 1.5 秒以内に Droppen 中継が引き継ぎ、キー入力はひとつも失われません",

  "Lid closed? Your sessions are still there.": "ふたを閉じても、セッションは読める。",
  "While your computer is on, it leaves an encrypted copy of your sessions with Droppen Hub. Close the lid or lose the network, and your phone still opens any session from the last 30 days; sending and terminals wait for the computer to come back.":
    "コンピュータが動いている間、セッションは暗号化して Droppen Hub に預けられます。ふたを閉じても、ネットワークが切れても、スマホは直近 30 日のセッションを開けます。送信とターミナルはコンピュータの復帰を待ちます。",

  "Everything you would walk back to your desk for.": "席に戻らないとできなかったことが、スマホで。",
  "Every session, one list": "すべてのセッションを、ひとつのリストで",
  "Home and work computers on one account, and one list of what needs you across all of them.": "自宅と職場のコンピュータをひとつのアカウントに。全台の「対応待ち」が、ひとつのリストにまとまります。",
  "Answer it, steer it": "答えて、舵を取る",
  "Answer its questions, add a follow-up while it works, or stop a turn going the wrong way.": "質問に答える、作業中に指示を追加する、方向を間違えたターンを止める。",
  "Told when it needs you": "呼ばれたら、すぐわかる",
  "A notification when an agent asks you something, finishes a turn you haven't read, or fails.": "agent が質問したとき、まだ読んでいないターンが終わったとき、失敗したときに通知します。",
  "Start one from your phone": "スマホから新しく始める",
  "Pick the agent, the folder and the computer, attach a screenshot, and send.": "agent とフォルダとコンピュータを選び、スクリーンショットを添えて送信。",
  "Hit a limit? Carry on": "上限に達しても、続けられる",
  "Add a few Claude accounts: when one hits its limit, a session can switch to another by itself.":
    "Claude のアカウントを複数登録すれば、ひとつが上限に達しても、セッションは別のアカウントで自動的に続けられます。",
  "Prompts on a schedule": "スケジュール実行",
  "Run a prompt every morning, or every hour on weekdays, on the computer you choose.": "毎朝、平日は毎時など、決めたタイミングで指示を自動実行。動かすコンピュータも選べます。",

  "Droppen is in beta. Its encryption has had internal security reviews, not an external audit. In a browser, the page code as loaded can use your keys, and could copy them out.":
    "Droppen はベータ版です。暗号設計は社内のセキュリティレビューを受けていますが、外部監査はまだです。ブラウザでは、読み込んだページのコードが秘密鍵を使え、持ち出すこともできます。",
  // The same caveat, whole, on /security.
  "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen はベータ版です。暗号設計は社内のセキュリティレビューを受けていますが、外部監査はまだです。ブラウザで使う場合は、そのとき読み込んだページのコードを信頼することになります。そのコードは秘密鍵を使えるうえ、持ち出すこともできます。",

  "Your AI keeps working, wherever you are.": "AI は動き続ける。どこにいても続きから。",
};

/** The landing page's tables, by i18next language. */
export const landingBundles: Record<"zh-CN" | "zh-TW" | "ja", Bundle> = {
  "zh-CN": { ...zhCN, ...downloadZhCN, ...pricingZhCN },
  "zh-TW": { ...zhTW, ...downloadZhTW, ...pricingZhTW },
  ja: { ...ja, ...downloadJa, ...pricingJa },
};
