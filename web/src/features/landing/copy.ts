/*
 * The landing page's words. As everywhere in the app (`src/i18n`), the keys
 * are the English text, so English needs no table. zh-CN is the page's first
 * language, written as native copy rather than translated from the English;
 * zh-TW follows the app's Taiwan wording (工作階段, 轉送, 資料, 裝置) and ja
 * its Japanese (直結, 中継経由, デバイス, 対応待ち).
 *
 * The voice is a product page's, not a manual's: every headline says what the
 * reader gets (their computer's AI on their phone, every session in one list,
 * the agent's question answered from anywhere), and the body under it gives
 * the proof and, briefly, the how. Droppen Hub, relays, ciphertext and p50s
 * come after the benefit, as its support, never as the opening; the hero
 * names none of them. End-to-end encryption is said in the hero's last words
 * and in one section near the end, and the rest of it is /security's. "We" /
 * "our servers" are Droppen Hub and the Droppen relays. A device is a 设备 / 裝置 / デバイス; the copy says 你的电脑 only for
 * the reader's own physical computer, and never 机器.
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
 * p50s, and nothing says "unhackable", "audited" or "open source".
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
  "make the sign-in button dark": "把登录按钮改成深色",
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
  "Speed": "速度",
  "Offline": "离线",
  "Security": "安全",
  "Source code": "源代码",
  "Third-party licences": "第三方许可证",
  "How it works": "工作原理",
  "Open app": "打开应用",
  "Sign in": "登录",
  "Get started": "开始使用",

  // Hero
  "For Claude Code and Codex": "支持 Claude Code 和 Codex",
  "Your computer's AI, in your pocket.": "电脑上的 AI，手机上接着用。",
  "Claude Code and Codex keep running on your own Macs and Linux machines. Every session on all of them lands in one list on your phone or in any browser: answer it, send the next prompt, open a terminal.":
    "Claude Code 和 Codex 照常在你自己的 Mac 和 Linux 上跑。所有电脑上的会话汇成一张清单，手机或任何浏览器里都能接手：回答它、发下一条指令、开终端。",
  "Start for free": "免费开始",
  "Public beta: free for {{hosts}} computers.": "公开测试版，{{hosts}} 台电脑以内免费。",
  "macOS · Linux · any browser": "macOS · Linux · 任何浏览器",
  "your own Claude and Codex accounts": "用你自己的 Claude 和 Codex 账号",
  "{{ms}} ms from a phone on 4G, direct": "手机 4G 直连 {{ms}} ms",
  "end-to-end encrypted": "端到端加密",
  "open-source client": "客户端开源",
  "Invite-only for now: sign-up needs an invite code.": "目前邀请制，注册需要邀请码。",
  "Sign-up is closed for now.": "目前暂停注册。",
  "See the plans": "查看各个方案",
  "What Droppen looks like": "Droppen 长什么样",
  "Sessions": "会话",
  "Every session, as it runs": "每个会话，实时在眼前",
  "The transcript, every tool it ran, and the question it is waiting on. Answer it, and it carries on.":
    "完整的对话、它调用过的每个工具，还有它正等你回答的问题。回答它，它就接着干。",
  "Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer":
    "Mac 上的 Droppen：Claude Code 和 Codex 的会话列表，以及一个等你回答的会话",
  "Split screen": "分屏",
  "Sessions and terminals, side by side": "会话和终端，并排摆开",
  "Lay out several panes at once: a session, a terminal, another session, each on whichever computer you choose.":
    "一次摆开几个窗格：一个会话、一个终端、再一个会话，每个都可以来自不同的电脑。",
  "Droppen with three panes: two sessions and a terminal": "Droppen 的三栏分屏：两个会话和一个终端",
  "Terminal": "终端",
  "A real terminal on any of your computers": "任何一台电脑上的真终端",
  "Open a shell on any computer of yours, from the browser on your phone or anywhere else.": "在手机或任何浏览器里，打开你任何一台电脑的 shell。",
  "A terminal in Droppen": "Droppen 里的终端",
  "Cloud": "云端",
  "Every computer on one account": "所有电脑，一个账户管起来",
  "my.droppen.app shows every computer on your account, and one list of what needs you across all of them.":
    "my.droppen.app 上能看到账户下的每台电脑，以及所有电脑上「需要你」的那张清单。",
  "my.droppen.app: three computers and the list of what needs you": "my.droppen.app：三台电脑，以及「需要你」清单",
  "Phone": "手机",
  "Made for the phone in your hand": "为手里的手机而做",
  "No app to install: open droppen.app, add it to your home screen, and be told when an agent needs you.":
    "不用装 App：打开 droppen.app，添加到主屏幕，Agent 需要你时就会通知你。",
  "Droppen on a phone: what needs you, and a session's question": "手机上的 Droppen：「需要你」清单，以及一个会话的提问",
  "Only the devices you let in": "只有你放行的设备",
  "A new phone or browser gets in only when a device you already have lets it in, with a QR code. End-to-end encrypted.":
    "新手机或新浏览器，只有你已有的设备扫码放行才能加入。全程端到端加密。",
  "Letting a new device in with a QR code": "扫码让新设备加入",
  "Three steps, then it goes where you go.": "三步装好，走到哪都能接手。",
  "Put Droppen where your AI runs": "把 Droppen 装在跑 AI 的电脑上",
  "Sign in on that computer": "在这台电脑上登录",

  // How it works: three steps, each true of the build (first-host.tsx, add-computer.tsx, pairing-screen.tsx)
  "Install it where your AI runs": "装在跑 AI 的电脑上",
  "The Mac app, or one command on Linux, on the computer that has Claude Code or Codex. Your sessions show up as they are: nothing to move or set up.":
    "在装了 Claude Code 或 Codex 的电脑上，装 Mac 应用，或者在 Linux 上跑一条命令。已有的会话原样出现，不用搬、不用配。",
  "One click in the Mac app, or a one-time code on a server. Add the computers at home and at work to the same account.":
    "Mac 应用里点一下就行，服务器上用一次性口令。家里和公司的电脑都能加进同一个账户。",
  "Pick up on your phone": "在手机上接着用",
  "Open droppen.app on your phone and let it in from a device you already have, with a QR code. Add it to your home screen, turn on notifications, and you hear when an agent needs you.":
    "在手机上打开 droppen.app，用你已有的设备扫码让它加入。添加到主屏幕、打开通知，agent 需要你时你就会知道。",

  // Your code, for your eyes only (the one section on encryption, near the end)
  "End-to-end encrypted": "端到端加密",
  "Your code, for your eyes only.": "你的代码，只有你看得到。",
  "Encrypted on your computer, opened only on the devices you approved. In between, Droppen Hub only finds your computers and keeps their encrypted copies; Droppen relays only pass ciphertext on. Neither holds a key that can read them.":
    "在你的电脑上加密，只在你批准的设备上解开。中间的 Droppen Hub 只负责找到你的电脑、保管加密副本，Droppen 中继只负责转发密文，两者都没有能读懂内容的钥匙。",
  "Only devices you approve get in": "只有你批准的设备进得来",
  "A new phone or browser gets in only when a device you already have lets it in, with a QR code or a one-time code. Lost one? Remove it, and it cannot open anything written afterwards.":
    "新手机、新浏览器，都要由你已有的设备扫码或输入一次性口令放行才能加入。手机丢了？移除它，之后写下的内容它一个字也解不开。",
  "Nothing readable on our servers": "我们的服务器上，什么都读不了",
  "An open-source client": "客户端开源",
  "The program on your computers, the Mac app and the web app are open source (AGPL-3.0): what handles your code can be read and checked. Droppen Hub is not open source, and holds no key that can read anything.":
    "你电脑上的程序、Mac 应用和网页应用都以 AGPL-3.0 开源：经手你代码的部分，谁都能读、能查。Droppen Hub 不开源，也没有任何能读懂内容的钥匙。",
  "Read the source": "查看源代码",
  "What is said in a session, typed in a terminal or kept in your files, and the names of your sessions, folders and models: none of it can be read on our servers. They do see your e-mail address, which devices are online and from which IP address, and how much data moves, when.":
    "会话里说了什么、终端里敲了什么、文件里存了什么，还有会话、目录和模型的名字，我们的服务器一样都读不了。它们看得到的是：你的邮箱、哪些设备在线、从哪个 IP 连接，以及什么时候传了多少数据。",

  // Typing from your phone stays snappy (the direct path)
  "Typing from your phone stays snappy.": "在手机上打字，照样跟手。",
  "From a phone on 4G, a keystroke's round trip is 63–73 ms direct and 215–227 ms through a Droppen relay (measured p50).":
    "手机走 4G 时，按键往返直连 63–73 ms，经 Droppen 中继 215–227 ms（实测 p50）。",
  "When your phone can reach your computer directly, keystrokes go straight across. When it cannot, a Droppen relay takes over by itself: you never pick a path.":
    "手机能直连电脑时，按键直接过去；连不通时，Droppen 中继自动接手。走哪条路，你不用管。",
  "No keystroke lost: if the direct path drops, traffic is back on the Droppen relay within 1.5 s, and unconfirmed keystrokes are sent again in order. None lost, doubled or reordered.":
    "一个按键都不丢：直连断了，1.5 秒内退回 Droppen 中继，还没确认的按键按原顺序重发——不丢、不重、不乱序。",
  "So far about two in three testers got a direct path; the rest go through a Droppen relay automatically.":
    "目前大约三分之二的测试者打通了直连，其余的自动走 Droppen 中继。",
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
  "Close the lid or lose the network, and your phone still opens every session as it last was: while your computer is on, the agent leaves an encrypted copy of what is new with Droppen Hub every few seconds.":
    "合上电脑、断了网，手机照样能打开每个会话的最新状态：电脑开着的时候，agent 每隔几秒就把新内容加密存一份到 Droppen Hub。",
  "Read-only meanwhile: sending and terminals wait for your computer to come back.": "这期间只读：发消息和用终端要等电脑回来。",
  "Opens at once next time: each device keeps its own copy, cleared when you sign out.": "下次秒开：每台设备都留一份副本，退出登录时清除。",
  "What syncs: each computer's own sessions updated in the last 30 days.": "同步范围：每台电脑自己的、最近 30 天内有更新的会话。",

  // What you get
  "What you get": "功能",
  "Everything you would walk back to your desk for.": "本来要走回工位才能做的事，手机上都能做。",
  "Every session, one list": "所有会话，一个列表",
  "The computers at home and at work, side by side on one account, and one list of what needs you across all of them: which session is asking, which has news, which is still working.":
    "家里和公司的电脑挂在同一个账户下并排看；所有设备上谁在问你、谁有新消息、谁还在干活，汇成一张「需要你」清单。",
  "Answer it, steer it": "回答它，也能指挥它",
  "Answer an agent's question, send a follow-up (it waits its turn while the agent works), or stop a turn that is going the wrong way.":
    "回答 agent 的提问，追加一条消息（agent 忙着时它会排队等着），或者把跑偏的这一轮停下来。",
  "Told when it needs you": "需要你时，第一时间知道",
  "A notification when an agent stops to ask, when a turn ends and you have not read it, or when a run fails. Not one for every line it prints.":
    "agent 停下来提问、一轮跑完而你还没看，或者运行失败时，都会通知你。不会每输出一行就通知一次。",
  "Start one from your phone": "在手机上开新会话",
  "Pick the agent, the folder and the computer, attach a screenshot, and send: the session starts on that computer.":
    "选好 agent、目录和哪台电脑，附上截图，发出去：会话就在那台电脑上开跑。",
  "Hit a limit? Carry on": "额度用完？接着跑",
  "Add a few Claude accounts, and when one hits its limit a session can carry on with another by itself.":
    "添加几个 Claude 账号，一个额度用完，会话能自动换到另一个接着跑。",
  "Prompts on a schedule": "定时任务",
  "Have a prompt run by itself every morning, or every hour on weekdays, on the computer you choose.": "让一条指令每天早上、或工作日每小时自己跑一次，在你选的那台电脑上。",

  // The beta's caveat, under that section
  "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen 目前是测试版。加密设计经过了内部安全评审，还没有做过外部审计。在浏览器里使用时，你需要信任当次加载的页面代码：这份代码能使用你的私钥，也能把它们复制出去。",

  // Footer
  "Invite-only beta": "邀请制测试版",
  "Public beta": "公开测试版",
  "Beta": "测试版",
  "Your AI keeps working, wherever you are.": "AI 接着干活，你在哪都能接手。",
  "Sign up with your invite code, connect your computer, and pick up from your phone.": "用邀请码注册，接上你的电脑，然后在手机上接着用。",
  "Sign up with your e-mail, connect your computer, and pick up from your phone.": "用邮箱注册，接上你的电脑，然后在手机上接着用。",
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
  "make the sign-in button dark": "把登入按鈕改成深色",
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
  "Speed": "速度",
  "Offline": "離線",
  "Security": "安全",
  "Source code": "原始碼",
  "Third-party licences": "第三方授權",
  "How it works": "運作方式",
  "Open app": "打開應用",
  "Sign in": "登入",
  "Get started": "開始使用",

  "For Claude Code and Codex": "支援 Claude Code 和 Codex",
  "Your computer's AI, in your pocket.": "電腦上的 AI，手機上接著用。",
  "Claude Code and Codex keep running on your own Macs and Linux machines. Every session on all of them lands in one list on your phone or in any browser: answer it, send the next prompt, open a terminal.":
    "Claude Code 和 Codex 照常在你自己的 Mac 和 Linux 上跑。所有電腦上的工作階段匯成一張清單，手機或任何瀏覽器裡都能接手：回答它、送出下一個指令、開終端機。",
  "Start for free": "免費開始",
  "Public beta: free for {{hosts}} computers.": "公開測試版，{{hosts}} 台電腦以內免費。",
  "macOS · Linux · any browser": "macOS · Linux · 任何瀏覽器",
  "your own Claude and Codex accounts": "用你自己的 Claude 和 Codex 帳號",
  "{{ms}} ms from a phone on 4G, direct": "手機 4G 直連 {{ms}} ms",
  "end-to-end encrypted": "端對端加密",
  "open-source client": "客戶端開源",
  "Invite-only for now: sign-up needs an invite code.": "目前採邀請制，註冊需要邀請碼。",
  "Sign-up is closed for now.": "目前暫停註冊。",
  "See the plans": "查看各個方案",
  "What Droppen looks like": "Droppen 長什麼樣",
  "Sessions": "工作階段",
  "Every session, as it runs": "每個工作階段，即時在眼前",
  "The transcript, every tool it ran, and the question it is waiting on. Answer it, and it carries on.":
    "完整的對話、它呼叫過的每個工具，還有它正等你回答的問題。回答它，它就接著做。",
  "Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer":
    "Mac 上的 Droppen：Claude Code 和 Codex 的工作階段清單，以及一個等你回答的工作階段",
  "Split screen": "分割畫面",
  "Sessions and terminals, side by side": "工作階段和終端機，並排擺開",
  "Lay out several panes at once: a session, a terminal, another session, each on whichever computer you choose.":
    "一次擺開幾個窗格：一個工作階段、一個終端機、再一個工作階段，每個都可以來自不同的電腦。",
  "Droppen with three panes: two sessions and a terminal": "Droppen 的三欄分割畫面：兩個工作階段和一個終端機",
  "Terminal": "終端機",
  "A real terminal on any of your computers": "任何一台電腦上的真終端機",
  "Open a shell on any computer of yours, from the browser on your phone or anywhere else.": "在手機或任何瀏覽器裡，打開你任何一台電腦的 shell。",
  "A terminal in Droppen": "Droppen 裡的終端機",
  "Cloud": "雲端",
  "Every computer on one account": "所有電腦，一個帳戶管起來",
  "my.droppen.app shows every computer on your account, and one list of what needs you across all of them.":
    "my.droppen.app 上能看到帳戶下的每台電腦，以及所有電腦上「需要你」的那張清單。",
  "my.droppen.app: three computers and the list of what needs you": "my.droppen.app：三台電腦，以及「需要你」清單",
  "Phone": "手機",
  "Made for the phone in your hand": "為手上的手機而做",
  "No app to install: open droppen.app, add it to your home screen, and be told when an agent needs you.":
    "不用裝 App：打開 droppen.app，加到主畫面，Agent 需要你時就會通知你。",
  "Droppen on a phone: what needs you, and a session's question": "手機上的 Droppen：「需要你」清單，以及一個工作階段的提問",
  "Only the devices you let in": "只有你放行的裝置",
  "A new phone or browser gets in only when a device you already have lets it in, with a QR code. End-to-end encrypted.":
    "新手機或新瀏覽器，只有你已有的裝置掃碼放行才能加入。全程端對端加密。",
  "Letting a new device in with a QR code": "掃碼讓新裝置加入",
  "Three steps, then it goes where you go.": "三步裝好，走到哪都能接手。",
  "Put Droppen where your AI runs": "把 Droppen 裝在跑 AI 的電腦上",
  "Sign in on that computer": "在這台電腦上登入",

  "Install it where your AI runs": "裝在跑 AI 的電腦上",
  "The Mac app, or one command on Linux, on the computer that has Claude Code or Codex. Your sessions show up as they are: nothing to move or set up.":
    "在裝了 Claude Code 或 Codex 的電腦上，安裝 Mac 應用程式，或在 Linux 上執行一行指令。既有的工作階段原樣出現，不用搬、不用設定。",
  "One click in the Mac app, or a one-time code on a server. Add the computers at home and at work to the same account.":
    "Mac 應用程式裡點一下就好，伺服器上用一次性代碼。家裡和公司的電腦都能加進同一個帳戶。",
  "Pick up on your phone": "在手機上接著用",
  "Open droppen.app on your phone and let it in from a device you already have, with a QR code. Add it to your home screen, turn on notifications, and you hear when an agent needs you.":
    "在手機上打開 droppen.app，用你已有的裝置掃碼讓它加入。加到主畫面、開啟通知，agent 需要你時你就會知道。",

  "End-to-end encrypted": "端對端加密",
  "Your code, for your eyes only.": "你的程式碼，只有你看得到。",
  "Encrypted on your computer, opened only on the devices you approved. In between, Droppen Hub only finds your computers and keeps their encrypted copies; Droppen relays only pass ciphertext on. Neither holds a key that can read them.":
    "在你的電腦上加密，只在你核准的裝置上解開。中間的 Droppen Hub 只負責找到你的電腦、保管加密副本，Droppen 中繼只負責轉送密文，兩者都沒有能讀懂內容的金鑰。",
  "Only devices you approve get in": "只有你核准的裝置進得來",
  "A new phone or browser gets in only when a device you already have lets it in, with a QR code or a one-time code. Lost one? Remove it, and it cannot open anything written afterwards.":
    "新手機、新瀏覽器，都要由你已有的裝置掃碼或輸入一次性代碼放行才能加入。手機掉了？移除它，之後寫下的內容它一個字也解不開。",
  "Nothing readable on our servers": "我們的伺服器上，什麼都讀不了",
  "An open-source client": "客戶端開源",
  "The program on your computers, the Mac app and the web app are open source (AGPL-3.0): what handles your code can be read and checked. Droppen Hub is not open source, and holds no key that can read anything.":
    "你電腦上的程式、Mac 應用程式和網頁應用程式都以 AGPL-3.0 開源：經手你程式碼的部分，誰都能讀、能查。Droppen Hub 不開源，也沒有任何能讀懂內容的金鑰。",
  "Read the source": "查看原始碼",
  "What is said in a session, typed in a terminal or kept in your files, and the names of your sessions, folders and models: none of it can be read on our servers. They do see your e-mail address, which devices are online and from which IP address, and how much data moves, when.":
    "工作階段裡說了什麼、終端機裡打了什麼、檔案裡存了什麼，還有工作階段、資料夾和模型的名稱，我們的伺服器一樣都讀不了。它們看得到的是：你的電子郵件、哪些裝置在線上、從哪個 IP 連線，以及什麼時候傳了多少資料。",

  "Typing from your phone stays snappy.": "在手機上打字，照樣跟手。",
  "From a phone on 4G, a keystroke's round trip is 63–73 ms direct and 215–227 ms through a Droppen relay (measured p50).":
    "手機走 4G 時，按鍵往返直連 63–73 ms，經 Droppen 中繼 215–227 ms（實測 p50）。",
  "When your phone can reach your computer directly, keystrokes go straight across. When it cannot, a Droppen relay takes over by itself: you never pick a path.":
    "手機能直連電腦時，按鍵直接過去；連不通時，Droppen 中繼自動接手。走哪條路，你不用管。",
  "No keystroke lost: if the direct path drops, traffic is back on the Droppen relay within 1.5 s, and unconfirmed keystrokes are sent again in order. None lost, doubled or reordered.":
    "一個按鍵都不丟：直連斷了，1.5 秒內退回 Droppen 中繼，還沒確認的按鍵依原順序重送——不丟、不重、不亂序。",
  "So far about two in three testers got a direct path; the rest go through a Droppen relay automatically.":
    "目前大約三分之二的測試者打通了直連，其餘的自動走 Droppen 中繼。",
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
  "Close the lid or lose the network, and your phone still opens every session as it last was: while your computer is on, the agent leaves an encrypted copy of what is new with Droppen Hub every few seconds.":
    "闔上電腦、斷了網，手機照樣能打開每個工作階段的最新狀態：電腦開著的時候，agent 每隔幾秒就把新內容加密存一份到 Droppen Hub。",
  "Read-only meanwhile: sending and terminals wait for your computer to come back.": "這段期間唯讀：傳訊息和用終端機要等電腦回來。",
  "Opens at once next time: each device keeps its own copy, cleared when you sign out.": "下次秒開：每台裝置都留一份副本，登出時清除。",
  "What syncs: each computer's own sessions updated in the last 30 days.": "同步範圍：每台電腦自己的、最近 30 天內有更新的工作階段。",

  "What you get": "功能",
  "Everything you would walk back to your desk for.": "原本要走回座位才能做的事，手機上都能做。",
  "Every session, one list": "所有工作階段，一個清單",
  "The computers at home and at work, side by side on one account, and one list of what needs you across all of them: which session is asking, which has news, which is still working.":
    "家裡和公司的電腦掛在同一個帳戶下並排看；所有裝置上誰在問你、誰有新訊息、誰還在工作，匯成一張「需要你」清單。",
  "Answer it, steer it": "回答它，也能指揮它",
  "Answer an agent's question, send a follow-up (it waits its turn while the agent works), or stop a turn that is going the wrong way.":
    "回答 agent 的提問，追加一則訊息（agent 忙的時候它會排隊等著），或把跑偏的這一輪停下來。",
  "Told when it needs you": "需要你時，第一時間知道",
  "A notification when an agent stops to ask, when a turn ends and you have not read it, or when a run fails. Not one for every line it prints.":
    "agent 停下來提問、一輪跑完而你還沒看，或執行失敗時，都會通知你。不會每輸出一行就通知一次。",
  "Start one from your phone": "在手機上開新的工作階段",
  "Pick the agent, the folder and the computer, attach a screenshot, and send: the session starts on that computer.":
    "選好 agent、資料夾和哪台電腦，附上截圖，送出：工作階段就在那台電腦上開跑。",
  "Hit a limit? Carry on": "額度用完？接著跑",
  "Add a few Claude accounts, and when one hits its limit a session can carry on with another by itself.":
    "加入幾個 Claude 帳號，一個額度用完，工作階段能自動換到另一個接著跑。",
  "Prompts on a schedule": "排程任務",
  "Have a prompt run by itself every morning, or every hour on weekdays, on the computer you choose.": "讓一個指令每天早上、或工作日每小時自己跑一次，在你選的那台電腦上。",

  "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen 目前是測試版。加密設計經過內部安全審查，還沒有做過外部稽核。在瀏覽器裡使用時，你需要信任當次載入的頁面程式碼：這份程式碼能使用你的私密金鑰，也能把它們複製出去。",

  "Invite-only beta": "邀請制測試版",
  "Public beta": "公開測試版",
  "Beta": "測試版",
  "Your AI keeps working, wherever you are.": "AI 接著工作，你在哪都能接手。",
  "Sign up with your invite code, connect your computer, and pick up from your phone.": "用邀請碼註冊，接上你的電腦，然後在手機上接著用。",
  "Sign up with your e-mail, connect your computer, and pick up from your phone.": "用電子郵件註冊，接上你的電腦，然後在手機上接著用。",
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
  "make the sign-in button dark": "ログインボタンを濃い色にして",
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
  "Speed": "速さ",
  "Offline": "オフライン",
  "Security": "セキュリティ",
  "Source code": "ソースコード",
  "Third-party licences": "サードパーティのライセンス",
  "How it works": "しくみ",
  "Open app": "アプリを開く",
  "Sign in": "ログイン",
  "Get started": "はじめる",

  "For Claude Code and Codex": "Claude Code と Codex に対応",
  "Your computer's AI, in your pocket.": "コンピュータの AI を、スマホで続きから。",
  "Claude Code and Codex keep running on your own Macs and Linux machines. Every session on all of them lands in one list on your phone or in any browser: answer it, send the next prompt, open a terminal.":
    "Claude Code と Codex は自分の Mac や Linux で動かしたまま。すべてのコンピュータのセッションがひとつのリストにまとまり、スマホでもどのブラウザでも、答える、次の指示を送る、ターミナルを開くがその場でできます。",
  "Start for free": "無料ではじめる",
  "Public beta: free for {{hosts}} computers.": "パブリックベータ。コンピュータ {{hosts}} 台まで無料。",
  "macOS · Linux · any browser": "macOS · Linux · どのブラウザでも",
  "your own Claude and Codex accounts": "自分の Claude・Codex アカウントのまま",
  "{{ms}} ms from a phone on 4G, direct": "4G のスマホから直結 {{ms}} ms",
  "end-to-end encrypted": "エンドツーエンド暗号化",
  "open-source client": "クライアントはオープンソース",
  "Invite-only for now: sign-up needs an invite code.": "いまは招待制です。登録には招待コードが必要です。",
  "Sign-up is closed for now.": "現在、新規登録を停止しています。",
  "See the plans": "プランを見る",
  "What Droppen looks like": "Droppen の画面",
  "Sessions": "セッション",
  "Every session, as it runs": "セッションを、動いているそのままに",
  "The transcript, every tool it ran, and the question it is waiting on. Answer it, and it carries on.":
    "会話のすべて、実行したツール、そしていま返事を待っている質問。答えれば、そのまま続きます。",
  "Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer":
    "Mac の Droppen：Claude Code と Codex のセッション一覧と、回答を待つセッション",
  "Split screen": "分割表示",
  "Sessions and terminals, side by side": "セッションとターミナルを並べて",
  "Lay out several panes at once: a session, a terminal, another session, each on whichever computer you choose.":
    "いくつものペインを同時に。セッション、ターミナル、別のセッション。それぞれ好きなコンピュータのものを。",
  "Droppen with three panes: two sessions and a terminal": "3 ペインの Droppen：2 つのセッションとターミナル",
  "Terminal": "ターミナル",
  "A real terminal on any of your computers": "どのコンピュータでも、本物のターミナル",
  "Open a shell on any computer of yours, from the browser on your phone or anywhere else.": "スマホでも、どのブラウザからでも、手持ちのどのコンピュータのシェルも開けます。",
  "A terminal in Droppen": "Droppen のターミナル",
  "Cloud": "クラウド",
  "Every computer on one account": "すべてのコンピュータを、ひとつのアカウントで",
  "my.droppen.app shows every computer on your account, and one list of what needs you across all of them.":
    "my.droppen.app には、アカウントのすべてのコンピュータと、全台の「対応待ち」リストが並びます。",
  "my.droppen.app: three computers and the list of what needs you": "my.droppen.app：3 台のコンピュータと「対応待ち」リスト",
  "Phone": "スマホ",
  "Made for the phone in your hand": "手の中のスマホのために",
  "No app to install: open droppen.app, add it to your home screen, and be told when an agent needs you.":
    "アプリのインストールは不要。droppen.app を開いてホーム画面に追加すれば、agent に呼ばれたときに通知が届きます。",
  "Droppen on a phone: what needs you, and a session's question": "スマホの Droppen：「対応待ち」リストとセッションの質問",
  "Only the devices you let in": "入れるのは、迎え入れたデバイスだけ",
  "A new phone or browser gets in only when a device you already have lets it in, with a QR code. End-to-end encrypted.":
    "新しいスマホやブラウザは、手持ちのデバイスが QR コードで迎え入れたときだけ加われます。エンドツーエンド暗号化。",
  "Letting a new device in with a QR code": "QR コードで新しいデバイスを迎え入れる",
  "Three steps, then it goes where you go.": "3 ステップで、どこからでも続きを。",
  "Put Droppen where your AI runs": "AI が動くコンピュータに Droppen を入れる",
  "Sign in on that computer": "そのコンピュータでログイン",

  "Install it where your AI runs": "AI が動くコンピュータに入れる",
  "The Mac app, or one command on Linux, on the computer that has Claude Code or Codex. Your sessions show up as they are: nothing to move or set up.":
    "Claude Code か Codex が入っているコンピュータに、Mac アプリを入れるか、Linux ならコマンドを 1 行。いまあるセッションがそのまま表示され、移行も設定も要りません。",
  "One click in the Mac app, or a one-time code on a server. Add the computers at home and at work to the same account.":
    "Mac アプリならクリックひとつ、サーバーならワンタイムコードで。自宅と職場のコンピュータを同じアカウントに追加できます。",
  "Pick up on your phone": "スマホで続きから",
  "Open droppen.app on your phone and let it in from a device you already have, with a QR code. Add it to your home screen, turn on notifications, and you hear when an agent needs you.":
    "スマホで droppen.app を開き、手持ちのデバイスから QR コードで迎え入れます。ホーム画面に追加して通知をオンにすれば、agent に呼ばれたときにわかります。",

  "End-to-end encrypted": "エンドツーエンド暗号化",
  "Your code, for your eyes only.": "あなたのコードは、あなたにしか見えない。",
  "Encrypted on your computer, opened only on the devices you approved. In between, Droppen Hub only finds your computers and keeps their encrypted copies; Droppen relays only pass ciphertext on. Neither holds a key that can read them.":
    "暗号化はあなたのコンピュータで、開けるのはあなたが承認したデバイスだけ。その間、Droppen Hub はあなたのコンピュータを見つけて暗号化コピーを預かるだけ、Droppen 中継は暗号文を渡すだけで、どちらも中身を読める鍵を持っていません。",
  "Only devices you approve get in": "入れるのは、承認したデバイスだけ",
  "A new phone or browser gets in only when a device you already have lets it in, with a QR code or a one-time code. Lost one? Remove it, and it cannot open anything written afterwards.":
    "新しいスマホやブラウザが加われるのは、手持ちのデバイスが QR コードかワンタイムコードで迎え入れたときだけ。なくしても外せば、その後に書かれたものは開けません。",
  "Nothing readable on our servers": "私たちのサーバーでは、何も読めない",
  "An open-source client": "クライアントはオープンソース",
  "The program on your computers, the Mac app and the web app are open source (AGPL-3.0): what handles your code can be read and checked. Droppen Hub is not open source, and holds no key that can read anything.":
    "コンピュータ上のプログラム、Mac アプリ、Web アプリは AGPL-3.0 のオープンソース。あなたのコードに触れる部分は、誰でも読んで確かめられます。Droppen Hub はオープンソースではなく、中身を読める鍵も持っていません。",
  "Read the source": "ソースを見る",
  "What is said in a session, typed in a terminal or kept in your files, and the names of your sessions, folders and models: none of it can be read on our servers. They do see your e-mail address, which devices are online and from which IP address, and how much data moves, when.":
    "セッションの会話、ターミナルの入力、ファイルの中身、そしてセッション・フォルダ・モデルの名前。どれも私たちのサーバーでは読めません。見えるのは、メールアドレス、どのデバイスがどの IP アドレスからオンラインか、いつどれだけのデータが流れたかです。",

  "Typing from your phone stays snappy.": "スマホからの入力も、もたつかない。",
  "From a phone on 4G, a keystroke's round trip is 63–73 ms direct and 215–227 ms through a Droppen relay (measured p50).":
    "4G のスマホからのキー入力の往復は、直結で 63–73 ms、Droppen 中継経由で 215–227 ms（実測 p50）。",
  "When your phone can reach your computer directly, keystrokes go straight across. When it cannot, a Droppen relay takes over by itself: you never pick a path.":
    "スマホからコンピュータに直結できればキー入力はまっすぐ届き、できなければ Droppen 中継が自動で引き継ぎます。経路を選ぶ必要はありません。",
  "No keystroke lost: if the direct path drops, traffic is back on the Droppen relay within 1.5 s, and unconfirmed keystrokes are sent again in order. None lost, doubled or reordered.":
    "キー入力はひとつも失われません。直結が切れると 1.5 秒以内に Droppen 中継へ戻り、未確認のキー入力を順番どおりに再送します。欠けず、重複せず、順序も崩れません。",
  "So far about two in three testers got a direct path; the rest go through a Droppen relay automatically.":
    "これまでのところ、テスターの約 3 人に 2 人が直結でき、残りは自動で Droppen 中継を経由しています。",
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
  "Close the lid or lose the network, and your phone still opens every session as it last was: while your computer is on, the agent leaves an encrypted copy of what is new with Droppen Hub every few seconds.":
    "ふたを閉じても、ネットワークが切れても、スマホはどのセッションも最後の状態で開けます。コンピュータが動いている間、agent が数秒ごとに新しい内容を暗号化して Droppen Hub に預けているからです。",
  "Read-only meanwhile: sending and terminals wait for your computer to come back.": "その間は読み取り専用。送信とターミナルはコンピュータの復帰を待ちます。",
  "Opens at once next time: each device keeps its own copy, cleared when you sign out.": "次はすぐ開けます。各デバイスにもコピーを残し、ログアウトすると消去します。",
  "What syncs: each computer's own sessions updated in the last 30 days.": "同期の対象：各コンピュータ自身のセッションのうち、過去 30 日に更新されたもの。",

  "What you get": "できること",
  "Everything you would walk back to your desk for.": "席に戻らないとできなかったことが、スマホで。",
  "Every session, one list": "すべてのセッションを、ひとつのリストで",
  "The computers at home and at work, side by side on one account, and one list of what needs you across all of them: which session is asking, which has news, which is still working.":
    "自宅と職場のコンピュータをひとつのアカウントに並べて表示。質問中、新着あり、作業中のセッションは、すべてのデバイス分が「対応待ち」のリストにまとまります。",
  "Answer it, steer it": "答えて、舵を取る",
  "Answer an agent's question, send a follow-up (it waits its turn while the agent works), or stop a turn that is going the wrong way.":
    "agent の質問に答える、指示を追加する（作業中なら順番を待ちます）、方向を間違えたターンを止める。",
  "Told when it needs you": "呼ばれたら、すぐわかる",
  "A notification when an agent stops to ask, when a turn ends and you have not read it, or when a run fails. Not one for every line it prints.":
    "agent が手を止めて質問したとき、ターンが終わってまだ読んでいないとき、実行が失敗したときに通知します。出力のたびに鳴ることはありません。",
  "Start one from your phone": "スマホから新しく始める",
  "Pick the agent, the folder and the computer, attach a screenshot, and send: the session starts on that computer.":
    "agent とフォルダとコンピュータを選び、スクリーンショットを添えて送信。そのコンピュータでセッションが始まります。",
  "Hit a limit? Carry on": "上限に達しても、続けられる",
  "Add a few Claude accounts, and when one hits its limit a session can carry on with another by itself.":
    "Claude のアカウントを複数登録すれば、ひとつが上限に達しても、セッションは別のアカウントで自動的に続けられます。",
  "Prompts on a schedule": "スケジュール実行",
  "Have a prompt run by itself every morning, or every hour on weekdays, on the computer you choose.":
    "毎朝、平日は毎時など、決めたタイミングで指示を自動実行。動かすコンピュータも選べます。",

  "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen はベータ版です。暗号設計は社内のセキュリティレビューを受けていますが、外部監査はまだです。ブラウザで使う場合は、そのとき読み込んだページのコードを信頼することになります。そのコードは秘密鍵を使えるうえ、持ち出すこともできます。",

  "Invite-only beta": "招待制ベータ",
  "Public beta": "パブリックベータ",
  "Beta": "ベータ",
  "Your AI keeps working, wherever you are.": "AI は動き続ける。どこにいても続きから。",
  "Sign up with your invite code, connect your computer, and pick up from your phone.": "招待コードで登録し、コンピュータをつなげば、あとはスマホで続きから。",
  "Sign up with your e-mail, connect your computer, and pick up from your phone.": "メールアドレスで登録し、コンピュータをつなげば、あとはスマホで続きから。",
};

/** The landing page's tables, by i18next language. */
export const landingBundles: Record<"zh-CN" | "zh-TW" | "ja", Bundle> = {
  "zh-CN": { ...zhCN, ...downloadZhCN, ...pricingZhCN },
  "zh-TW": { ...zhTW, ...downloadZhTW, ...pricingZhTW },
  ja: { ...ja, ...downloadJa, ...pricingJa },
};
