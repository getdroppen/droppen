import { columnOf, registerCopy, type CopyBundle, type CopyRow } from "./copy-table";
import { downloadJa, downloadZhCN, downloadZhTW } from "./download-copy";
import { pricingJa, pricingZhCN, pricingZhTW } from "./pricing-copy";

/**
 * The public pages' base table: the header, the footer, the shared links, the words
 * two pages both draw, and the landing page's own selling copy. Every public page
 * imports this module for those words, and the render tests read `landingBundles` as
 * all of them, so it carries the download and pricing tables too (they do not
 * register themselves).
 */
const LINES: readonly CopyRow[] = [
  ["Droppen relay", "Droppen 中继", "Droppen 中繼", "Droppen 中継"],
  ["Computer", "电脑", "電腦", "コンピュータ"],
  ["Phone", "手机", "手機", "スマートフォン"],

  ["Linux server", "Linux 服务器", "Linux 伺服器", "Linux サーバー"],

  [
    "Droppen — your computer's Agent, on your phone",
    "Droppen — 电脑上的 Agent 在手机上接着用",
    "Droppen — 電腦上的 Agent 在手機上接著用",
    "Droppen — コンピュータ上の Agent を、スマホでそのまま使う",
  ],
  [
    "Your Agent keeps running on your computer. From your phone or browser, approve permissions, read output, and send instructions. You do not have to stay at your desk. End-to-end encrypted, with an open-source client.",
    "Agent 仍在电脑上运行。在手机或浏览器上审批权限、查看输出、发送指令，不必守在电脑前。端到端加密，客户端开源。",
    "Agent 仍在電腦上執行。在手機或瀏覽器上核准權限、查看輸出、傳送指令，不必守在電腦前。端對端加密，客戶端開源。",
    "Agent はコンピュータ上で動き続けます。スマホやブラウザから権限を承認し、出力を確認し、指示を送れます。デスクにいる必要はありません。エンドツーエンド暗号化、クライアントはオープンソース。",
  ],
  ["Site navigation", "网站导航", "網站導覽", "サイトナビゲーション"],
  ["Security", "安全", "安全", "セキュリティ"],
  ["Client source code", "客户端源代码", "客戶端原始碼", "クライアントのソースコード"],
  ["Third-party notices", "第三方软件声明", "第三方軟體聲明", "サードパーティ通知"],
  ["How it works", "工作原理", "運作方式", "しくみ"],
  ["Open app", "打开应用", "打開應用", "アプリを開く"],
  ["Sign in", "登录", "登入", "ログイン"],
  ["Sign up", "注册", "註冊", "登録"],

  ["Your computer's Agent, on your phone.", "电脑上的 Agent 在手机上接着用", "電腦上的 Agent 在手機上接著用", "コンピュータ上の Agent を、スマホでそのまま使う"],
  [
    "Your Agent keeps running on your computer. From your phone or browser, approve permissions, read output, and send instructions. You do not have to stay at your desk.",
    "Agent 仍在电脑上运行。在手机或浏览器上审批权限、查看输出、发送指令，不必守在电脑前。",
    "Agent 仍在電腦上執行。在手機或瀏覽器上核准權限、查看輸出、傳送指令，不必守在電腦前。",
    "Agent はコンピュータ上で動き続けます。スマホやブラウザから権限を承認し、出力を確認し、指示を送れます。デスクにいる必要はありません。",
  ],
  ["Start free", "开始试用", "開始試用", "無料で始める"],
  ["Sign up on the web", "在网页上注册", "在網頁上註冊", "Web で登録"],
  ["Open web app", "打开网页版", "開啟網頁版", "Web アプリを開く"],
  ["Join the waitlist", "加入等候名单", "加入等候名單", "ウェイトリストに登録"],
  ["Run Droppen on a Linux server", "Linux 服务器上运行 Droppen", "在 Linux 伺服器上執行 Droppen", "Linux サーバーで Droppen を実行"],
  ["Invite-only for now: sign-up needs an invite code.", "目前邀请制，注册需要邀请码。", "目前採邀請制，註冊需要邀請碼。", "いまは招待制です。登録には招待コードが必要です。"],
  ["Sign-up is closed for now.", "目前暂停注册。", "目前暫停註冊。", "現在、新規登録を停止しています。"],
  [
    "Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer",
    "Mac 上的 Droppen：Claude Code 和 Codex 的会话列表，以及一个等你回答的会话",
    "Mac 上的 Droppen：Claude Code 和 Codex 的工作階段清單，以及一個等你回答的工作階段",
    "Mac の Droppen：Claude Code と Codex のセッション一覧と、回答を待つセッション",
  ],
  [
    "Droppen with three panes: two sessions and a terminal",
    "Mac 上的 Droppen 分成三栏：两个会话和一个终端",
    "Mac 上的 Droppen 分成三欄：兩個工作階段和一個終端機",
    "Mac の Droppen を 3 ペインに：2 つのセッションとターミナル",
  ],
  [
    "Droppen on a phone: what needs you, and a session's question",
    "手机上的 Droppen：「需要你」清单，以及一个会话的提问",
    "手機上的 Droppen：「需要你」清單，以及一個工作階段的提問",
    "スマホの Droppen：「対応待ち」リストとセッションの質問",
  ],

  ["End-to-end encrypted", "端到端加密", "端對端加密", "エンドツーエンド暗号化"],
  ["Security and encryption", "安全与端到端加密", "安全與端對端加密", "セキュリティとエンドツーエンド暗号化"],
  [
    "Encrypted on your computer and decrypted only on your devices. Droppen Hub stores only encrypted copies. Droppen relays forward only encrypted data. Neither holds a key that can decrypt them.",
    "内容在你的电脑上加密，只在你的设备上解密。Droppen Hub 保存的只是加密副本，Droppen 中继转发的只是加密数据，两者都没有解密所需的密钥。",
    "內容在你的電腦上加密，只在你的裝置上解密。Droppen Hub 儲存的只是加密副本，Droppen 中繼轉送的只是加密資料，兩者都沒有解密所需的金鑰。",
    "暗号化はあなたのコンピュータで、復号はあなたのデバイスでだけ。Droppen Hub が保存するのは暗号化コピーだけ、Droppen 中継が転送するのは暗号化データだけで、どちらも復号に必要な鍵を持っていません。",
  ],
  ["Only devices you approve can join", "只有你批准的设备才能加入", "只有你核准的裝置才能加入", "加われるのは、承認したデバイスだけ"],
  [
    "A new phone or browser can join only after a device you already trust approves it. If a device is lost, remove it from another device. A removed device cannot decrypt new data.",
    "新手机或新浏览器须经已受信任的设备批准后才能加入。设备丢失时，可从另一台设备上移除它。已移除的设备无法解密新数据。",
    "新手機或新瀏覽器須經已信任的裝置核准後才能加入。裝置遺失時，可從另一台裝置移除。已移除的裝置無法解密新資料。",
    "新しいスマホやブラウザは、信頼済みのデバイスが承認した場合にだけ参加できます。デバイスを紛失したら、別のデバイスから削除してください。削除したデバイスは新しいデータを復号できません。",
  ],
  ["Open-source client", "客户端开源", "客戶端開源", "クライアントはオープンソース"],
  ["How the encryption works", "加密实现原理", "加密實現原理", "暗号化のしくみ"],
  [
    "The droppen command-line program, the Mac app and the web app are open source under AGPL-3.0. Droppen Hub is not open source.",
    "droppen 命令行程序、Mac 应用和网页应用都是开源的，许可证是 AGPL-3.0。Droppen Hub 不开源。",
    "droppen 命令列程式、Mac 應用程式和網頁應用程式都是開源的，授權條款是 AGPL-3.0。Droppen Hub 不開源。",
    "droppen コマンドラインプログラム、Mac アプリ、Web アプリは AGPL-3.0 のオープンソースです。Droppen Hub はオープンソースではありません。",
  ],


  ["How it connects", "连接方式", "連線方式", "接続方法"],
  ["Direct · {{ms}}", "直连 · {{ms}}", "直連 · {{ms}}", "直結 · {{ms}}"],
  ["Relay · {{ms}}", "中继 · {{ms}}", "中繼 · {{ms}}", "中継 · {{ms}}"],

  ["Every session in one list", "所有会话，一个列表", "所有工作階段，一個清單", "すべてのセッションを、ひとつのリストに"],
  [
    "See Claude Code, Codex, and ACP agent sessions from all computers in one list. Active, waiting, and unread tasks come first.",
    "在一份列表中查看所有电脑上的 Claude Code、Codex 与 ACP Agent 会话。运行中、等待中与未读任务排在前面。",
    "在單一清單中查看所有電腦上的 Claude Code、Codex 與 ACP Agent 工作階段。執行中、等待中與未讀任務排在前面。",
    "すべてのコンピュータの Claude Code、Codex、ACP Agent セッションをひとつのリストで確認。実行中、対応待ち、未読のタスクが上に表示されます。",
  ],
  ["Answer questions and approve actions", "实时审批与交互", "即時審批與互動", "リアルタイム承認と指示送信"],
  [
    "Approve Bash commands and file edits, answer the agent's questions, add instructions while it works, or stop a task.",
    "审批 Bash 命令与文件修改，回答 Agent 的提问，在运行时补充指令，或停止任务。",
    "核准 Bash 指令與檔案修改，回答 Agent 的提問，在執行中補充指令，或停止任務。",
    "Bash コマンドやファイル編集を承認し、Agent の質問に答え、実行中に指示を追加し、タスクを停止できます。",
  ],
  ["Start a session from your phone", "在手机上新建会话", "在手機上新增工作階段", "スマホからセッションを開始"],
  [
    "Select Claude Code or Codex, choose a computer and a folder, attach screenshots, and start a task from your phone.",
    "选择 Claude Code 或 Codex、目标电脑和文件夹，附上截图，在手机上直接开始任务。",
    "選擇 Claude Code 或 Codex、目標電腦和資料夾，附上螢幕截圖，在手機上直接開始任務。",
    "Claude Code または Codex を選び、コンピュータとフォルダを指定し、スクリーンショットを添付して、スマホからタスクを開始できます。",
  ],
  ["Browse files and terminals", "浏览文件与终端", "遠端檔案與終端機", "リモートファイルとターミナル"],
  [
    "Browse project folders on your computers, inspect code changes, and open a terminal on the computer, all from your phone's browser.",
    "在手机浏览器中浏览电脑上的项目文件夹、查看代码改动，并打开电脑上的终端。",
    "在手機瀏覽器中瀏覽電腦上的專案資料夾、查看程式碼變更，並開啟電腦上的終端機。",
    "スマホのブラウザから、コンピュータ上のプロジェクトフォルダを閲覧し、コード変更を確認し、コンピュータのターミナルを開けます。",
  ],
  ["Scheduled tasks", "定时任务", "排程任務", "定時実行"],
  [
    "Set a schedule on a computer to run agent workflows, tests, or daily maintenance.",
    "在电脑上设置定时任务，按计划运行 Agent 工作流、测试或日常维护。",
    "在電腦上設定排程任務，按計畫執行 Agent 工作流程、測試或日常維護。",
    "コンピュータにスケジュールを設定し、Agent ワークフロー、テスト、日常メンテナンスを定期的に実行できます。",
  ],

  [
    "Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.",
    "Droppen 目前是测试版。加密设计经过了内部安全评审，还没有做过外部审计。在浏览器里使用时，你需要信任当次加载的页面代码：这份代码能使用你的私钥，也能把它们复制出去。",
    "Droppen 目前是測試版。加密設計經過內部安全審查，還沒有做過外部稽核。在瀏覽器裡使用時，你需要信任當次載入的頁面程式碼：這份程式碼能使用你的私密金鑰，也能把它們複製出去。",
    "Droppen はベータ版です。暗号設計は社内のセキュリティレビューを受けていますが、外部監査はまだです。ブラウザで使う場合は、そのとき読み込んだページのコードを信頼することになります。そのコードは秘密鍵を使えるうえ、持ち出すこともできます。",
  ],

  [
    "Start free. Then install Droppen on your computer.",
    "先开始试用，再在电脑上安装 Droppen。",
    "先開始試用，再在電腦上安裝 Droppen。",
    "まず無料で始めて、コンピュータに Droppen をインストールします。",
  ],

  ["Choose a screen", "选择界面", "選擇介面", "画面を選ぶ"],
  ["Screen {{n}} of {{total}}", "第 {{n}} 个界面，共 {{total}} 个", "第 {{n}} 個介面，共 {{total}} 個", "{{total}} 画面中 {{n}} 画面目"],
  [
    "Screen {{n}} of {{total}}: {{title}}",
    "第 {{n}} 个界面，共 {{total}} 个：{{title}}",
    "第 {{n}} 個介面，共 {{total}} 個：{{title}}",
    "{{total}} 画面中 {{n}} 画面目：{{title}}",
  ],
  ["All computers, one list.", "所有电脑，一个列表。", "所有電腦，一個清單。", "すべてのコンピュータを、ひとつのリストで。"],
  [
    "Connect Mac and Linux computers to one account. See all Claude Code and Codex sessions in real time. Tasks that need your input are marked.",
    "将多台 Mac 和 Linux 电脑接入同一个账户。实时查看所有 Claude Code 和 Codex 会话。需要你确认的任务会标出来。",
    "將多台 Mac 與 Linux 電腦接入同一個帳戶。即時查看所有 Claude Code 與 Codex 工作階段。需要你確認的任務會標示出來。",
    "複数の Mac と Linux コンピュータをひとつのアカウントに接続。すべての Claude Code と Codex セッションをリアルタイムで確認でき、対応が必要なタスクには印が付きます。",
  ],
  ["Code on the left, terminal on the right.", "左边代码，右边终端。", "左邊程式碼，右邊終端機。", "左にコード、右にターミナル。"],
  [
    "Run an AI session and a terminal side by side. See code changes, test logs, and command output in one window. You do not have to switch applications.",
    "AI 会话与终端并排运行。在同一窗口查看代码改动、测试日志和命令输出，不必切换应用。",
    "AI 工作階段與終端機並排執行。在同一視窗查看程式碼變更、測試記錄與指令輸出，不必切換應用程式。",
    "AI セッションとターミナルを並べて実行。コード変更、テストログ、コマンド出力をひとつのウィンドウで確認でき、アプリを切り替える必要はありません。",
  ],
  ["Leave the computer, take over on your phone.", "离开电脑，在手机上接手。", "離開電腦，在手機上接手。", "コンピュータを離れて、スマホで引き継ぐ。"],
  [
    "Open {{app}} in a mobile browser. Sessions sync at once, and tasks that wait for your approval stay at the top.",
    "在手机浏览器打开 {{app}}，会话立即同步。等待你批准的任务会置顶。",
    "在手機瀏覽器開啟 {{app}}，工作階段立即同步。等待你核准的任務會置頂。",
    "スマホのブラウザで {{app}} を開くと、セッションがすぐに同期されます。承認待ちのタスクは最上部に表示されます。",
  ],
  ["What you can do from your phone.", "在手机上能做什么", "在手機上能做什麼", "スマートフォンでできること"],
  ["Read offline", "离线也能读", "離線也能讀", "オフラインでも読めます"],
  [
    "When your computer is online, an encrypted copy syncs to your phone. If the computer shuts down or goes offline, you can still read the last {{free}} days ({{pro}} on Pro).",
    "电脑在线时，加密副本会同步到手机。电脑关机或离线后，仍可阅读最近 {{free}} 天（Pro 为 {{pro}} 天）的内容。",
    "電腦在線時，加密副本會同步到手機。電腦關機或離線後，仍可閱讀最近 {{free}} 天（Pro 為 {{pro}} 天）的內容。",
    "コンピュータがオンラインのとき、暗号化コピーがスマホに同期されます。コンピュータの電源が切れたりオフラインになっても、直近 {{free}} 日間（Pro は {{pro}} 日間）は読めます。",
  ],
  ["Direct connection with automatic relay fallback.", "直连优先，网络受限时自动中继", "直連優先，網路受限時自動中繼", "直接接続を優先、制限時は自動で中継へ切り替え"],
  ["End-to-end encryption", "端到端加密", "端對端加密", "エンドツーエンド暗号化"],
  ["Encryption", "加密", "加密", "暗号化"],
  ["Keys", "密钥", "金鑰", "鍵"],
  ["Local only", "仅本地", "僅本地", "ローカルのみ"],
  ["{{n}} devices that run AI", "{{n}} 台运行 AI 的设备", "{{n}} 台執行 AI 的裝置", "AI を動かすデバイス {{n}} 台"],
  [
    "{{devices}} devices, phones and browsers included",
    "共 {{devices}} 台设备（含手机和浏览器）",
    "共 {{devices}} 台裝置（含手機和瀏覽器）",
    "デバイス {{devices}} 台（スマホ・ブラウザを含む）",
  ],
  [
    "{{free}} days of offline access · {{gb}} GB relay",
    "{{free}} 天离线查看 · {{gb}} GB 中继流量",
    "{{free}} 天離線查看 · {{gb}} GB 中繼流量",
    "オフライン閲覧 {{free}} 日 · 中継 {{gb}} GB",
  ],
  [
    "{{pro}} devices that run AI · {{devices}} devices",
    "{{pro}} 台运行 AI 的设备 · 共 {{devices}} 台设备",
    "{{pro}} 台執行 AI 的裝置 · 共 {{devices}} 台裝置",
    "AI を動かすデバイス {{pro}} 台 · デバイス {{devices}} 台",
  ],
  [
    "{{pro}} days of offline access · {{gb}} GB relay",
    "{{pro}} 天离线查看 · {{gb}} GB 中继流量",
    "{{pro}} 天離線查看 · {{gb}} GB 中繼流量",
    "オフライン閲覧 {{pro}} 日 · 中継 {{gb}} GB",
  ],
  ["Compare all plans", "比较所有方案", "比較所有方案", "すべてのプランを比較"],
  ["Recommended", "推荐", "推薦", "おすすめ"],
  ["USD / month", "美元 / 月", "美元 / 月", "USD / 月"],
  ["Extra usage is billed", "超出部分按用量计费", "超出部分按用量計費", "超過分は従量課金"],
  ["Choose Pro", "选择 Pro", "選擇 Pro", "Pro を選ぶ"],
  [
    "${{yearly}} a year, about {{saving}}% off · prices in US dollars",
    "年付 ${{yearly}}，约省 {{saving}}% · 价格以美元计",
    "年付 ${{yearly}}，約省 {{saving}}% · 價格以美元計",
    "年額 ${{yearly}}、約 {{saving}}% お得 · 料金は米ドル",
  ],
  ["Do I have to keep my computer on?", "电脑需要一直开着吗？", "電腦需要一直開著嗎？", "コンピュータはつけっぱなしにする必要がありますか？"],
  [
    "The computer must stay on to run tasks. You do not need to stay at your desk: while the computer is offline, your phone can still read the last {{free}} days ({{pro}} on Pro) of synced sessions. Sending prompts and using the terminal resume when it reconnects.",
    "电脑需要保持开机才能运行任务。你不必守在电脑前：电脑离线时，手机仍可阅读最近 {{free}} 天（Pro 为 {{pro}} 天）已同步的会话。电脑重新联网后，可以继续发送指令和使用终端。",
    "電腦需要保持開機才能執行任務。你不必守在電腦前：電腦離線時，手機仍可閱讀最近 {{free}} 天（Pro 為 {{pro}} 天）已同步的工作階段。電腦重新連線後，可以繼續傳送指令和使用終端機。",
    "タスクを実行するには、コンピュータの電源を入れておく必要があります。デスクにいる必要はありません。コンピュータがオフラインでも、直近 {{free}} 日間（Pro は {{pro}} 日間）の同期済みセッションをスマホで読めます。プロンプトの送信とターミナルは、再接続すると再開できます。",
  ],
  ["Is there a Windows client?", "有 Windows 客户端吗？", "有 Windows 客戶端嗎？", "Windows クライアントはありますか？"],
  [
    "Droppen's web app runs in any Windows browser, but the AI must run on a Mac, Linux, or WSL 2 machine. A native Windows host client is planned.",
    "Droppen 网页端可在 Windows 浏览器中完整使用，但运行 AI 的电脑须为 Mac、Linux 或 WSL 2 环境。Windows 原生宿主客户端仍在规划中。",
    "Droppen 網頁端可在 Windows 瀏覽器中完整使用，但執行 AI 的電腦須為 Mac、Linux 或 WSL 2 環境。Windows 原生宿主客戶端仍在規劃中。",
    "Windows のブラウザから Droppen の Web アプリを利用できますが、AI は Mac、Linux、または WSL 2 上で動作する必要があります。Windows ネイティブホストは計画中です。",
  ],
  ["Will my code be uploaded?", "我的代码会上传吗？", "我的程式碼會上傳嗎？", "コードはアップロードされますか？"],
  [
    "Session content is encrypted on your computer before it is sent. In a browser, you trust the page code loaded in that session.",
    "会话内容在离开电脑前已加密。在浏览器中使用时，需信任当次加载的页面代码。",
    "工作階段內容在離開電腦前已加密。在瀏覽器中使用時，需信任當次載入的頁面程式碼。",
    "セッションの内容は、送信前にコンピュータで暗号化されます。ブラウザでは、そのとき読み込まれたページのコードを信頼することになります。",
  ],
  ["Is the free plan enough?", "免费版够用吗？", "免費版夠用嗎？", "無料プランで足りますか？"],
  [
    "If you have up to {{hosts}} computers that run AI and need {{free}} days of offline access, the free plan is sufficient. Upgrade to Pro for more devices, {{pro}} days of offline access, or additional relay bandwidth.",
    "若管理不超过 {{hosts}} 台运行 AI 的电脑，且仅需 {{free}} 天离线历史查看，免费版完全够用。若需连接更多设备、长达 {{pro}} 天离线访问或更高流量限额，可随时升级 Pro 方案。",
    "若管理不超過 {{hosts}} 台執行 AI 的電腦，且僅需 {{free}} 天離線歷史查看，免費版完全夠用。若需連接更多裝置、長達 {{pro}} 天離線存取或更高流量配額，可隨時升級 Pro 方案。",
    "AI を実行するコンピュータが {{hosts}} 台以下で、{{free}} 日間のオフライン履歴で十分な場合は無料プランで対応できます。さらなるデバイス数、{{pro}} 日間の履歴、追加の中継トラフィックが必要な場合は Pro をご検討ください。",
  ],
  ["Product", "产品", "產品", "製品"],
  ["Resources", "资源", "資源", "リソース"],
  ["Legal", "法律", "法律", "法務"],
  ["Interface", "界面", "介面", "画面"],
  ["Encryption design", "加密原理", "加密原理", "暗号化の仕組み"],
  ["Back to top", "回到顶部", "回到頂部", "ページ上部へ"],
  [
    "Your AI, from your phone. End-to-end encrypted, open-source client.",
    "你的 AI，手机上接着用。端到端加密，客户端开源。",
    "你的 AI，手機上接著用。端對端加密，客戶端開源。",
    "あなたの AI を、スマホで続きから。エンドツーエンド暗号化、クライアントはオープンソース。",
  ],
];

export const landingBundles: Record<"zh-CN" | "zh-TW" | "ja", CopyBundle> = {
  "zh-CN": { ...columnOf(LINES, 1), ...downloadZhCN, ...pricingZhCN },
  "zh-TW": { ...columnOf(LINES, 2), ...downloadZhTW, ...pricingZhTW },
  ja: { ...columnOf(LINES, 3), ...downloadJa, ...pricingJa },
};

registerCopy(landingBundles);
