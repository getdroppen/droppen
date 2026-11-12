import { columnOf, type CopyRow } from "./copy-table";

/** The download page's words, registered by `copy.ts` with the public pages' base bundle. */
const LINES: readonly CopyRow[] = [
  ["Download Droppen", "下载 Droppen", "下載 Droppen", "Droppen をダウンロード"],

  [
    "Install Droppen where Claude Code or Codex runs: the Mac app, or one command on Linux, macOS, or WSL. Your phone needs only the browser.",
    "在运行 Claude Code 或 Codex 的设备上安装 Droppen。支持 macOS 原生应用，或通过单行命令运行于 Linux、macOS 与 WSL。控制端无需安装，任何浏览器即开即用。",
    "在執行 Claude Code 或 Codex 的裝置上安裝 Droppen。支援 macOS 原生應用程式，或透過單行指令執行於 Linux、macOS 與 WSL。控制端無需安裝，任何瀏覽器即開即用。",
    "Claude Code や Codex を実行するデバイスに Droppen をインストールします。macOS ネイティブアプリ、または Linux・macOS・WSL での 1 行コマンドに対応。操作側へのインストールは不要で、ブラウザから直接利用できます。",
  ],

  ["This device", "这台设备", "這台裝置", "このデバイス"],
  ["Linux", "Linux", "Linux", "Linux"],
  ["Windows / WSL", "Windows / WSL", "Windows / WSL", "Windows / WSL"],
  ["Windows", "Windows", "Windows", "Windows"],
  ["Phone / tablet", "手机 / 平板", "手機 / 平板", "スマホ / タブレット"],
  ["Download", "下载", "下載", "ダウンロード"],
  ["copy the command", "复制命令", "複製指令", "コマンドをコピー"],
  ["macOS", "macOS", "macOS", "macOS"],
  ["Browser", "浏览器", "瀏覽器", "ブラウザ"],
  ["Other platforms", "其他平台", "其他平台", "その他のプラットフォーム"],
  ["Coming soon", "敬请期待", "敬請期待", "近日公開"],

  ["The App is coming soon", "App 即将推出", "App 即將推出", "アプリは近日公開"],
  [
    "Until it is out, run Droppen in the background on your Mac: one line, and the same sign-in.",
    "原生应用推出前，可通过单行命令将 Droppen 作为后台服务在 Mac 上运行，登录流程与体验完全一致。",
    "原生應用程式推出前，可透過單行指令將 Droppen 作為背景服務在 Mac 上執行，登入流程與體驗完全一致。",
    "ネイティブアプリ公開までは、Mac のバックグラウンドサービスとして Droppen を実行できます。コマンドは 1 行、サインイン手順は同じです。",
  ],
  [
    "Linux support, with no browser needed on the server, is on its way. Until then, run Droppen on a Mac.",
    "Linux 无头服务器支持即将推出。在此之前，请先在 Mac 设备上运行 Droppen。",
    "Linux 無頭伺服器支援即將推出。在此之前，請先在 Mac 裝置上執行 Droppen。",
    "Linux サーバー（ヘッドレス環境）対応は近日提供予定です。それまでは Mac 上で Droppen を実行してください。",
  ],

  ["Droppen for Mac", "Droppen · Mac 版", "Droppen · Mac 版", "Droppen for Mac"],

  ["Download for macOS", "下载 macOS 版", "下載 macOS 版", "macOS 版をダウンロード"],



  [
    "Using it from your phone: signing in to Droppen from the app is coming in an update. Until then, the app runs Droppen on this Mac.",
    "在手机上使用：在 App 内登录 Droppen 的功能将在后续更新中提供。在此之前，App 仅在这台 Mac 上运行 Droppen。",
    "在手機上使用：在 App 內登入 Droppen 的功能將在後續更新中提供。在此之前，App 僅在這台 Mac 上執行 Droppen。",
    "スマホから使う場合：アプリからの Droppen へのサインインは、今後のアップデートで対応します。それまでは、アプリはこの Mac でのみ Droppen を動かします。",
  ],

  ["Paste it into Terminal on that Mac.", "在 Mac 的「终端」中粘贴并执行该命令。", "在 Mac 的「終端機」中貼上並執行該指令。", "Mac の「ターミナル」に貼り付けて実行します。"],


  ["Server", "服务器", "伺服器", "サーバー"],
  ["Run the server", "运行服务器", "執行伺服器", "サーバーを動かす"],
  [
    "Linux · x86_64 or aarch64 · glibc 2.28+ · systemd",
    "Linux · x86_64 或 aarch64 · glibc 2.28+ · systemd",
    "Linux · x86_64 或 aarch64 · glibc 2.28+ · systemd",
    "Linux · x86_64 または aarch64 · glibc 2.28+ · systemd",
  ],
  ["Windows · WSL 2", "Windows · WSL 2", "Windows · WSL 2", "Windows · WSL 2"],
  ["Get a one-time code", "获取一次性注册码", "取得一次性註冊碼", "ワンタイムコードを取得"],
  [
    "In my devices, add a device and show its one-time code. It works once, for 10 minutes.",
    "在「我的设备」页面点击添加设备并生成注册码。该注册码 10 分钟内有效，仅可使用一次。",
    "在「我的裝置」頁面點擊新增裝置並產生註冊碼。該註冊碼 10 分鐘內有效，僅可使用一次。",
    "「マイデバイス」でデバイスを追加し、ワンタイムコードを表示します。コードは 1 回のみ有効で、有効期限は 10 分間です。",
  ],
  ["Install it", "安装", "安裝", "インストール"],
  [
    "Replace <code> with the one-time code.",
    "将命令中的 <注册码> 替换为获取的一次性注册码并执行。",
    "將指令中的 <註冊碼> 替換為獲取的一次性註冊碼並執行。",
    "コマンド内の <コード> を取得したワンタイムコードに置き換えて実行します。",
  ],
  ["<code>", "<注册码>", "<註冊碼>", "<コード>"],

  ["Check it is online", "确认在线", "確認在線", "オンラインを確認"],
  [
    "It shows as online in my devices. If the script asks, run this so it stays up after you log out.",
    "设备在「我的设备」中显示为在线状态。若安装脚本提示，请运行下方命令以允许用户登出后持续在后台运行。",
    "裝置在「我的裝置」中顯示為在線狀態。若安裝指令碼提示，請執行下方指令以允許使用者登出後持續在背景執行。",
    "「マイデバイス」にオンラインとして表示されます。スクリプトに案内された場合は、ログアウト後も常駐するよう以下のコマンドを実行してください。",
  ],
  ["It shows as online in my devices.", "设备在「我的设备」列表中已显示为在线状态。", "裝置在「我的裝置」清單中已顯示為在線狀態。", "「マイデバイス」の一覧でオンライン状態として表示されます。"],
  ["How to turn on systemd", "如何开启 systemd", "如何開啟 systemd", "systemd を有効にする方法"],
  [
    "This browser reports Linux on {{arch}}.",
    "当前浏览器运行在 {{arch}} 架构的 Linux 系统上。",
    "目前瀏覽器執行在 {{arch}} 架構的 Linux 系統上。",
    "このブラウザは {{arch}} の Linux で動いています。",
  ],
  [
    "Droppen's Linux builds are for x86_64 and aarch64.",
    "Droppen 的 Linux 版本支持 x86_64 和 aarch64。",
    "Droppen 的 Linux 版本支援 x86_64 和 aarch64。",
    "Droppen の Linux 版は x86_64 と aarch64 に対応しています。",
  ],

  [
    "Once Linux is supported: a Linux server, or WSL.",
    "Linux 支持推出后：可使用 Linux 服务器或 WSL。",
    "Linux 支援推出後：可使用 Linux 伺服器或 WSL。",
    "Linux 対応後は、Linux サーバーか WSL で。",
  ],

  [
    "Sign in with the account your other devices use.",
    "使用与其他受控设备相同的 Droppen 账号登录。",
    "使用與其他受控裝置相同的 Droppen 帳號登入。",
    "他の接続デバイスと同じ Droppen アカウントでサインインします。",
  ],
  ["Approve this browser", "批准这个浏览器", "核准這個瀏覽器", "このブラウザを承認"],
  [
    "On a device already in your account, open my devices → add a browser or phone, then scan the QR code this browser shows, or enter its number code.",
    "在已认证设备上打开「我的设备 → 添加浏览器或手机」，扫描当前浏览器展示的二维码，或直接输入配对数字码。",
    "在已認證裝置上打開「我的裝置 → 新增瀏覽器或手機」，掃描當前瀏覽器顯示的 QR 碼，或直接輸入配對數字碼。",
    "認証済みデバイスで「マイデバイス → ブラウザまたはスマホを追加」を開き、このブラウザに表示された QR コードをスキャンするか、数字コードを入力します。",
  ],
  ["Use it on any other device", "在手机或任意其他设备上使用", "在手機或任意其他裝置上使用", "スマホなどほかのデバイスで使う"],
  [
    "Nothing to install: any browser works.",
    "控制端无需安装应用：任何现代浏览器均可直接访问。",
    "控制端無需安裝應用程式：任何現代瀏覽器均可直接存取。",
    "操作側にアプリのインストールは不要：モダンブラウザからそのまま利用できます。",
  ],

  ["Manage your installation", "管理安装", "管理安裝", "インストールの管理"],
  [
    "Update, uninstall, or check what you installed.",
    "版本更新、服务卸载以及文件完整性校验指南。",
    "版本更新、服務解除安裝以及檔案完整性校驗指南。",
    "アップデート、アンインストール、および配布ファイルの整合性検証。",
  ],
  ["Update", "更新", "更新", "アップデート"],
  [
    "The Mac app updates itself. For a command-line install, run the install command again: it upgrades in place and keeps your data.",
    "macOS 应用内置自动更新。通过命令行安装的设备，再次运行安装命令即可就地平滑升级，所有本地数据与配置均完整保留。",
    "macOS 應用程式內建自動更新。透過指令行安裝的裝置，再次執行安裝指令即可原地升級，所有本地資料與設定完整保留。",
    "macOS アプリは自動でアップデートされます。コマンドライン版は、インストールコマンドを再実行することでデータや設定を保持したままインプレース更新できます。",
  ],
  [
    "Run the install command again: it upgrades in place and keeps your data.",
    "再次运行安装命令即可就地平滑升级，所有本地数据与配置均完整保留。",
    "再次執行安裝指令即可原地升級，所有本地資料與設定完整保留。",
    "インストールコマンドを再実行することで、データと設定を保持したままインプレース更新されます。",
  ],
  ["Uninstall", "卸载", "解除安裝", "アンインストール"],
  ["Remove it from your account", "从账户中移除", "從帳戶中移除", "アカウントから削除する"],
  [
    "On {{app}}, open my devices and remove the device. Every key it held is replaced, so it cannot decrypt anything written afterwards.",
    "在 {{app}} 的「我的设备」列表中移除该设备。系统将立即轮换加密密钥，该设备此后无法解密任何新的会话数据。",
    "在 {{app}} 的「我的裝置」清單中移除該裝置。系統將立即輪換加密金鑰，該裝置此後無法解密任何新的工作階段資料。",
    "{{app}} の「マイデバイス」でこのデバイスを削除します。暗号鍵は直ちに無効化され、以降に書き込まれたセッションは復号できなくなります。",
  ],
  ["Disconnect it", "在设备上断开连接", "在裝置上中斷連線", "デバイスで接続を解除する"],
  [
    "On that device, open Droppen's Settings → Access → Droppen → Disconnect this device. This deletes the Droppen keys stored on it. Removing the device from your account leaves them in place.",
    "在该设备上打开 Droppen 界面，点击「设置 → 访问 → Droppen → 断开这台设备」。此操作将彻底销毁保存在该设备上的加密密钥。（仅在云端账户中移除设备不会清除设备上的残留密钥）。",
    "在該裝置上開啟 Droppen 介面，點擊「設定 → 存取 → Droppen → 中斷這台裝置」。此操作將徹底銷毀保存在本地的加密金鑰。（僅在雲端帳號中移除裝置不會清除本地殘留金鑰）。",
    "そのデバイスで Droppen の「設定 → アクセス → Droppen → このデバイスの接続を解除」を開きます。これによりローカルに保存された暗号鍵が完全に削除されます。（アカウントからの削除だけではローカルの鍵は残ります）。",
  ],
  ["Remove the program", "删除程序", "刪除程式", "プログラムを削除する"],
  [
    "Mac app: quit Droppen and drag it from Applications to the Trash.",
    "Mac 应用：退出 Droppen，将其从「应用程序」文件夹拖入废纸篓。",
    "Mac 應用程式：結束 Droppen，將其從「應用程式」資料夾拖入垃圾桶。",
    "Mac アプリ：Droppen を終了し、「アプリケーション」フォルダからゴミ箱にドラッグします。",
  ],
  [
    "Command-line install (Mac or Linux): run this on that device, as the user who installed it:",
    "命令行安装（Mac 或 Linux）：在对应设备上以安装时所用的用户身份运行：",
    "指令行安裝（Mac 或 Linux）：在對應裝置上以安裝時所用的使用者身分執行：",
    "コマンドラインでのインストール（Mac または Linux）：そのデバイスで、インストールしたユーザーとして実行します：",
  ],
  [
    "Run this on that device, as the user who installed it:",
    "在对应设备上以安装时所用的用户身份运行：",
    "在對應裝置上以安裝時所用的使用者身分執行：",
    "そのデバイスで、インストールしたユーザーとして実行します：",
  ],
  [
    "It stops and removes the service and the program, keeps your data, and shows where the data is and how to delete it.",
    "该命令将停止并注销后台服务与二进制文件，保留历史会话数据，并输出本地数据路径与彻底清理指引。",
    "該指令將停止並註銷背景服務與二進位檔案，保留歷史工作階段資料，並輸出本地資料路徑與徹底清理指引。",
    "バックグラウンドサービスと実行ファイルを停止・削除します。データは保持され、データの保存先パスと完全削除の手順が画面に出力されます。",
  ],
  ["Checks & connection details", "核对与连接详情", "核對與連線詳情", "確認と接続の詳細"],
  ["Check the file before you open it", "打开前先核对文件", "開啟前先核對檔案", "開く前にファイルを確認する"],
  [
    "Before you open the .dmg or extract the .tar.gz, check it in a terminal:",
    "在打开 .dmg 或解压 .tar.gz 之前，在终端中校验文件哈希：",
    "在打開 .dmg 或解壓 .tar.gz 之前，在終端機中校驗檔案雜湊：",
    ".dmg を開く前、または .tar.gz を展開する前に、ターミナルでハッシュ値を照合します：",
  ],
  [
    "The checksum must match that file's line in the release's SHA256SUMS. If it does not, do not install the file. The install script runs the same check first.",
    "校验哈希值必须与发布页 SHA256SUMS 文件中对应项完全一致；若不匹配请勿安装。一键安装脚本在执行前也会自动进行哈希比对。",
    "校驗雜湊值必須與發布頁 SHA256SUMS 檔案中對應項目完全一致；若不符請勿安裝。一鍵安裝指令碼在執行前也會自動進行雜湊比對。",
    "算出されたチェックサムがリリースの SHA256SUMS と完全に一致することを確認してください。一致しない場合はインストールしないでください。インストールスクリプトも実行前に同様の自動検証を行います。",
  ],
  ["Build provenance", "构建来源证明", "建置來源證明", "ビルドの来歴"],
  [
    "From v0.2.127, every file has a build provenance attestation. With the GitHub CLI, check that this repository's release workflow built it:",
    "从 v0.2.127 起，官方构建产物均包含 SLSA 构建来源证明（Attestation）。通过 GitHub CLI 可核验文件确由本仓库发布工作流构建：",
    "從 v0.2.127 起，官方組建產物均包含 SLSA 組建來源證明（Attestation）。透過 GitHub CLI 可核驗檔案確由本儲存庫發布工作流程組建：",
    "v0.2.127 以降、すべての公式配布物に SLSA ビルド出所証明（Attestation）が添付されています。GitHub CLI で、公式リポジトリのリリースワークフローでビルドされたことを検証できます：",
  ],
  ["How to verify a download", "如何校验下载文件", "如何校驗下載檔案", "ダウンロードの検証方法"],
  ["Mac app", "Mac 应用", "Mac 應用程式", "Mac アプリ"],
  [
    "Signed with a Developer ID and notarized by Apple. It installs only updates signed for that version.",
    "使用 Developer ID 签名并经 Apple 官方公证；仅允许安装带有合规签名的官方更新。",
    "使用 Developer ID 簽章並經 Apple 官方公證；僅允許安裝帶有合規簽章的官方更新。",
    "Apple Developer ID で署名され、Apple の公証を受けています。該当バージョン向けに正しく署名されたアップデートのみをインストールします。",
  ],
  ["Install script", "安装脚本", "安裝腳本", "インストールスクリプト"],
  [
    "Runs as you, never as root, and checks the release's SHA-256 checksums before it installs anything.",
    "始终以当前普通用户身份运行，从不使用 root 权限；安装前自动校验发布文件的 SHA-256 哈希值。",
    "始終以當前普通使用者身分執行，從不使用 root 權限；安裝前自動校驗發布檔案的 SHA-256 雜湊值。",
    "一般ユーザー権限で動作し、root 権限は一切不要です。インストール前に配布ファイルの SHA-256 チェックサムを自動検証します。",
  ],
  ["Background service", "后台服务", "背景服務", "バックグラウンドサービス"],
  [
    "A command-line install runs as a launchd user agent on macOS, or a systemd user service on Linux. On Linux, --system makes it a system service that starts at boot and still runs as you; sudo only sets up and manages that service.",
    "命令行安装在 macOS 上注册为 launchd 用户代理，在 Linux 上注册为 systemd 用户服务。在 Linux 上指定 --system 参数可注册为系统开机自启服务，Droppen 仍以你的用户身份运行，sudo 仅用于注册与管理系统服务。",
    "指令行安裝在 macOS 上註冊為 launchd 使用者代理，在 Linux 上註冊為 systemd 使用者服務。在 Linux 上指定 --system 參數可註冊為系統開機自啟服務，Droppen 仍以你的使用者身分執行，sudo 僅用於註冊與管理系統服務。",
    "macOS では launchd ユーザーエージェント、Linux では systemd ユーザーサービスとしてバックグラウンド実行されます。Linux で --system を指定するとブート時自動起動サービスとして登録されますが、プロセスは通常ユーザー権限のまま動作します（sudo はサービス登録・管理のみに使用）。",
  ],
  ["Network", "网络", "網路", "ネットワーク"],
  [
    "Listens only on 127.0.0.1 and connects out to Droppen, so there is no inbound port to open. A command-line install uses port 8700, or the next free port.",
    "仅在本地 127.0.0.1 监听，并通过加密出站连接接入 Droppen，无需在防火墙开放任何入站端口或配置内网穿透。默认使用本地 8700 端口（冲突时自动顺延）。",
    "僅在本地 127.0.0.1 監聽，並透過加密出站連線接入 Droppen，無需在防火牆開放任何入站連接埠或設定內網穿透。預設使用本地 8700 連接埠（衝突時自動順延）。",
    "ローカルの 127.0.0.1 のみでリッスンし、Droppen へはセキュアなアウトバウンド接続で接続するため、インバウンドポートの開放やポート転送は一切不要です。CLI 版は 8700 番ポート（衝突時は空いている次のポート）を使用します。",
  ],
  [
    "End-to-end encrypted between your devices: neither Droppen Hub nor the Droppen relays hold a key that can decrypt your sessions.",
    "在你的多台设备之间全程端到端加密：Droppen Hub 及中继节点均不持有能够解密你的会话数据的私钥。",
    "在你的多台裝置之間全程端對端加密：Droppen Hub 及中繼節點均不持有能夠解密你的工作階段資料的私鑰。",
    "デバイス間は完全にエンドツーエンドで暗号化されます。Droppen Hub もリレーサーバーも、セッションやターミナルを復号できる鍵を保持しません。",
  ],

  ["Download for Mac", "下载 Mac 版", "下載 Mac 版", "Mac 版をダウンロード"],
  [
    "macOS 13 or later · Apple silicon and Intel",
    "需要 macOS 13 及以上 · 支持 Apple 芯片和 Intel",
    "需要 macOS 13 以上 · 支援 Apple 晶片和 Intel",
    "macOS 13 以降 · Apple シリコンと Intel に対応",
  ],
  ["macOS 13 or later", "macOS 13 或更高版本", "macOS 13 或更新版本", "macOS 13 以降"],
  ["checksums and other files", "校验值和其他文件", "校驗值和其他檔案", "チェックサムとほかのファイル"],
  ["Or download it:", "也可以直接下载：", "也可以直接下載：", "直接ダウンロードもできます："],
  [
    "Paste it into a terminal on this device. Droppen installs, starts, and opens its page.",
    "在终端中粘贴运行。Droppen 会自动完成安装、启动并打开本地控制台页面。",
    "在終端機中貼上執行。Droppen 會自動完成安裝、啟動並打開本地主控台頁面。",
    "ターミナルに貼り付けて実行すると、Droppen が自動的にインストールされ、起動してローカル管理画面が開きます。",
  ],
  ["A server with no browser?", "无图形界面的服务器？", "無圖形介面的伺服器？", "ブラウザのないサーバーなら？"],
  ["Get Droppen:", "获取 Droppen：", "取得 Droppen：", "Droppen を入手："],
  [
    "On your phone, Droppen works in the browser. Open {{app}} and add it to your home screen. Your AI runs on a Mac or a Linux device where Droppen is installed.",
    "手机端直接在浏览器中打开 {{app}}，并可添加到主屏幕。Agent 在已安装 Droppen 的 Mac 或 Linux 设备上运行。",
    "手機端直接在瀏覽器中打開 {{app}}，並可加入主畫面。Agent 在已安裝 Droppen 的 Mac 或 Linux 裝置上執行。",
    "スマホでは、Droppen はブラウザで直接使えます。{{app}} を開いてホーム画面に追加してください。エージェントは Mac や Linux 側で動作し続けます。",
  ],
  ["how to set it up", "查看设置指南", "查看設定指南", "設定のしかた"],
];

export const downloadZhCN = columnOf(LINES, 1);
export const downloadZhTW = columnOf(LINES, 2);
export const downloadJa = columnOf(LINES, 3);
