/*
 * The landing page's words. As everywhere in the app (`src/i18n`), the keys
 * are the English text, so English needs no table. zh-CN is the page's first
 * language, written as native copy rather than translated from the English;
 * zh-TW follows the app's Taiwan wording (工作階段, 轉送, 資料, 裝置) and ja
 * its Japanese (直結, 中継経由, デバイス, 対応待ち).
 *
 * The voice is a product page's, not a manual's: every headline says what the
 * reader gets (their computer's AI on their phone, all their devices on one
 * screen, code nobody else can read), and the body under it gives the proof
 * and, briefly, the how. Droppen Hub, relays, NAT, STUN, ciphertext and p50s
 * come after the benefit, as its support, never as the opening; the hero
 * names none of them. "We" / "our servers" are Droppen Hub and the Droppen
 * relays. A device is a 设备 / 裝置 / デバイス; the copy says 你的电脑 only for
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
 * its 「需要你」 list in features/cloud/my-devices.tsx): numbers are the
 * measured p50s, and nothing says "unhackable", "audited" or "open source".
 */

import { downloadJa, downloadZhCN, downloadZhTW } from "./download-copy";
import { pricingJa, pricingZhCN, pricingZhTW } from "./pricing-copy";

type Bundle = Record<string, string>;

const zhCN: Bundle = {
  // Diagram cards: every row says something about the product.
  "your computer": "你的电脑",
  "your phone": "你的手机",
  "browser": "浏览器",
  "readable offline": "电脑关机也能读",
  "computer off · {{time}}": "电脑已关机 · {{time}}",
  "Droppen can't read it": "Droppen 读不了",
  "make it ink": "改成墨色",
  "done, tests pass": "改好了，测试通过",
  "Droppen relay": "Droppen 中继",
  "what Droppen sees": "Droppen 看到的",
  "relay": "中继",
  "encrypted copy": "加密副本",
  "read-only": "只读",
  "ready to merge?": "可以合并了吗？",
  "all tests pass": "测试全部通过",
  "encrypted locally": "本地加密",
  "decrypted locally": "本地解密",

  // Page
  "Droppen — your computer's AI, on your phone": "Droppen — 电脑上的 AI，手机上接着用",
  "Claude Code and Codex keep running on your own computer; from your phone you watch them, answer their questions and open a terminal. All your devices on one screen, end-to-end encrypted: we cannot see your code.":
    "Claude Code 和 Codex 照常在你自己的电脑上跑，你在手机上看进度、回答提问、开终端。所有设备一个屏，端到端加密：你的代码，我们看不到。",
  "On this page": "本页",
  "Privacy": "隐私",
  "Speed": "速度",
  "Offline": "离线",
  "Security": "安全",
  "Third-party licences": "第三方许可证",
  "How it works": "工作原理",
  "Open app": "打开应用",
  "Sign in": "登录",
  "Get started": "开始使用",

  // Hero
  "For Claude Code and Codex": "为 Claude Code 和 Codex 用户打造",
  "Your computer's AI, in your pocket.": "电脑上的 AI，手机上接着用。",
  "Claude Code and Codex keep running on your own computer. Walk away, and from your phone you still watch them work, answer their questions and open a terminal. Every device on one screen, end-to-end encrypted: we cannot see your code.":
    "Claude Code 和 Codex 照常在你自己的电脑上跑；人走开了，在手机上照样看它干活、回答它的提问、打开终端。所有设备一个屏，全程端到端加密：你的代码，我们看不到。",
  "Invite-only for now: sign-up needs an invite code.": "目前邀请制，注册需要邀请码。",
  "keystroke echo in a browser on the same computer, direct (measured p50)": "同一台电脑上用浏览器直连，按键回显（实测 p50）",
  "round trip from your phone on 4G, direct; 215–227 ms when it cannot connect directly (measured p50)": "手机 4G 直连电脑的往返；不能直连时 215–227 ms（实测 p50）",
  "keys on our servers that can open your code or your conversations": "我们服务器上能解开你代码和对话的钥匙",
  "recovery words, held only by you: we cannot recover them for you": "个恢复词，只在你手里，我们也找不回",

  // Your code, for your eyes only (a message's journey)
  "End-to-end encrypted": "端到端加密",
  "Your code, for your eyes only.": "你的代码，只有你看得到。",
  "From the agent on your computer to the phone in your hand, a message can be read at the two ends and nowhere in between: not on the network, and not on our servers.":
    "从你电脑上的 agent，到你手里的手机，消息只在两端读得出。中间的网络读不了，我们的服务器也读不了。",
  "Locked before it leaves your computer": "出电脑之前，先上锁",
  "The agent encrypts every message on your computer. The keys exist only on your computers and on the devices you approved.":
    "每条消息都在你的电脑上由 agent 加密。钥匙只在你的电脑和你批准过的设备上。",
  "All we see is noise": "我们只看到一串乱码",
  "The Droppen relay carrying it knows that about 1 KB went to one of your devices at 14:02. Not what it says, not the session's name, its folder or the model.":
    "负责转发的 Droppen 中继只知道 14:02 有约 1 KB 数据发往你的某台设备；说了什么、会话叫什么、在哪个目录、用的什么模型，它一概不知。",
  "Delivered, with an encrypted copy kept": "送到你手上，再留一份加密副本",
  "The Droppen relay passes it on to your device. The agent also leaves an encrypted copy with Droppen Hub, so you can still read the session when your computer is off.":
    "Droppen 中继把它转到你的设备上。agent 另外把一份加密副本存到 Droppen Hub，电脑关机后会话照样能看。",
  "Opened only in your hand": "只在你手里解开",
  "Your phone or browser decrypts it on the spot, with keys made in that browser that stay in it.":
    "手机或浏览器在本地解开它，用的钥匙在这个浏览器里生成，也只存在这个浏览器里。",

  // Typing from your phone stays snappy (the direct path)
  "Direct path": "直连",
  "Typing from your phone stays snappy.": "在手机上打字，照样跟手。",
  "Whenever your phone and your computer can reach each other, keystrokes go straight across instead of taking the detour through a Droppen relay: on the same computer or network, with IPv6 on both ends, or through a NAT that can be punched through. When they cannot, you do nothing: the Droppen relay is always there and takes over by itself.":
    "只要手机和电脑能互相连通，按键就直接过去，不再绕道 Droppen 中继——同一台电脑、同一个局域网、两端都有 IPv6，或者 NAT 能打洞，都行。连不通也不用你操心：Droppen 中继一直都在，自动接手。",
  "Just as private: the direct path runs its own end-to-end handshake, with the same encryption and the same admission rules as through a Droppen relay.":
    "直连一样私密：它会单独做一次端到端握手，加密方式和准入规则都和走 Droppen 中继时相同。",
  "No keystroke lost: if the direct path drops, traffic is back on the Droppen relay within 1.5 s, and unconfirmed keystrokes are sent again in order. None lost, doubled or reordered.":
    "一个按键都不丢：直连断了，1.5 秒内退回 Droppen 中继，还没确认的按键按原顺序重发——不丢、不重、不乱序。",
  "So far about two in three testers got a direct path; the rest go through a Droppen relay automatically.":
    "目前大约三分之二的测试者打通了直连，其余的自动走 Droppen 中继。",
  "Rather your other devices and the STUN servers did not learn your IP address? Turn off “Allow direct connections”.":
    "不想让自己的其他设备和 STUN 服务器知道你的 IP？关掉「允许直连」就行。",
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
  "Offline copy": "离线副本",
  "Lid closed? Your sessions are still there.": "电脑合上了，会话照样能看。",
  "While your computer is on, the agent encrypts what is new every few seconds and leaves it with Droppen Hub. Once the lid is closed, your phone fetches that encrypted copy from Droppen Hub and opens it on the spot.":
    "电脑开着的时候，agent 每隔几秒把新内容加密，存到 Droppen Hub。合上电脑以后，手机从 Droppen Hub 取回这份加密副本，在本地解开。",
  "Still only yours: Droppen Hub keeps session text only as encrypted pages, and its database holds metadata only.":
    "照样只有你看得到：Droppen Hub 只把会话正文存成加密的页，数据库里只有元数据。",
  "Read-only meanwhile: sending and terminals wait for your computer to come back.": "这期间只读：发消息和用终端要等电脑回来。",
  "Opens at once next time: each device keeps its own copy, cleared when you sign out.": "下次秒开：每台设备都留一份副本，退出登录时清除。",
  "What syncs: each computer's own sessions updated in the last 30 days.": "同步范围：每台电脑自己的、最近 30 天内有更新的会话。",

  // What you get
  "What you get": "功能",
  "Walk away from your desk. Everything stays within reach.": "人离开工位，一切照样在手边。",
  "Answer it from anywhere": "它一问，你随手就回",
  "Watch sessions as they run and see at a glance which one is asking you. Answer it, send a follow-up, or open a terminal.":
    "看着会话实时滚动，一眼看到谁在问你；回答它、追加一句，或者直接开终端。",
  "Your AI, your accounts": "还是你的 AI、你的账号",
  "Claude Code and Codex run on your own computer, with your own accounts, quotas and code, never on our servers. Add a few Claude accounts, and when one hits its limit a session can carry on with another by itself.":
    "Claude Code 和 Codex 跑在你自己的电脑上，用你自己的账号、额度和代码，从不在我们的服务器上跑。添加几个 Claude 账号，一个额度用完，会话能自动换到另一个接着跑。",
  "All your devices, one screen": "多台设备，一个屏",
  "The computers at home and at work, side by side on one account, and one list of what needs you across all of them: which session is asking, which has news, which is still working.":
    "家里和公司的电脑挂在同一个账户下并排看；所有设备上谁在问你、谁有新消息、谁还在干活，汇成一张「需要你」清单。",
  "Only devices you approve get in": "只有你批准的设备进得来",
  "A new phone or browser joins only when a device you already have approves it, by QR code or a one-time code. Twelve recovery words; no key escrow.":
    "新手机、新浏览器，都要由你已有的设备扫码或输入一次性口令批准才能加入。12 个恢复词，不托管任何钥匙。",
  "Lost a phone? Cut it off": "手机丢了？移除就切断",
  "Removing a device closes its connections and direct paths and rotates the keys: it cannot open anything written afterwards.":
    "移除一台设备，它的连接和直连随即关闭、钥匙随之轮换：之后写下的内容，它一个字也解不开。",
  "Direct or not: your call": "直不直连，你说了算",
  "One switch, “Allow direct connections”, decides: turn it off and your device and your computer never learn each other's IP address.":
    "一个开关「允许直连」说了算：关掉它，设备和电脑之间就不交换 IP 地址。",

  // What we can see and what we cannot
  "What we can see and what we cannot, line by line.": "我们能看到什么、看不到什么，一条条写清楚。",
  "Our servers come in two kinds. Droppen Hub only finds your computers and keeps their encrypted copies; Droppen relays only pass ciphertext on. Neither can read what goes through it. Here is exactly where that line is.":
    "我们的服务器有两种：Droppen Hub 只负责找到你的电脑、保管加密副本，Droppen 中继只负责转发密文。两者都读不了经手的内容。界线在哪，下面列得清清楚楚。",
  "What we can see": "我们能看到",
  "your e-mail address and when you sign in": "你的邮箱和登录时间",
  "which devices and computers are online, when they connect, and from which IP address": "哪些设备和电脑在线、何时连接、来自哪个 IP",
  "how much data moves and when: stored data is padded to fixed sizes, live connections are not": "传了多少数据、什么时候传的：保存的数据按档位填充，实时连接不填充",
  "so, roughly when you are working, and in a terminal the rhythm of your typing": "因此大致知道你什么时候在工作，以及终端里的打字节奏",
  "What we cannot see": "我们看不到",
  "what is said in a session, or typed and printed in a terminal": "会话里说了什么，终端里输入和输出了什么",
  "session titles, folders, repositories and branches": "会话标题、目录、仓库和分支",
  "model names, which CLI, your accounts and quotas": "模型名、用的哪个 CLI、你的账号和额度",
  "the names of your computers and devices, your files and their names": "电脑名和设备名、你的文件和文件名",
  "Who holds the keys": "钥匙在谁手里",
  "Device keys: made in each browser or phone, and kept only there": "设备钥匙：在每个浏览器或手机里生成，只存在那里",
  "A computer's identity key: on that computer only": "电脑的身份钥匙：只在那台电脑上",
  "Recovery words: with you only; neither Droppen Hub nor we can recover them for you": "恢复词：只在你手里，Droppen Hub 和我们都没法替你找回",
  "Us (Droppen Hub and relays): no key that can decrypt anything": "我们（Droppen Hub 和中继）：没有任何能解密的钥匙",
  "Droppen is an invite-only beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen 目前是邀请制测试版。加密设计经过了内部安全评审，还没有做过外部审计。在浏览器里使用时，你需要信任当次加载的页面代码：这份代码能使用你的私钥，也能把它们复制出去。",

  // Footer
  "Invite-only beta": "邀请制测试版",
  "Your AI keeps working. You can walk away.": "AI 接着干活，你可以走开了。",
  "Sign up with your invite code, connect your computer, and pick up from your phone.": "用邀请码注册，接上你的电脑，然后在手机上接着用。",
};

const zhTW: Bundle = {
  // Diagram cards: every row says something about the product.
  "your computer": "你的電腦",
  "your phone": "你的手機",
  "browser": "瀏覽器",
  "readable offline": "電腦關機也能讀",
  "computer off · {{time}}": "電腦已關機 · {{time}}",
  "Droppen can't read it": "Droppen 讀不了",
  "make it ink": "改成墨色",
  "done, tests pass": "改好了，測試通過",
  "Droppen relay": "Droppen 中繼",
  "what Droppen sees": "Droppen 看到的",
  "relay": "中繼",
  "encrypted copy": "加密副本",
  "read-only": "唯讀",
  "ready to merge?": "可以合併了嗎？",
  "all tests pass": "測試全部通過",
  "encrypted locally": "本機加密",
  "decrypted locally": "本機解密",

  "Droppen — your computer's AI, on your phone": "Droppen — 電腦上的 AI，手機上接著用",
  "Claude Code and Codex keep running on your own computer; from your phone you watch them, answer their questions and open a terminal. All your devices on one screen, end-to-end encrypted: we cannot see your code.":
    "Claude Code 和 Codex 照常在你自己的電腦上跑，你在手機上看進度、回答提問、開終端機。所有裝置一個畫面，端對端加密：你的程式碼，我們看不到。",
  "On this page": "本頁",
  "Privacy": "隱私",
  "Speed": "速度",
  "Offline": "離線",
  "Security": "安全",
  "Third-party licences": "第三方授權",
  "How it works": "運作方式",
  "Open app": "打開應用",
  "Sign in": "登入",
  "Get started": "開始使用",

  "For Claude Code and Codex": "為 Claude Code 和 Codex 使用者打造",
  "Your computer's AI, in your pocket.": "電腦上的 AI，手機上接著用。",
  "Claude Code and Codex keep running on your own computer. Walk away, and from your phone you still watch them work, answer their questions and open a terminal. Every device on one screen, end-to-end encrypted: we cannot see your code.":
    "Claude Code 和 Codex 照常在你自己的電腦上跑；人走開了，在手機上照樣看它工作、回答它的提問、開啟終端機。所有裝置一個畫面，全程端對端加密：你的程式碼，我們看不到。",
  "Invite-only for now: sign-up needs an invite code.": "目前採邀請制，註冊需要邀請碼。",
  "keystroke echo in a browser on the same computer, direct (measured p50)": "同一台電腦上用瀏覽器直連，按鍵回顯（實測 p50）",
  "round trip from your phone on 4G, direct; 215–227 ms when it cannot connect directly (measured p50)": "手機 4G 直連電腦的往返；無法直連時 215–227 ms（實測 p50）",
  "keys on our servers that can open your code or your conversations": "我們伺服器上能解開你程式碼和對話的金鑰",
  "recovery words, held only by you: we cannot recover them for you": "個復原詞，只在你手上，我們也找不回",

  "End-to-end encrypted": "端對端加密",
  "Your code, for your eyes only.": "你的程式碼，只有你看得到。",
  "From the agent on your computer to the phone in your hand, a message can be read at the two ends and nowhere in between: not on the network, and not on our servers.":
    "從你電腦上的 agent，到你手中的手機，訊息只在兩端讀得出。中間的網路讀不了，我們的伺服器也讀不了。",
  "Locked before it leaves your computer": "離開電腦之前，先上鎖",
  "The agent encrypts every message on your computer. The keys exist only on your computers and on the devices you approved.":
    "每則訊息都在你的電腦上由 agent 加密。金鑰只在你的電腦和你核准過的裝置上。",
  "All we see is noise": "我們只看到一串亂碼",
  "The Droppen relay carrying it knows that about 1 KB went to one of your devices at 14:02. Not what it says, not the session's name, its folder or the model.":
    "負責轉送的 Droppen 中繼只知道 14:02 有約 1 KB 資料送往你的某台裝置；說了什麼、工作階段叫什麼、在哪個資料夾、用哪個模型，它一概不知。",
  "Delivered, with an encrypted copy kept": "送到你手上，再留一份加密副本",
  "The Droppen relay passes it on to your device. The agent also leaves an encrypted copy with Droppen Hub, so you can still read the session when your computer is off.":
    "Droppen 中繼把它轉送到你的裝置。agent 另外把一份加密副本存到 Droppen Hub，電腦關機後工作階段照樣能看。",
  "Opened only in your hand": "只在你手上解開",
  "Your phone or browser decrypts it on the spot, with keys made in that browser that stay in it.":
    "手機或瀏覽器在本機解開它，用的金鑰在這個瀏覽器裡產生，也只存在這個瀏覽器裡。",

  "Direct path": "直連",
  "Typing from your phone stays snappy.": "在手機上打字，照樣跟手。",
  "Whenever your phone and your computer can reach each other, keystrokes go straight across instead of taking the detour through a Droppen relay: on the same computer or network, with IPv6 on both ends, or through a NAT that can be punched through. When they cannot, you do nothing: the Droppen relay is always there and takes over by itself.":
    "只要手機和電腦能互相連通，按鍵就直接過去，不再繞道 Droppen 中繼——同一台電腦、同一個區域網路、兩端都有 IPv6，或者 NAT 能打洞，都行。連不通也不用你操心：Droppen 中繼一直都在，自動接手。",
  "Just as private: the direct path runs its own end-to-end handshake, with the same encryption and the same admission rules as through a Droppen relay.":
    "直連一樣私密：它會另外做一次端對端交握，加密方式和准入規則都和走 Droppen 中繼時相同。",
  "No keystroke lost: if the direct path drops, traffic is back on the Droppen relay within 1.5 s, and unconfirmed keystrokes are sent again in order. None lost, doubled or reordered.":
    "一個按鍵都不丟：直連斷了，1.5 秒內退回 Droppen 中繼，還沒確認的按鍵依原順序重送——不丟、不重、不亂序。",
  "So far about two in three testers got a direct path; the rest go through a Droppen relay automatically.":
    "目前大約三分之二的測試者打通了直連，其餘的自動走 Droppen 中繼。",
  "Rather your other devices and the STUN servers did not learn your IP address? Turn off “Allow direct connections”.":
    "不想讓自己的其他裝置和 STUN 伺服器知道你的 IP？關閉「允許直連」就好。",
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

  "Offline copy": "離線副本",
  "Lid closed? Your sessions are still there.": "電腦闔上了，工作階段照樣能看。",
  "While your computer is on, the agent encrypts what is new every few seconds and leaves it with Droppen Hub. Once the lid is closed, your phone fetches that encrypted copy from Droppen Hub and opens it on the spot.":
    "電腦開著的時候，agent 每隔幾秒把新內容加密，存到 Droppen Hub。闔上電腦以後，手機從 Droppen Hub 取回這份加密副本，在本機解開。",
  "Still only yours: Droppen Hub keeps session text only as encrypted pages, and its database holds metadata only.":
    "照樣只有你看得到：Droppen Hub 只把工作階段內文存成加密的頁，資料庫裡只有中繼資料。",
  "Read-only meanwhile: sending and terminals wait for your computer to come back.": "這段期間唯讀：傳訊息和用終端機要等電腦回來。",
  "Opens at once next time: each device keeps its own copy, cleared when you sign out.": "下次秒開：每台裝置都留一份副本，登出時清除。",
  "What syncs: each computer's own sessions updated in the last 30 days.": "同步範圍：每台電腦自己的、最近 30 天內有更新的工作階段。",

  "What you get": "功能",
  "Walk away from your desk. Everything stays within reach.": "人離開座位，一切照樣在手邊。",
  "Answer it from anywhere": "它一問，你隨手就回",
  "Watch sessions as they run and see at a glance which one is asking you. Answer it, send a follow-up, or open a terminal.":
    "看著工作階段即時捲動，一眼看到誰在問你；回答它、追加一句，或者直接開終端機。",
  "Your AI, your accounts": "還是你的 AI、你的帳號",
  "Claude Code and Codex run on your own computer, with your own accounts, quotas and code, never on our servers. Add a few Claude accounts, and when one hits its limit a session can carry on with another by itself.":
    "Claude Code 和 Codex 跑在你自己的電腦上，用你自己的帳號、額度和程式碼，從不在我們的伺服器上跑。加入幾個 Claude 帳號，一個額度用完，工作階段能自動換到另一個接著跑。",
  "All your devices, one screen": "多台裝置，一個畫面",
  "The computers at home and at work, side by side on one account, and one list of what needs you across all of them: which session is asking, which has news, which is still working.":
    "家裡和公司的電腦掛在同一個帳戶下並排看；所有裝置上誰在問你、誰有新訊息、誰還在工作，匯成一張「需要你」清單。",
  "Only devices you approve get in": "只有你核准的裝置進得來",
  "A new phone or browser joins only when a device you already have approves it, by QR code or a one-time code. Twelve recovery words; no key escrow.":
    "新手機、新瀏覽器，都要由你已有的裝置掃碼或輸入一次性代碼核准才能加入。12 個復原詞，不託管任何金鑰。",
  "Lost a phone? Cut it off": "手機掉了？移除就切斷",
  "Removing a device closes its connections and direct paths and rotates the keys: it cannot open anything written afterwards.":
    "移除一台裝置，它的連線和直連隨即關閉、金鑰隨之輪替：之後寫下的內容，它一個字也解不開。",
  "Direct or not: your call": "直不直連，你說了算",
  "One switch, “Allow direct connections”, decides: turn it off and your device and your computer never learn each other's IP address.":
    "一個開關「允許直連」說了算：關掉它，裝置和電腦之間就不交換 IP 位址。",

  "What we can see and what we cannot, line by line.": "我們能看到什麼、看不到什麼，一條條寫清楚。",
  "Our servers come in two kinds. Droppen Hub only finds your computers and keeps their encrypted copies; Droppen relays only pass ciphertext on. Neither can read what goes through it. Here is exactly where that line is.":
    "我們的伺服器有兩種：Droppen Hub 只負責找到你的電腦、保管加密副本，Droppen 中繼只負責轉送密文。兩者都讀不了經手的內容。界線在哪，下面列得清清楚楚。",
  "What we can see": "我們能看到",
  "your e-mail address and when you sign in": "你的電子郵件和登入時間",
  "which devices and computers are online, when they connect, and from which IP address": "哪些裝置和電腦在線、何時連線、來自哪個 IP",
  "how much data moves and when: stored data is padded to fixed sizes, live connections are not": "傳了多少資料、什麼時候傳的：保存的資料依級距填充，即時連線不填充",
  "so, roughly when you are working, and in a terminal the rhythm of your typing": "因此大致知道你什麼時候在工作，以及終端機裡的打字節奏",
  "What we cannot see": "我們看不到",
  "what is said in a session, or typed and printed in a terminal": "工作階段裡說了什麼，終端機裡輸入和輸出了什麼",
  "session titles, folders, repositories and branches": "工作階段標題、資料夾、儲存庫和分支",
  "model names, which CLI, your accounts and quotas": "模型名稱、用的哪個 CLI、你的帳號和額度",
  "the names of your computers and devices, your files and their names": "電腦名稱和裝置名稱、你的檔案和檔名",
  "Who holds the keys": "金鑰在誰手上",
  "Device keys: made in each browser or phone, and kept only there": "裝置金鑰：在每個瀏覽器或手機裡產生，只存在那裡",
  "A computer's identity key: on that computer only": "電腦的身分金鑰：只在那台電腦上",
  "Recovery words: with you only; neither Droppen Hub nor we can recover them for you": "復原詞：只在你手上，Droppen Hub 和我們都沒辦法替你找回",
  "Us (Droppen Hub and relays): no key that can decrypt anything": "我們（Droppen Hub 和中繼）：沒有任何能解密的金鑰",
  "Droppen is an invite-only beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen 目前是邀請制測試版。加密設計經過內部安全審查，還沒有做過外部稽核。在瀏覽器裡使用時，你需要信任當次載入的頁面程式碼：這份程式碼能使用你的私密金鑰，也能把它們複製出去。",

  "Invite-only beta": "邀請制測試版",
  "Your AI keeps working. You can walk away.": "AI 繼續工作，你可以走開了。",
  "Sign up with your invite code, connect your computer, and pick up from your phone.": "用邀請碼註冊，接上你的電腦，然後在手機上接著用。",
};

const ja: Bundle = {
  // Diagram cards: every row says something about the product.
  "your computer": "あなたのコンピュータ",
  "your phone": "あなたのスマホ",
  "browser": "ブラウザ",
  "readable offline": "電源オフでも読める",
  "computer off · {{time}}": "電源オフ · {{time}}",
  "Droppen can't read it": "Droppen には読めない",
  "make it ink": "墨色にして",
  "done, tests pass": "完了、テスト通過",
  "Droppen relay": "Droppen 中継",
  "what Droppen sees": "Droppen に見えるもの",
  "relay": "中継",
  "encrypted copy": "暗号化コピー",
  "read-only": "読み取り専用",
  "ready to merge?": "マージしていい？",
  "all tests pass": "テスト全件通過",
  "encrypted locally": "ローカルで暗号化",
  "decrypted locally": "ローカルで復号",

  "Droppen — your computer's AI, on your phone": "Droppen — コンピュータの AI を、スマホで続きから",
  "Claude Code and Codex keep running on your own computer; from your phone you watch them, answer their questions and open a terminal. All your devices on one screen, end-to-end encrypted: we cannot see your code.":
    "Claude Code と Codex は自分のコンピュータで動かしたまま、スマホで様子を見て、質問に答え、ターミナルも開けます。すべてのデバイスをひとつの画面に。エンドツーエンド暗号化で、あなたのコードは私たちにも見えません。",
  "On this page": "このページ",
  "Privacy": "プライバシー",
  "Speed": "速さ",
  "Offline": "オフライン",
  "Security": "セキュリティ",
  "Third-party licences": "サードパーティのライセンス",
  "How it works": "しくみ",
  "Open app": "アプリを開く",
  "Sign in": "ログイン",
  "Get started": "はじめる",

  "For Claude Code and Codex": "Claude Code と Codex のユーザーへ",
  "Your computer's AI, in your pocket.": "コンピュータの AI を、スマホで続きから。",
  "Claude Code and Codex keep running on your own computer. Walk away, and from your phone you still watch them work, answer their questions and open a terminal. Every device on one screen, end-to-end encrypted: we cannot see your code.":
    "Claude Code と Codex は自分のコンピュータで動かしたまま。席を離れても、スマホで様子を見て、質問に答え、ターミナルも開けます。すべてのデバイスをひとつの画面に。エンドツーエンド暗号化で、あなたのコードは私たちにも見えません。",
  "Invite-only for now: sign-up needs an invite code.": "いまは招待制です。登録には招待コードが必要です。",
  "keystroke echo in a browser on the same computer, direct (measured p50)": "同じコンピュータのブラウザで直結したときのキー入力エコー（実測 p50）",
  "round trip from your phone on 4G, direct; 215–227 ms when it cannot connect directly (measured p50)": "4G のスマホから直結したときの往復。直結できないときは 215–227 ms（実測 p50）",
  "keys on our servers that can open your code or your conversations": "私たちのサーバーにある、あなたのコードや会話を開ける鍵の数",
  "recovery words, held only by you: we cannot recover them for you": "語のリカバリーワード。持つのはあなただけで、私たちにも復元できません",

  "End-to-end encrypted": "エンドツーエンド暗号化",
  "Your code, for your eyes only.": "あなたのコードは、あなたにしか見えない。",
  "From the agent on your computer to the phone in your hand, a message can be read at the two ends and nowhere in between: not on the network, and not on our servers.":
    "コンピュータ上の agent から手元のスマホまで、メッセージが読めるのは両端だけ。途中のネットワークにも、私たちのサーバーにも読めません。",
  "Locked before it leaves your computer": "コンピュータを出る前に、鍵をかける",
  "The agent encrypts every message on your computer. The keys exist only on your computers and on the devices you approved.":
    "メッセージはすべて、コンピュータの上で agent が暗号化します。鍵があるのは、あなたのコンピュータと、あなたが承認したデバイスだけです。",
  "All we see is noise": "私たちに見えるのは、ノイズだけ",
  "The Droppen relay carrying it knows that about 1 KB went to one of your devices at 14:02. Not what it says, not the session's name, its folder or the model.":
    "運ぶ Droppen 中継にわかるのは、14:02 に約 1 KB があなたのどれかのデバイスへ送られたことだけ。内容も、セッション名も、フォルダも、モデルもわかりません。",
  "Delivered, with an encrypted copy kept": "届けて、暗号化コピーも残す",
  "The Droppen relay passes it on to your device. The agent also leaves an encrypted copy with Droppen Hub, so you can still read the session when your computer is off.":
    "Droppen 中継がデバイスへ渡します。agent は暗号化したコピーを Droppen Hub にも預けるので、コンピュータの電源が切れてもセッションを読めます。",
  "Opened only in your hand": "開けるのは、あなたの手元だけ",
  "Your phone or browser decrypts it on the spot, with keys made in that browser that stay in it.":
    "スマホやブラウザが手元で開きます。鍵はそのブラウザの中で作られ、そのブラウザの中にだけあります。",

  "Direct path": "直結",
  "Typing from your phone stays snappy.": "スマホからの入力も、もたつかない。",
  "Whenever your phone and your computer can reach each other, keystrokes go straight across instead of taking the detour through a Droppen relay: on the same computer or network, with IPv6 on both ends, or through a NAT that can be punched through. When they cannot, you do nothing: the Droppen relay is always there and takes over by itself.":
    "スマホとコンピュータが互いに届くなら、キー入力は Droppen 中継を回り道せず、まっすぐ届きます。同じコンピュータ、同じネットワーク、両端に IPv6、あるいはホールパンチングできる NAT なら直結できます。届かないときも何もしなくて大丈夫。Droppen 中継がいつも控えていて、自動で引き継ぎます。",
  "Just as private: the direct path runs its own end-to-end handshake, with the same encryption and the same admission rules as through a Droppen relay.":
    "直結でも同じように守られます。直結の上でもう一度エンドツーエンドのハンドシェイクを行い、暗号化も接続の許可ルールも Droppen 中継経由と同じです。",
  "No keystroke lost: if the direct path drops, traffic is back on the Droppen relay within 1.5 s, and unconfirmed keystrokes are sent again in order. None lost, doubled or reordered.":
    "キー入力はひとつも失われません。直結が切れると 1.5 秒以内に Droppen 中継へ戻り、未確認のキー入力を順番どおりに再送します。欠けず、重複せず、順序も崩れません。",
  "So far about two in three testers got a direct path; the rest go through a Droppen relay automatically.":
    "これまでのところ、テスターの約 3 人に 2 人が直結でき、残りは自動で Droppen 中継を経由しています。",
  "Rather your other devices and the STUN servers did not learn your IP address? Turn off “Allow direct connections”.":
    "自分の他のデバイスや STUN サーバーに IP アドレスを知られたくなければ、「直接接続を許可」をオフにするだけです。",
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

  "Offline copy": "オフラインコピー",
  "Lid closed? Your sessions are still there.": "ふたを閉じても、セッションは読める。",
  "While your computer is on, the agent encrypts what is new every few seconds and leaves it with Droppen Hub. Once the lid is closed, your phone fetches that encrypted copy from Droppen Hub and opens it on the spot.":
    "コンピュータが動いている間、agent は数秒ごとに新しい内容を暗号化して Droppen Hub に預けます。ふたを閉じたあとは、スマホが Droppen Hub からその暗号化コピーを取り寄せ、手元で開きます。",
  "Still only yours: Droppen Hub keeps session text only as encrypted pages, and its database holds metadata only.":
    "読めるのはやはりあなただけ。Droppen Hub はセッション本文を暗号化したページとしてしか保管せず、データベースにあるのはメタデータだけです。",
  "Read-only meanwhile: sending and terminals wait for your computer to come back.": "その間は読み取り専用。送信とターミナルはコンピュータの復帰を待ちます。",
  "Opens at once next time: each device keeps its own copy, cleared when you sign out.": "次はすぐ開けます。各デバイスにもコピーを残し、ログアウトすると消去します。",
  "What syncs: each computer's own sessions updated in the last 30 days.": "同期の対象：各コンピュータ自身のセッションのうち、過去 30 日に更新されたもの。",

  "What you get": "できること",
  "Walk away from your desk. Everything stays within reach.": "席を離れても、すべて手の届くところに。",
  "Answer it from anywhere": "どこからでも、すぐ答える",
  "Watch sessions as they run and see at a glance which one is asking you. Answer it, send a follow-up, or open a terminal.":
    "動いているセッションを見て、どれが質問しているかひと目でわかる。答えても、ひとこと足しても、ターミナルを開いてもいい。",
  "Your AI, your accounts": "いつもの AI、いつものアカウント",
  "Claude Code and Codex run on your own computer, with your own accounts, quotas and code, never on our servers. Add a few Claude accounts, and when one hits its limit a session can carry on with another by itself.":
    "Claude Code と Codex はあなたのコンピュータで、あなたのアカウント・使用枠・コードのまま動き、私たちのサーバーで動くことはありません。Claude のアカウントを複数登録すれば、ひとつが上限に達しても、セッションは別のアカウントで自動的に続けられます。",
  "All your devices, one screen": "すべてのデバイスを、ひとつの画面で",
  "The computers at home and at work, side by side on one account, and one list of what needs you across all of them: which session is asking, which has news, which is still working.":
    "自宅と職場のコンピュータをひとつのアカウントに並べて表示。質問中、新着あり、作業中のセッションは、すべてのデバイス分が「対応待ち」のリストにまとまります。",
  "Only devices you approve get in": "入れるのは、承認したデバイスだけ",
  "A new phone or browser joins only when a device you already have approves it, by QR code or a one-time code. Twelve recovery words; no key escrow.":
    "新しいスマホやブラウザは、手持ちのデバイスが QR コードかワンタイムコードで承認したときだけ加われます。リカバリーワードは 12 語、鍵の預託はありません。",
  "Lost a phone? Cut it off": "スマホをなくしても、外せば切れる",
  "Removing a device closes its connections and direct paths and rotates the keys: it cannot open anything written afterwards.":
    "デバイスを外すと、その接続と直結はすぐ閉じ、鍵も更新されます。その後に書かれたものは開けません。",
  "Direct or not: your call": "直結するかは、あなたが決める",
  "One switch, “Allow direct connections”, decides: turn it off and your device and your computer never learn each other's IP address.":
    "スイッチひとつ「直接接続を許可」で決まります。オフにすれば、デバイスとコンピュータが互いの IP アドレスを知ることはありません。",

  "What we can see and what we cannot, line by line.": "見えるもの、見えないもの。すべて書き出します。",
  "Our servers come in two kinds. Droppen Hub only finds your computers and keeps their encrypted copies; Droppen relays only pass ciphertext on. Neither can read what goes through it. Here is exactly where that line is.":
    "私たちのサーバーは 2 種類です。Droppen Hub はあなたのコンピュータを見つけ、暗号化コピーを預かるだけ。Droppen 中継は暗号文を渡すだけ。どちらも、通る中身は読めません。その境界を、正確に書きます。",
  "What we can see": "私たちに見えるもの",
  "your e-mail address and when you sign in": "メールアドレスとログインの時刻",
  "which devices and computers are online, when they connect, and from which IP address": "どのデバイスとコンピュータがオンラインか、いつ、どの IP アドレスから接続したか",
  "how much data moves and when: stored data is padded to fixed sizes, live connections are not": "どれだけのデータがいつ流れたか（保存するデータは決まったサイズまでパディングし、ライブの接続はしない）",
  "so, roughly when you are working, and in a terminal the rhythm of your typing": "つまり、あなたがだいたいいつ作業しているか、ターミナルでの打鍵のリズムも",
  "What we cannot see": "私たちに見えないもの",
  "what is said in a session, or typed and printed in a terminal": "セッションで話された内容、ターミナルの入力と出力",
  "session titles, folders, repositories and branches": "セッションのタイトル、フォルダ、リポジトリ、ブランチ",
  "model names, which CLI, your accounts and quotas": "モデル名、どの CLI か、アカウントと使用枠",
  "the names of your computers and devices, your files and their names": "コンピュータ名とデバイス名、ファイルとファイル名",
  "Who holds the keys": "鍵を持っているのは誰か",
  "Device keys: made in each browser or phone, and kept only there": "デバイスの鍵：各ブラウザやスマートフォンの中で作られ、そこにだけある",
  "A computer's identity key: on that computer only": "コンピュータの識別鍵：そのコンピュータの中だけ",
  "Recovery words: with you only; neither Droppen Hub nor we can recover them for you": "リカバリーワード：あなたの手元だけ。Droppen Hub にも私たちにも復元できません",
  "Us (Droppen Hub and relays): no key that can decrypt anything": "私たち（Droppen Hub と中継）：復号できる鍵は何ひとつ持たない",
  "Droppen is an invite-only beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen は招待制ベータです。暗号設計は社内のセキュリティレビューを受けていますが、外部監査はまだです。ブラウザで使う場合は、そのとき読み込んだページのコードを信頼することになります。そのコードは秘密鍵を使えるうえ、持ち出すこともできます。",

  "Invite-only beta": "招待制ベータ",
  "Your AI keeps working. You can walk away.": "AI は動かしたまま、席を離れよう。",
  "Sign up with your invite code, connect your computer, and pick up from your phone.": "招待コードで登録し、コンピュータをつなげば、あとはスマホで続きから。",
};

/** The landing page's tables, by i18next language. */
export const landingBundles: Record<"zh-CN" | "zh-TW" | "ja", Bundle> = {
  "zh-CN": { ...zhCN, ...downloadZhCN, ...pricingZhCN },
  "zh-TW": { ...zhTW, ...downloadZhTW, ...pricingZhTW },
  ja: { ...ja, ...downloadJa, ...pricingJa },
};
