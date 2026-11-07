
import { registerCopy } from "./register-copy";
import { downloadJa, downloadZhCN, downloadZhTW } from "./download-copy";
import { pricingJa, pricingZhCN, pricingZhTW } from "./pricing-copy";

type Bundle = Record<string, string>;

const zhCN: Bundle = {
  "Droppen relay": "Droppen 中继",
  "Phone": "手机",
  "relay": "中继",
  "encrypted copy": "加密副本",

  "Linux server": "Linux 服务器",

  "Droppen — your computer's Agent, on your phone": "Droppen — 电脑上的 Agent，在手机上接着使用",
  "Claude Code and Codex keep running on your Mac and Linux machines. Monitor sessions, approve tool permissions, answer prompts, and open terminals from any mobile browser. End-to-end encrypted, with an open-source client.":
    "Claude Code 和 Codex 保持在本地 Mac 与 Linux 运行。在手机或浏览器中随时汇总多机状态、实时审批权限、查阅输出并打开终端。端到端加密，客户端开源。",
  "On this page": "本页",
  "Site navigation": "网站导航",
  "Security": "安全",
  "Client source code": "客户端源代码",
  "Third-party notices": "第三方软件声明",
  "How it works": "工作原理",
  "Open app": "打开应用",
  "Sign in": "登录",

  "Your computer's Agent, on your phone.": "电脑上的 Agent，在手机上接着使用",
  "Claude Code and Codex keep running on your Mac and Linux machines. Monitor progress, approve tool permissions, and send instructions from any phone browser.":
    "Claude Code 和 Codex 仍在你自己的电脑上运行。无需守在工位，在手机浏览器中实时审批权限、查阅输出并派发指令。",
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

  "End-to-end encrypted": "端到端加密",
  "Security and encryption": "安全与端到端加密",
  "Encrypted on your computer and decrypted only on your devices. Droppen Hub stores only encrypted copies and Droppen relays forward only encrypted data; neither holds a key that can decrypt them.":
    "内容在你的电脑上加密，只在你的设备上解密。Droppen Hub 保存的只是加密副本，Droppen 中继转发的只是加密数据，两者都没有解密所需的密钥。",
  "Only devices you approve can join": "只有你批准的设备才能加入",
  "A new phone or browser can join only when an existing authorized device approves it. If a device is lost, remove it remotely: removed devices cannot decrypt subsequent data.":
    "新手机或新浏览器须经已授权设备批准后方可加入。若设备丢失，可从其他设备远程移除：已移除设备无法解密后续会话。",
  "Open-source client": "客户端开源",
  "How the encryption works": "加密实现原理",
  "The droppen command-line program, the Mac app and the web app are open source under AGPL-3.0. Droppen Hub is not open source.":
    "droppen 命令行程序、Mac 应用和网页应用都是开源的，许可证是 AGPL-3.0。Droppen Hub 不开源。",
  "Read the source": "查看源代码",

  "direct": "直连",

  "How it connects": "连接方式",
  "Direct · {{ms}}": "直连 · {{ms}}",
  "Relay · {{ms}}": "中继 · {{ms}}",
  "Switches within 1.5 s": "1.5 秒内自动切换",
  "Keys you type are sent again in order": "输入的按键会按原顺序重发",
  "Switching · 1.5 s": "切换 · 1.5 秒",

  "Every session in one list": "所有会话，一个列表",
  "Aggregate Claude Code, Codex, and ACP agent sessions across all computers. Active, waiting, and unread tasks are prioritized in a single view.":
    "单账号聚合所有电脑上的 Claude Code、Codex 及各类 Agent。正在运行、等待确认与未读消息按优先级统一呈现。",
  "Answer questions and approve actions": "实时审批与交互",
  "Approve Bash commands and file edits, answer agent questions, add instructions during execution, or abort tasks immediately.":
    "在手机上直接审批 Bash 命令与文件修改，回答 AI 提问。任务执行中可随时追加指令或立即终止会话。",
  "Start a session from your phone": "在手机上新建会话",
  "Select Claude Code or Codex, choose a target computer and folder, attach screenshots, and dispatch new tasks directly from your phone.":
    "在手机上选择 Claude Code 或 Codex、目标电脑和工作文件夹，支持附带截图直接派发新任务。",
  "Browse files and terminals": "浏览文件与终端",
  "Browse project directories on your remote computers, inspect code diffs, and open native host terminals directly from your mobile browser.":
    "在手机上查阅远程电脑的项目目录、检查代码修改，并能直接打开主机原生终端执行命令。",
  "Scheduled tasks": "定时任务",
  "Configure recurring schedules on target computers to run automated agent workflows, tests, or daily maintenance automatically.":
    "在指定的电脑上配置计划任务，按设定的频率自动唤起 Agent 执行日常巡检、构建或自动化流程。",

  "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen 目前是测试版。加密设计经过了内部安全评审，还没有做过外部审计。在浏览器里使用时，你需要信任当次加载的页面代码：这份代码能使用你的私钥，也能把它们复制出去。",

  "Your AI keeps working on your computer. When it needs you, take control from your phone.": "让本地 AI 专注编码，需要你时，手机随时响应接管。",

  "Free plan": "免费版",
  "Product facts": "产品信息",
  "Choose a screen": "选择界面",
  "Screen {{n}} of {{total}}": "第 {{n}} 个界面，共 {{total}} 个",
  "Screen {{n}} of {{total}}: {{title}}": "第 {{n}} 个界面，共 {{total}} 个：{{title}}",
  "All computers, one list.": "所有电脑，一个列表。",
  "Connect multiple Mac and Linux computers under one account. View all Claude Code and Codex sessions in real time and see immediately which task requires your input.":
    "将多台 Mac 与 Linux 电脑接入同一个账户。会话状态实时汇总，一目了然定位正在运行或等待确认的任务。",
  "Code on the left, terminal on the right.": "左边代码，右边终端。",
  "Run an AI session and native terminal side by side. Inspect code changes, test logs, and command outputs in a single window without switching applications.":
    "AI 会话与主机原生终端同屏并排运行。实时查看代码改动、测试日志与命令输出，无需在窗口间频繁切换。",
  "Leave the computer, take over on your phone.": "离开电脑，在手机上接手。",
  "Open {{app}} in any mobile browser to sync sessions instantly. Tasks awaiting confirmation are pinned to the top for quick approval and replies.":
    "在手机浏览器打开 {{app}} 即时同步会话。「需要你」待处理任务优先置顶，随时随地审批命令、回复提问或派发新任务。",
  "What you can do from your phone.": "在手机上能做什么",
  "Read offline": "离线也能读",
  "When your computer is online, an encrypted copy syncs to your phone. If it shuts down or disconnects, you can still read the last {{free}} days ({{pro}} on Pro).":
    "电脑在线时自动同步加密副本至手机。即使电脑关机、休眠或离线，仍可查看最近 {{free}} 天（Pro 为 {{pro}} 天）的历史会话。",
  "Direct connection with automatic relay fallback.": "直连优先，网络受限时自动中继",
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
  "No. The AI runs on your computer, but you do not need to stay at your desk. While it is offline, your phone can still read the last {{free}} days ({{pro}} on Pro) of synced sessions. Sending prompts and using the terminal resume once it reconnects.":
    "不用。AI 在你的电脑上运行，但启动后无需守在电脑前。电脑离线时，手机仍可查阅最近 {{free}} 天（Pro 为 {{pro}} 天）已同步的历史会话；发送新指令和终端交互将在电脑重新联网后继续。",
  "Is there a Windows client?": "有 Windows 客户端吗？",
  "Droppen's web app runs in any Windows browser, but the AI must run on a Mac, Linux, or WSL 2 machine. A native Windows host client is planned.":
    "Droppen 网页端可在 Windows 浏览器中完整使用，但运行 AI 的电脑须为 Mac、Linux 或 WSL 2 环境。Windows 原生宿主客户端仍在规划中。",
  "Will my code be uploaded?": "我的代码会上传吗？",
  "Session content is encrypted on your computer before transmission. Droppen Hub stores only encrypted copies, and relays forward only encrypted data; neither holds decryption keys. In a browser, you trust the page code loaded during that session.":
    "会话内容在离开你的电脑前已完成端到端加密。Droppen Hub 仅保存加密副本，中继仅转发加密流量，两者均无解密密钥。在浏览器中使用时，需信任当次加载的应用代码。",
  "Is the free plan enough?": "免费版够用吗？",
  "If you have up to {{hosts}} computers that run AI and need {{free}} days of offline access, the free plan is sufficient. Upgrade to Pro for more devices, {{pro}} days of offline access, or additional relay bandwidth.":
    "若管理不超过 {{hosts}} 台运行 AI 的电脑，且仅需 {{free}} 天离线历史查看，免费版完全够用。若需连接更多设备、长达 {{pro}} 天离线访问或更高流量限额，可随时升级 Pro 方案。",
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
  "Droppen relay": "Droppen 中繼",
  "Phone": "手機",
  "relay": "中繼",
  "encrypted copy": "加密副本",

  "Linux server": "Linux 伺服器",

  "Droppen — your computer's Agent, on your phone": "Droppen — 電腦上的 Agent，在手機上接著使用",
  "Claude Code and Codex keep running on your Mac and Linux machines. Monitor sessions, approve tool permissions, answer prompts, and open terminals from any mobile browser. End-to-end encrypted, with an open-source client.":
    "Claude Code 和 Codex 保持在本地 Mac 與 Linux 執行。在手機或瀏覽器中隨時彙整多機狀態、即時審批權限、查閱輸出並開啟終端機。端對端加密，客戶端開源。",
  "On this page": "本頁",
  "Site navigation": "網站導覽",
  "Security": "安全",
  "Client source code": "客戶端原始碼",
  "Third-party notices": "第三方軟體聲明",
  "How it works": "運作方式",
  "Open app": "打開應用",
  "Sign in": "登入",

  "Your computer's Agent, on your phone.": "電腦上的 Agent，在手機上接著使用",
  "Claude Code and Codex keep running on your Mac and Linux machines. Monitor progress, approve tool permissions, and send instructions from any phone browser.":
    "Claude Code 和 Codex 仍在你自己的電腦上執行。無需守在工位，在手機瀏覽器中即時審批權限、查閱輸出並派發指令。",
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

  "End-to-end encrypted": "端對端加密",
  "Security and encryption": "安全與端對端加密",
  "Encrypted on your computer and decrypted only on your devices. Droppen Hub stores only encrypted copies and Droppen relays forward only encrypted data; neither holds a key that can decrypt them.":
    "內容在你的電腦上加密，只在你的裝置上解密。Droppen Hub 儲存的只是加密副本，Droppen 中繼轉送的只是加密資料，兩者都沒有解密所需的金鑰。",
  "Only devices you approve can join": "只有你核准的裝置才能加入",
  "A new phone or browser can join only when an existing authorized device approves it. If a device is lost, remove it remotely: removed devices cannot decrypt subsequent data.":
    "新手機或新瀏覽器須經已授權裝置核准後方可加入。若裝置遺失，可從其他裝置遠端移除：已移除裝置無法解密後續工作階段。",
  "Open-source client": "客戶端開源",
  "How the encryption works": "加密實現原理",
  "The droppen command-line program, the Mac app and the web app are open source under AGPL-3.0. Droppen Hub is not open source.":
    "droppen 命令列程式、Mac 應用程式和網頁應用程式都是開源的，授權條款是 AGPL-3.0。Droppen Hub 不開源。",
  "Read the source": "查看原始碼",

  "direct": "直連",

  "How it connects": "連線方式",
  "Direct · {{ms}}": "直連 · {{ms}}",
  "Relay · {{ms}}": "中繼 · {{ms}}",
  "Switches within 1.5 s": "1.5 秒內自動切換",
  "Keys you type are sent again in order": "輸入的按鍵會按原順序重送",
  "Switching · 1.5 s": "切換 · 1.5 秒",
  "Every session in one list": "所有工作階段，一個清單",
  "Aggregate Claude Code, Codex, and ACP agent sessions across all computers. Active, waiting, and unread tasks are prioritized in a single view.":
    "單一帳戶聚合所有電腦上的 Claude Code、Codex 及各類 Agent。執行中、等待確認與未讀訊息按優先級統一呈現。",
  "Answer questions and approve actions": "即時審批與互動",
  "Approve Bash commands and file edits, answer agent questions, add instructions during execution, or abort tasks immediately.":
    "在手機上直接審批 Bash 指令與檔案修改，回覆 AI 提問。任務執行中可隨時補充指令或立即終止工作階段。",
  "Start a session from your phone": "在手機上新增工作階段",
  "Select Claude Code or Codex, choose a target computer and folder, attach screenshots, and dispatch new tasks directly from your phone.":
    "在手機上選擇 Claude Code 或 Codex、目標電腦和工作資料夾，支援附加螢幕截圖直接派發新任務。",
  "Browse files and terminals": "遠端檔案與終端機",
  "Browse project directories on your remote computers, inspect code diffs, and open native host terminals directly from your mobile browser.":
    "在手機上瀏覽遠端電腦的專案資料夾、查閱程式碼變更，並能直接開啟主機原生終端機執行指令。",
  "Scheduled tasks": "排程任務",
  "Configure recurring schedules on target computers to run automated agent workflows, tests, or daily maintenance automatically.":
    "在指定的電腦上設定排程任務，依設定頻率自動喚起 Agent 執行日常巡檢、建置或自動化流程。",

  "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen 目前是測試版。加密設計經過內部安全審查，還沒有做過外部稽核。在瀏覽器裡使用時，你需要信任當次載入的頁面程式碼：這份程式碼能使用你的私密金鑰，也能把它們複製出去。",

  "Your AI keeps working on your computer. When it needs you, take control from your phone.": "讓本地 AI 專注編碼，需要你時，手機隨時響應接管。",

  "Free plan": "免費版",
  "Product facts": "產品資訊",
  "Choose a screen": "選擇介面",
  "Screen {{n}} of {{total}}": "第 {{n}} 個介面，共 {{total}} 個",
  "Screen {{n}} of {{total}}: {{title}}": "第 {{n}} 個介面，共 {{total}} 個：{{title}}",
  "All computers, one list.": "所有電腦，一個清單。",
  "Connect multiple Mac and Linux computers under one account. View all Claude Code and Codex sessions in real time and see immediately which task requires your input.":
    "將多台 Mac 與 Linux 電腦接入同一個帳戶。工作階段狀態即時彙整，一目了然定位執行中或等待確認的任務。",
  "Code on the left, terminal on the right.": "左邊程式碼，右邊終端機。",
  "Run an AI session and native terminal side by side. Inspect code changes, test logs, and command outputs in a single window without switching applications.":
    "AI 工作階段與主機原生終端機同屏並排執行。即時查看程式碼變更、測試記錄與指令輸出，無需在視窗間頻繁切換。",
  "Leave the computer, take over on your phone.": "離開電腦，在手機上接手。",
  "Open {{app}} in any mobile browser to sync sessions instantly. Tasks awaiting confirmation are pinned to the top for quick approval and replies.":
    "在手機瀏覽器開啟 {{app}} 即時同步工作階段。「需要你」待處理任務優先置頂，隨時隨地審批指令、回覆提問或派發新任務。",
  "What you can do from your phone.": "在手機上能做什麼",
  "Read offline": "離線也能讀",
  "When your computer is online, an encrypted copy syncs to your phone. If it shuts down or disconnects, you can still read the last {{free}} days ({{pro}} on Pro).":
    "電腦在線時自動同步加密副本至手機。即使電腦關機、休眠或離線，仍可查看最近 {{free}} 天（Pro 為 {{pro}} 天）的歷史工作階段。",
  "Direct connection with automatic relay fallback.": "直連優先，網路受限時自動中繼",
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
  "No. The AI runs on your computer, but you do not need to stay at your desk. While it is offline, your phone can still read the last {{free}} days ({{pro}} on Pro) of synced sessions. Sending prompts and using the terminal resume once it reconnects.":
    "不用。AI 在你的電腦上執行，但啟動後無需守在電腦前。電腦離線時，手機仍可查閱最近 {{free}} 天（Pro 為 {{pro}} 天）已同步的歷史工作階段；傳送新指令與終端機互動將在電腦重新聯網後繼續。",
  "Is there a Windows client?": "有 Windows 客戶端嗎？",
  "Droppen's web app runs in any Windows browser, but the AI must run on a Mac, Linux, or WSL 2 machine. A native Windows host client is planned.":
    "Droppen 網頁端可在 Windows 瀏覽器中完整使用，但執行 AI 的電腦須為 Mac、Linux 或 WSL 2 環境。Windows 原生宿主客戶端仍在規劃中。",
  "Will my code be uploaded?": "我的程式碼會上傳嗎？",
  "Session content is encrypted on your computer before transmission. Droppen Hub stores only encrypted copies, and relays forward only encrypted data; neither holds decryption keys. In a browser, you trust the page code loaded during that session.":
    "工作階段內容在離開你的電腦前已完成端對端加密。Droppen Hub 僅儲存加密副本，中繼僅轉送加密流量，兩者均無解密金鑰。在瀏覽器中使用時，需信任當次載入的應用程式碼。",
  "Is the free plan enough?": "免費版夠用嗎？",
  "If you have up to {{hosts}} computers that run AI and need {{free}} days of offline access, the free plan is sufficient. Upgrade to Pro for more devices, {{pro}} days of offline access, or additional relay bandwidth.":
    "若管理不超過 {{hosts}} 台執行 AI 的電腦，且僅需 {{free}} 天離線歷史查看，免費版完全夠用。若需連接更多裝置、長達 {{pro}} 天離線存取或更高流量配額，可隨時升級 Pro 方案。",
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
  "Droppen relay": "Droppen 中継",
  "Phone": "スマートフォン",
  "relay": "中継",
  "encrypted copy": "暗号化コピー",

  "Linux server": "Linux サーバー",

  "Droppen — your computer's Agent, on your phone": "Droppen — コンピュータの Agent を、スマホで続きから",
  "Claude Code and Codex keep running on your Mac and Linux machines. Monitor sessions, approve tool permissions, answer prompts, and open terminals from any mobile browser. End-to-end encrypted, with an open-source client.":
    "Claude Code や Codex をローカルの Mac/Linux で実行したまま、スマホやブラウザから複数デバイスを一括管理。権限承認、返答、指示送信、ターミナル操作を安全に行えます。端対端暗号化、オープンソース。",
  "On this page": "このページ",
  "Site navigation": "サイトナビゲーション",
  "Security": "セキュリティ",
  "Client source code": "クライアントのソースコード",
  "Third-party notices": "サードパーティ通知",
  "How it works": "しくみ",
  "Open app": "アプリを開く",
  "Sign in": "ログイン",

  "Your computer's Agent, on your phone.": "コンピュータの Agent を、スマホで続きから。",
  "Claude Code and Codex keep running on your Mac and Linux machines. Monitor progress, approve tool permissions, and send instructions from any phone browser.":
    "Claude Code や Codex は手元のコンピュータで動作。デスクを離れても、スマホのブラウザから権限承認、状況確認、指示送信が可能です。",
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

  "End-to-end encrypted": "エンドツーエンド暗号化",
  "Security and encryption": "セキュリティとエンドツーエンド暗号化",
  "Encrypted on your computer and decrypted only on your devices. Droppen Hub stores only encrypted copies and Droppen relays forward only encrypted data; neither holds a key that can decrypt them.":
    "暗号化はあなたのコンピュータで、復号はあなたのデバイスでだけ。Droppen Hub が保存するのは暗号化コピーだけ、Droppen 中継が転送するのは暗号化データだけで、どちらも復号に必要な鍵を持っていません。",
  "Only devices you approve can join": "加われるのは、承認したデバイスだけ",
  "A new phone or browser can join only when an existing authorized device approves it. If a device is lost, remove it remotely: removed devices cannot decrypt subsequent data.":
    "新しいスマホやブラウザの参加には、既存の承認済みデバイスによる承認が必要です。紛失時は遠隔で削除でき、削除されたデバイスは以後のデータを復号できません。",
  "Open-source client": "クライアントはオープンソース",
  "How the encryption works": "暗号化のしくみ",
  "The droppen command-line program, the Mac app and the web app are open source under AGPL-3.0. Droppen Hub is not open source.":
    "droppen コマンドラインプログラム、Mac アプリ、Web アプリは AGPL-3.0 のオープンソースです。Droppen Hub はオープンソースではありません。",
  "Read the source": "ソースを見る",

  "direct": "直結",

  "How it connects": "接続方法",
  "Direct · {{ms}}": "直結 · {{ms}}",
  "Relay · {{ms}}": "中継 · {{ms}}",
  "Switches within 1.5 s": "1.5 秒以内に自動で切り替え",
  "Keys you type are sent again in order": "入力したキーは順番どおりに再送されます",
  "Switching · 1.5 s": "切り替え · 1.5 秒",
  "Every session in one list": "すべてのセッションを、ひとつのリストに",
  "Aggregate Claude Code, Codex, and ACP agent sessions across all computers. Active, waiting, and unread tasks are prioritized in a single view.":
    "すべてのコンピュータの Claude Code、Codex、ACP Agent セッションを統合。実行中、対応待ち、未読タスクを優先度順に整理します。",
  "Answer questions and approve actions": "リアルタイム承認と指示送信",
  "Approve Bash commands and file edits, answer agent questions, add instructions during execution, or abort tasks immediately.":
    "スマホから Bash コマンドやファイル編集を承認し、AI の質問に返答。実行中の指示追加やタスクの中断も即座に行えます。",
  "Start a session from your phone": "スマホからセッションを開始",
  "Select Claude Code or Codex, choose a target computer and folder, attach screenshots, and dispatch new tasks directly from your phone.":
    "Claude Code または Codex、対象コンピュータとフォルダを選択。スクリーンショットを添付してスマホから直接タスクを指示できます。",
  "Browse files and terminals": "リモートファイルとターミナル",
  "Browse project directories on your remote computers, inspect code diffs, and open native host terminals directly from your mobile browser.":
    "リモートコンピュータのプロジェクトフォルダを閲覧し、コード変更を確認。ホストのネイティブターミナルも直接開けます。",
  "Scheduled tasks": "定時実行",
  "Configure recurring schedules on target computers to run automated agent workflows, tests, or daily maintenance automatically.":
    "指定したコンピュータで定期タスクを設定し、定期的なコードチェック、ビルド、自動化ワークフローを実行できます。",

  "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.":
    "Droppen はベータ版です。暗号設計は社内のセキュリティレビューを受けていますが、外部監査はまだです。ブラウザで使う場合は、そのとき読み込んだページのコードを信頼することになります。そのコードは秘密鍵を使えるうえ、持ち出すこともできます。",

  "Your AI keeps working on your computer. When it needs you, take control from your phone.": "ローカルコンピュータで AI を稼働させ、必要なときはいつでもスマホから対応。",

  "Free plan": "無料プラン",
  "Product facts": "製品情報",
  "Choose a screen": "画面を選ぶ",
  "Screen {{n}} of {{total}}": "{{total}} 画面中 {{n}} 画面目",
  "Screen {{n}} of {{total}}: {{title}}": "{{total}} 画面中 {{n}} 画面目：{{title}}",
  "All computers, one list.": "すべてのコンピュータを、ひとつのリストで。",
  "Connect multiple Mac and Linux computers under one account. View all Claude Code and Codex sessions in real time and see immediately which task requires your input.":
    "複数の Mac や Linux コンピュータをひとつのアカウントに集約。セッション状態をリアルタイムで同期し、対応待ちのタスクを即座に確認できます。",
  "Code on the left, terminal on the right.": "左にコード、右にターミナル。",
  "Run an AI session and native terminal side by side. Inspect code changes, test logs, and command outputs in a single window without switching applications.":
    "AI セッションとホストターミナルを並列表示。ウィンドウを切り替えることなく、コード変更、テストログ、コマンド出力を同一画面で確認できます。",
  "Leave the computer, take over on your phone.": "コンピュータを離れて、スマホで引き継ぐ。",
  "Open {{app}} in any mobile browser to sync sessions instantly. Tasks awaiting confirmation are pinned to the top for quick approval and replies.":
    "スマホのブラウザで {{app}} を開くだけで即座に同期。「対応待ち」が最上部に固定され、外出先でも承認、返答、指示が可能です。",
  "What you can do from your phone.": "スマートフォンでできること",
  "Read offline": "オフラインでも読めます",
  "When your computer is online, an encrypted copy syncs to your phone. If it shuts down or disconnects, you can still read the last {{free}} days ({{pro}} on Pro).":
    "オンライン時に暗号化コピーを自動保存。コンピュータがオフラインやスリープでも、過去 {{free}} 日間（Pro は {{pro}} 日間）の履歴を閲覧できます。",
  "Direct connection with automatic relay fallback.": "直接接続を優先、制限時は自動で中継へ切り替え",
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
  "{{n}} devices that run AI": "AI を動かすデバイス {{n}} 台",
  "{{devices}} devices, phones and browsers included": "デバイス {{devices}} 台（スマホ・ブラウザを含む）",
  "{{free}} days of offline access · {{gb}} GB relay": "オフライン閲覧 {{free}} 日 · 中継 {{gb}} GB",
  "{{pro}} devices that run AI · {{devices}} devices": "AI を動かすデバイス {{pro}} 台 · デバイス {{devices}} 台",
  "{{pro}} days of offline access · {{gb}} GB relay": "オフライン閲覧 {{pro}} 日 · 中継 {{gb}} GB",
  "Compare all plans": "すべてのプランを比較",
  "Recommended": "おすすめ",
  "USD / month": "USD / 月",
  "Extra usage is billed": "超過分は従量課金",
  "Choose Pro": "Pro を選ぶ",
  "${{yearly}} a year, about {{saving}}% off · prices in US dollars": "年額 ${{yearly}}、約 {{saving}}% お得 · 料金は米ドル",
  "Do I have to keep my computer on?": "コンピュータはつけっぱなしにする必要がありますか？",
  "No. The AI runs on your computer, but you do not need to stay at your desk. While it is offline, your phone can still read the last {{free}} days ({{pro}} on Pro) of synced sessions. Sending prompts and using the terminal resume once it reconnects.":
    "いいえ。AI はローカルコンピュータで動作するため、デスクに留まる必要はありません。コンピュータがオフラインでも、直近 {{free}} 日間（Pro は {{pro}} 日間）の同期済み履歴をスマホで確認できます。新規指示やターミナル操作は再接続後に再開されます。",
  "Is there a Windows client?": "Windows クライアントはありますか？",
  "Droppen's web app runs in any Windows browser, but the AI must run on a Mac, Linux, or WSL 2 machine. A native Windows host client is planned.":
    "Windows のブラウザから Droppen の Web アプリを利用できますが、AI は Mac、Linux、または WSL 2 上で動作する必要があります。Windows ネイティブホストは計画中です。",
  "Will my code be uploaded?": "コードはアップロードされますか？",
  "Session content is encrypted on your computer before transmission. Droppen Hub stores only encrypted copies, and relays forward only encrypted data; neither holds decryption keys. In a browser, you trust the page code loaded during that session.":
    "セッション内容はコンピュータから送信される前にエンドツーエンドで暗号化されます。Droppen Hub は暗号化コピーのみを保持し、中継ノードは暗号化データのみを転送します。いずれも復号鍵を保持しません。",
  "Is the free plan enough?": "無料プランで足りますか？",
  "If you have up to {{hosts}} computers that run AI and need {{free}} days of offline access, the free plan is sufficient. Upgrade to Pro for more devices, {{pro}} days of offline access, or additional relay bandwidth.":
    "AI を実行するコンピュータが {{hosts}} 台以下で、{{free}} 日間のオフライン履歴で十分な場合は無料プランで対応できます。さらなるデバイス数、{{pro}} 日間の履歴、追加の中継トラフィックが必要な場合は Pro をご検討ください。",
  "Product": "製品",
  "Resources": "リソース",
  "Legal": "法務",
  "Interface": "画面",
  "Encryption design": "暗号化の仕組み",
  "Back to top": "ページ上部へ",
  "Your AI, from your phone. End-to-end encrypted, open-source client.": "あなたの AI を、スマホで続きから。エンドツーエンド暗号化、クライアントはオープンソース。",
  "macOS 13 or later · Linux · Windows in the browser · iOS and Android": "macOS 13 以降 · Linux · Windows はブラウザ · iOS と Android",
};

export const landingBundles: Record<"zh-CN" | "zh-TW" | "ja", Bundle> = {
  "zh-CN": { ...zhCN, ...downloadZhCN, ...pricingZhCN },
  "zh-TW": { ...zhTW, ...downloadZhTW, ...pricingZhTW },
  ja: { ...ja, ...downloadJa, ...pricingJa },
};

registerCopy(landingBundles);
