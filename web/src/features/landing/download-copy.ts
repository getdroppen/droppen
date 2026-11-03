/*
 * The download page's words (`features/landing/download.tsx`), and the shared
 * ones the home page's hero draws from this table —
 * merged into the landing page's tables (`copy.ts`), so the landing test
 * holds them to the same rules: keys are the English text, zh-CN is written
 * as native copy first, zh-TW and ja say the same things, and every claim is
 * one the build makes good on (or, behind `APP_AVAILABLE` /
 * `APP_SIGN_IN_AVAILABLE` / `LINUX_AVAILABLE`, one it will once those are on).
 *
 * The page is one task in order — the macOS client, the server daemon, using
 * it from any other device, then maintenance — so the words follow that
 * order. It calls what the AI runs on a device: 设备 / 裝置 / デバイス,
 * 「我的设备」, 「我的裝置」, 「マイデバイス」 (`test/ui-wording.test.ts`), and
 * names a platform (Mac, a Linux server, Windows) where that is the point.
 * Buttons are named exactly as the app names them (「登录 Droppen」,
 * 「设置 → 访问 → Droppen」, 「断开这台设备」, 「显示一次性注册码」).
 */

type Bundle = Record<string, string>;

export const downloadZhCN: Bundle = {
  "Download Droppen": "下载 Droppen",

  "Install Droppen where Claude Code or Codex runs: the Mac app, or one command on Linux, macOS, or WSL. Your phone needs only the browser.":
    "在运行 Claude Code 或 Codex 的设备上安装 Droppen：Mac 应用，或 Linux、macOS、WSL 上的一条命令。手机只需浏览器。",

  // Platform names: the home page's picker, the download page's segment
  "This device": "这台设备",
  "Mac": "Mac",
  "Linux": "Linux",
  "Windows / WSL": "Windows / WSL",
  "Windows": "Windows",
  "Linux server": "Linux 服务器",
  "Phone / tablet": "手机 / 平板",
  "Download": "下载",
  "copy": "复制",
  "copy the command": "复制命令",
  "Recommended": "推荐",
  "macOS": "macOS",
  "Browser": "浏览器",
  "Other platforms": "其他平台",
  "Coming soon": "敬请期待",

  // Coming-soon states
  "The App is coming soon": "App 即将推出",
  "Until it is out, run Droppen in the background on your Mac: one line, and the same sign-in.":
    "App 推出之前，可以先以后台服务在 Mac 上运行 Droppen：只需一条命令，登录方式相同。",
  "Linux support, with no browser needed on the server, is on its way. Until then, run Droppen on a Mac.":
    "Linux 支持（服务器上无需浏览器）尚未推出。在此之前，可以在 Mac 上运行 Droppen。",

  // Mac
  "Droppen for Mac": "Droppen · Mac 版",

  "Download for macOS": "下载 macOS 版",

  "Open the .dmg and drag Droppen to Applications.": "打开 .dmg，把 Droppen 拖进「应用程序」。",

  "Open Droppen": "打开 Droppen",
  "It opens a short setup guide, four steps.": "它会打开一个四步的设置向导。",

  "Using it from your phone: signing in to Droppen from the app is coming in an update. Until then, the app runs Droppen on this Mac.":
    "在手机上使用：在 App 内登录 Droppen 的功能将在后续更新中提供。在此之前，App 仅在这台 Mac 上运行 Droppen。",
  "End-to-end encrypted": "端到端加密",
  "Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer":
    "Mac 上的 Droppen：Claude Code 和 Codex 的会话列表，以及一个等你回答的会话",

  "Droppen on a phone: what needs you, and a session's question": "手机上的 Droppen：「需要你」清单，以及一个会话的提问",


  "Paste it into Terminal on that Mac.": "在这台 Mac 的「终端」中粘贴并运行。",

  "Sign in to Droppen": "登录 Droppen",

  // Linux
  "Server": "服务器",
  "Run the server": "运行服务器",
  "Linux · x86_64 or aarch64 · glibc 2.28+ · systemd": "Linux · x86_64 或 aarch64 · glibc 2.28+ · systemd",
  "Windows · WSL 2": "Windows · WSL 2",
  "Get a one-time code": "获取一次性注册码",
  "In my devices, add a device and show its one-time code. It works once, for 10 minutes.":
    "在「我的设备」中添加设备，并显示一次性注册码。10 分钟内有效，仅可使用一次。",
  "Install it": "安装",
  "Replace <code> with the one-time code.": "把 <注册码> 替换为一次性注册码。",
  "<code>": "<注册码>",

  "Check it is online": "确认在线",
  "It shows as online in my devices. If the script asks, run this so it stays up after you log out.":
    "在「我的设备」中会显示为在线。如果脚本提示，请运行下面的命令，使服务在退出登录后继续运行。",
  "It shows as online in my devices.": "在「我的设备」中会显示为在线。",
  "How to turn on systemd": "如何开启 systemd",
  "This browser reports Linux on {{arch}}.": "当前浏览器运行在 {{arch}} 架构的 Linux 上。",
  "Droppen's Linux builds are for x86_64 and aarch64.": "Droppen 的 Linux 版本支持 x86_64 和 aarch64。",

  // Windows / WSL
  "Once Linux is supported: a Linux server, or WSL.": "Linux 支持推出后：可使用 Linux 服务器或 WSL。",

  // Phone / tablet

  "Sign in": "登录",
  "Sign in with the account your other devices use.": "使用你的其他设备所用的账户登录。",
  "Approve this browser": "批准这个浏览器",
  "On a device already in your account, open my devices → add a browser or phone, then scan the QR code this browser shows, or enter its number code.":
    "在已加入账户的设备上，打开「我的设备 → 添加浏览器或手机」，扫描此浏览器显示的二维码，或输入数字码。",
  "Use it on any other device": "在手机或其他设备上使用",
  "Nothing to install: any browser works.": "无需安装：任何浏览器都能用。",


  // Maintenance
  "Manage your installation": "管理安装",
  "Update, uninstall, or check what you installed.": "更新、卸载，或核对安装的内容。",
  "Update": "更新",
  "The Mac app updates itself. For a command-line install, run the install command again: it upgrades in place and keeps your data.":
    "Mac 应用会自动更新。命令行安装的设备，再次运行安装命令即可原地升级，数据保留。",
  "Run the install command again: it upgrades in place and keeps your data.":
    "再次运行安装命令即可原地升级，数据保留。",
  "Uninstall": "卸载",
  "Remove it from your account": "从账户中移除",
  "On {{app}}, open my devices and remove the device. Every key it held is replaced, so it cannot decrypt anything written afterwards.":
    "在 {{app}} 的「我的设备」中移除这台设备。它持有过的所有密钥都会更换，之后写入的内容它无法解密。",
  "Disconnect it": "在设备上断开连接",
  "On that device, open Droppen's Settings → Access → Droppen → Disconnect this device. This deletes the Droppen keys stored on it. Removing the device from your account leaves them in place.":
    "在那台设备上打开 Droppen 的「设置 → 访问 → Droppen → 断开这台设备」。这会删除该设备上保存的 Droppen 密钥。仅从账户中移除设备，这些密钥仍会保留。",
  "Remove the program": "删除程序",
  "Mac app: quit Droppen and drag it from Applications to the Trash.": "Mac 应用：退出 Droppen，将它从「应用程序」文件夹拖到废纸篓。",
  "Command-line install (Mac or Linux): run this on that device, as the user who installed it:":
    "命令行安装（Mac 或 Linux）：在那台设备上，以安装时的用户运行：",
  "Run this on that device, as the user who installed it:": "在那台设备上，以安装时的用户运行：",
  "It stops and removes the service and the program, keeps your data, and shows where the data is and how to delete it.":
    "该命令会停止并移除服务和程序，保留数据，最后显示数据所在的位置和删除方法。",
  "Checks & connection details": "核对与连接详情",
  "Check the file before you open it": "打开前先核对文件",
  "Before you open the .dmg or extract the .tar.gz, check it in a terminal:": "打开 .dmg 或解压 .tar.gz 之前，先在终端中核对文件：",
  "The checksum must match that file's line in the release's SHA256SUMS. If it does not, do not install the file. The install script runs the same check first.":
    "校验值必须与发布页 SHA256SUMS 中该文件所在行一致。不一致时请勿安装。安装脚本在安装前也会执行同样的核对。",
  "Build provenance": "构建来源证明",
  "From v0.2.127, every file has a build provenance attestation. With the GitHub CLI, check that this repository's release workflow built it:":
    "从 v0.2.127 起，每个文件都有构建来源证明。使用 GitHub CLI 可核对该文件是否由本仓库的发布工作流构建：",
  "How to verify a download": "如何校验下载文件",
  "Mac app": "Mac 应用",
  "Signed with a Developer ID and notarized by Apple. It installs only updates signed for that version.": "使用 Developer ID 签名并经 Apple 公证；只安装为对应版本签名的更新。",
  "Install script": "安装脚本",
  "Runs as you, never as root, and checks the release's SHA-256 checksums before it installs anything.": "以当前用户身份运行，从不使用 root；安装前先核对发布文件的 SHA-256 校验值。",
  "Background service": "后台服务",
  "A command-line install runs as a launchd user agent on macOS, or a systemd user service on Linux. On Linux, --system makes it a system service that starts at boot and still runs as you; sudo only sets up and manages that service.":
    "命令行安装：macOS 上以 launchd 用户代理运行，Linux 上以 systemd 用户服务运行。在 Linux 上加 --system 会成为开机启动的系统服务，Droppen 仍以你的用户身份运行；sudo 仅用于配置和管理该服务。",
  "Network": "网络",
  "Listens only on 127.0.0.1 and connects out to Droppen, so there is no inbound port to open. A command-line install uses port 8700, or the next free port.":
    "只在 127.0.0.1 上监听，通过出站连接接入 Droppen，无需开放入站端口。命令行安装使用 8700 端口，被占用时改用下一个空闲端口。",
  "Encryption": "加密",
  "End-to-end encrypted between your devices: neither Droppen Hub nor the Droppen relays hold a key that can decrypt your sessions.":
    "在你的设备之间端到端加密：Droppen Hub 和 Droppen 中继都不持有能解密你的会话的密钥。",
  "How the encryption works": "加密是怎么做的",

  // The home page's hero (shared)
  "Download for Mac": "下载 Mac 版",
  "macOS 13 or later · Apple silicon and Intel": "需要 macOS 13 及以上 · 支持 Apple 芯片和 Intel",
  "checksums and other files": "校验值和其他文件",
  "Or download it:": "也可以直接下载：",
  "Paste it into a terminal on this device: Droppen installs, starts and opens its page.": "在这台设备的终端里粘贴运行：Droppen 会自动安装、启动，并打开它的页面。",
  "A server with no browser?": "服务器上没有浏览器？",
  "Get Droppen:": "获取 Droppen：",
  "On your phone, Droppen works in the browser: open {{app}} and add it to your home screen. Your AI runs on a Mac or on Linux, with Droppen installed there.":
    "在手机上，Droppen 直接在浏览器里用：打开 {{app}} 并添加到主屏幕。AI 保持在已安装 Droppen 的 Mac 或 Linux 设备上运行。",
  "how to set it up": "查看设置指南",
};

export const downloadZhTW: Bundle = {
  "Download Droppen": "下載 Droppen",

  "Install Droppen where Claude Code or Codex runs: the Mac app, or one command on Linux, macOS, or WSL. Your phone needs only the browser.":
    "在執行 Claude Code 或 Codex 的裝置上安裝 Droppen：Mac 應用程式，或 Linux、macOS、WSL 上的一行指令。手機只需瀏覽器。",

  "This device": "這台裝置",
  "Mac": "Mac",
  "Linux": "Linux",
  "Windows / WSL": "Windows / WSL",
  "Windows": "Windows",
  "Linux server": "Linux 伺服器",
  "Phone / tablet": "手機 / 平板",
  "Download": "下載",
  "copy": "複製",
  "copy the command": "複製指令",
  "Recommended": "推薦",
  "macOS": "macOS",
  "Browser": "瀏覽器",
  "Other platforms": "其他平台",
  "Coming soon": "敬請期待",

  "The App is coming soon": "App 即將推出",
  "Until it is out, run Droppen in the background on your Mac: one line, and the same sign-in.":
    "App 推出之前，可以先以背景服務在 Mac 上執行 Droppen：只需一行指令，登入方式相同。",
  "Linux support, with no browser needed on the server, is on its way. Until then, run Droppen on a Mac.":
    "Linux 支援（伺服器上無需瀏覽器）尚未推出。在此之前，可以在 Mac 上執行 Droppen。",

  "Droppen for Mac": "Droppen · Mac 版",

  "Download for macOS": "下載 macOS 版",

  "Open the .dmg and drag Droppen to Applications.": "打開 .dmg，把 Droppen 拖進「應用程式」。",

  "Open Droppen": "打開 Droppen",
  "It opens a short setup guide, four steps.": "它會打開一個四步的設定引導。",

  "Using it from your phone: signing in to Droppen from the app is coming in an update. Until then, the app runs Droppen on this Mac.":
    "在手機上使用：在 App 內登入 Droppen 的功能將在後續更新中提供。在此之前，App 僅在這台 Mac 上執行 Droppen。",
  "End-to-end encrypted": "端對端加密",
  "Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer":
    "Mac 上的 Droppen：Claude Code 和 Codex 的工作階段清單，以及一個等你回答的工作階段",

  "Droppen on a phone: what needs you, and a session's question": "手機上的 Droppen：「需要你」清單，以及一個工作階段的提問",


  "Paste it into Terminal on that Mac.": "在這台 Mac 的「終端機」中貼上並執行。",

  "Sign in to Droppen": "登入 Droppen",

  "Server": "伺服器",
  "Run the server": "執行伺服器",
  "Linux · x86_64 or aarch64 · glibc 2.28+ · systemd": "Linux · x86_64 或 aarch64 · glibc 2.28+ · systemd",
  "Windows · WSL 2": "Windows · WSL 2",
  "Get a one-time code": "取得一次性註冊碼",
  "In my devices, add a device and show its one-time code. It works once, for 10 minutes.":
    "在「我的裝置」新增裝置並顯示一次性註冊碼。10 分鐘內有效，僅可使用一次。",
  "Install it": "安裝",
  "Replace <code> with the one-time code.": "把 <註冊碼> 替換為一次性註冊碼。",
  "<code>": "<註冊碼>",

  "Check it is online": "確認在線",
  "It shows as online in my devices. If the script asks, run this so it stays up after you log out.":
    "「我的裝置」中會顯示為在線。如果指令碼提示，請執行下面的指令，讓服務在登出後繼續執行。",
  "It shows as online in my devices.": "「我的裝置」中會顯示為在線。",
  "How to turn on systemd": "如何開啟 systemd",
  "This browser reports Linux on {{arch}}.": "目前瀏覽器執行在 {{arch}} 架構的 Linux 上。",
  "Droppen's Linux builds are for x86_64 and aarch64.": "Droppen 的 Linux 版本支援 x86_64 和 aarch64。",

  "Once Linux is supported: a Linux server, or WSL.": "Linux 支援推出後：可使用 Linux 伺服器或 WSL。",


  "Sign in": "登入",
  "Sign in with the account your other devices use.": "使用你的其他裝置所用的帳戶登入。",
  "Approve this browser": "核准這個瀏覽器",
  "On a device already in your account, open my devices → add a browser or phone, then scan the QR code this browser shows, or enter its number code.":
    "在已加入帳戶的裝置上，打開「我的裝置 → 新增瀏覽器或手機」，掃描此瀏覽器顯示的 QR 碼，或輸入數字碼。",
  "Use it on any other device": "在手機或其他裝置上使用",
  "Nothing to install: any browser works.": "無需安裝：任何瀏覽器都能用。",


  "Manage your installation": "管理安裝",
  "Update, uninstall, or check what you installed.": "更新、解除安裝，或核對安裝的內容。",
  "Update": "更新",
  "The Mac app updates itself. For a command-line install, run the install command again: it upgrades in place and keeps your data.":
    "Mac 應用程式會自動更新。以命令列安裝的裝置，再次執行安裝指令即可原地升級，資料保留。",
  "Run the install command again: it upgrades in place and keeps your data.":
    "再次執行安裝指令即可原地升級，資料保留。",
  "Uninstall": "解除安裝",
  "Remove it from your account": "從帳戶中移除",
  "On {{app}}, open my devices and remove the device. Every key it held is replaced, so it cannot decrypt anything written afterwards.":
    "在 {{app}} 的「我的裝置」中移除這台裝置。它持有過的所有金鑰都會更換，之後寫入的內容它無法解密。",
  "Disconnect it": "在裝置上中斷連線",
  "On that device, open Droppen's Settings → Access → Droppen → Disconnect this device. This deletes the Droppen keys stored on it. Removing the device from your account leaves them in place.":
    "在那台裝置上打開 Droppen 的「設定 → 存取 → Droppen → 中斷這台裝置」。這會刪除該裝置上儲存的 Droppen 金鑰。僅從帳戶中移除裝置，這些金鑰仍會保留。",
  "Remove the program": "刪除程式",
  "Mac app: quit Droppen and drag it from Applications to the Trash.": "Mac 應用程式：結束 Droppen，將它從「應用程式」資料夾拖到垃圾桶。",
  "Command-line install (Mac or Linux): run this on that device, as the user who installed it:":
    "命令列安裝（Mac 或 Linux）：在那台裝置上，以安裝時的使用者執行：",
  "Run this on that device, as the user who installed it:": "在那台裝置上，以安裝時的使用者執行：",
  "It stops and removes the service and the program, keeps your data, and shows where the data is and how to delete it.":
    "該指令會停止並移除服務和程式，保留資料，最後顯示資料所在的位置和刪除方法。",
  "Checks & connection details": "核對與連線詳情",
  "Check the file before you open it": "開啟前先核對檔案",
  "Before you open the .dmg or extract the .tar.gz, check it in a terminal:": "打開 .dmg 或解壓 .tar.gz 之前，先在終端機中核對檔案：",
  "The checksum must match that file's line in the release's SHA256SUMS. If it does not, do not install the file. The install script runs the same check first.":
    "校驗值必須與發布頁 SHA256SUMS 中該檔案所在行一致。不一致時請勿安裝。安裝腳本在安裝前也會執行同樣的核對。",
  "Build provenance": "建置來源證明",
  "From v0.2.127, every file has a build provenance attestation. With the GitHub CLI, check that this repository's release workflow built it:":
    "從 v0.2.127 起，每個檔案都有建置來源證明。使用 GitHub CLI 可核對該檔案是否由本儲存庫的發布工作流程建置：",
  "How to verify a download": "如何校驗下載檔案",
  "Mac app": "Mac 應用程式",
  "Signed with a Developer ID and notarized by Apple. It installs only updates signed for that version.": "使用 Developer ID 簽署並經 Apple 公證；只安裝為對應版本簽署的更新。",
  "Install script": "安裝腳本",
  "Runs as you, never as root, and checks the release's SHA-256 checksums before it installs anything.": "以目前使用者身分執行，從不使用 root；安裝前先核對發布檔案的 SHA-256 校驗值。",
  "Background service": "背景服務",
  "A command-line install runs as a launchd user agent on macOS, or a systemd user service on Linux. On Linux, --system makes it a system service that starts at boot and still runs as you; sudo only sets up and manages that service.":
    "命令列安裝：macOS 上以 launchd 使用者代理執行，Linux 上以 systemd 使用者服務執行。在 Linux 上加 --system 會成為開機啟動的系統服務，Droppen 仍以你的使用者身分執行；sudo 僅用於設定和管理該服務。",
  "Network": "網路",
  "Listens only on 127.0.0.1 and connects out to Droppen, so there is no inbound port to open. A command-line install uses port 8700, or the next free port.":
    "只在 127.0.0.1 上監聽，透過對外連線接入 Droppen，無需開放對內連接埠。命令列安裝使用 8700 連接埠，被佔用時改用下一個空閒連接埠。",
  "Encryption": "加密",
  "End-to-end encrypted between your devices: neither Droppen Hub nor the Droppen relays hold a key that can decrypt your sessions.":
    "在你的裝置之間端對端加密：Droppen Hub 和 Droppen 中繼都不持有能解密你的工作階段的金鑰。",
  "How the encryption works": "加密是怎麼做的",

  "Download for Mac": "下載 Mac 版",
  "macOS 13 or later · Apple silicon and Intel": "需要 macOS 13 以上 · 支援 Apple 晶片和 Intel",
  "checksums and other files": "校驗值和其他檔案",
  "Or download it:": "也可以直接下載：",
  "Paste it into a terminal on this device: Droppen installs, starts and opens its page.": "在這台裝置的終端機裡貼上執行：Droppen 會自動安裝、啟動，並打開它的頁面。",
  "A server with no browser?": "伺服器上沒有瀏覽器？",
  "Get Droppen:": "取得 Droppen：",
  "On your phone, Droppen works in the browser: open {{app}} and add it to your home screen. Your AI runs on a Mac or on Linux, with Droppen installed there.":
    "在手機上，Droppen 直接在瀏覽器裡用：打開 {{app}}，加入主畫面。AI 跑在 Mac 或 Linux 上，Droppen 裝在那裡。",
  "how to set it up": "怎麼設定",
};

export const downloadJa: Bundle = {
  "Download Droppen": "Droppen をダウンロード",

  "Install Droppen where Claude Code or Codex runs: the Mac app, or one command on Linux, macOS, or WSL. Your phone needs only the browser.":
    "Claude Code か Codex が動くデバイスに Droppen をインストール。Mac アプリか、Linux・macOS・WSL のコマンド 1 行で。スマホはブラウザだけで使えます。",

  "This device": "このデバイス",
  "Mac": "Mac",
  "Linux": "Linux",
  "Windows / WSL": "Windows / WSL",
  "Windows": "Windows",
  "Linux server": "Linux サーバー",
  "Phone / tablet": "スマホ / タブレット",
  "Download": "ダウンロード",
  "copy": "コピー",
  "copy the command": "コマンドをコピー",
  "Recommended": "おすすめ",
  "macOS": "macOS",
  "Browser": "ブラウザ",
  "Other platforms": "その他のプラットフォーム",
  "Coming soon": "近日公開",

  "The App is coming soon": "アプリは近日公開",
  "Until it is out, run Droppen in the background on your Mac: one line, and the same sign-in.":
    "公開までは、Mac のバックグラウンドサービスとして Droppen を動かせます。コマンドは 1 行、サインインの手順は同じです。",
  "Linux support, with no browser needed on the server, is on its way. Until then, run Droppen on a Mac.":
    "Linux 対応（サーバーにブラウザ不要）はまだ提供していません。それまでは Mac で Droppen を動かせます。",

  "Droppen for Mac": "Droppen · Mac 版",

  "Download for macOS": "macOS 版をダウンロード",

  "Open the .dmg and drag Droppen to Applications.": ".dmg を開き、Droppen を「アプリケーション」にドラッグします。",

  "Open Droppen": "Droppen を開く",
  "It opens a short setup guide, four steps.": "4 ステップの短いセットアップガイドが開きます。",

  "Using it from your phone: signing in to Droppen from the app is coming in an update. Until then, the app runs Droppen on this Mac.":
    "スマホから使う場合：アプリからの Droppen へのサインインは、今後のアップデートで対応します。それまでは、アプリはこの Mac でのみ Droppen を動かします。",
  "End-to-end encrypted": "エンドツーエンド暗号化",
  "Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer":
    "Mac の Droppen：Claude Code と Codex のセッション一覧と、回答を待つセッション",

  "Droppen on a phone: what needs you, and a session's question": "スマホの Droppen：「対応待ち」リストとセッションの質問",


  "Paste it into Terminal on that Mac.": "その Mac の「ターミナル」に貼り付けて実行します。",

  "Sign in to Droppen": "Droppen にサインイン",

  "Server": "サーバー",
  "Run the server": "サーバーを動かす",
  "Linux · x86_64 or aarch64 · glibc 2.28+ · systemd": "Linux · x86_64 または aarch64 · glibc 2.28+ · systemd",
  "Windows · WSL 2": "Windows · WSL 2",
  "Get a one-time code": "ワンタイムコードを取得",
  "In my devices, add a device and show its one-time code. It works once, for 10 minutes.":
    "「マイデバイス」でデバイスを追加し、ワンタイムコードを表示します。有効期限は 10 分、使用は 1 回だけです。",
  "Install it": "インストール",
  "Replace <code> with the one-time code.": "<コード> をワンタイムコードに置き換えます。",
  "<code>": "<コード>",

  "Check it is online": "オンラインを確認",
  "It shows as online in my devices. If the script asks, run this so it stays up after you log out.":
    "「マイデバイス」にオンラインとして表示されます。スクリプトに求められたら、ログアウト後も動き続けるよう次のコマンドを実行します。",
  "It shows as online in my devices.": "「マイデバイス」にオンラインとして表示されます。",
  "How to turn on systemd": "systemd を有効にする方法",
  "This browser reports Linux on {{arch}}.": "このブラウザは {{arch}} の Linux で動いています。",
  "Droppen's Linux builds are for x86_64 and aarch64.": "Droppen の Linux 版は x86_64 と aarch64 に対応しています。",

  "Once Linux is supported: a Linux server, or WSL.": "Linux 対応後は、Linux サーバーか WSL で。",


  "Sign in": "サインイン",
  "Sign in with the account your other devices use.": "ほかのデバイスと同じアカウントでサインインします。",
  "Approve this browser": "このブラウザを承認",
  "On a device already in your account, open my devices → add a browser or phone, then scan the QR code this browser shows, or enter its number code.":
    "このアカウントの既存のデバイスで「マイデバイス → ブラウザかスマートフォンを追加」を開き、このブラウザに表示された QR コードを読み取るか、数字コードを入力します。",
  "Use it on any other device": "スマホなどほかのデバイスで使う",
  "Nothing to install: any browser works.": "インストール不要。どのブラウザでも使えます。",


  "Manage your installation": "インストールの管理",
  "Update, uninstall, or check what you installed.": "更新、アンインストール、インストール内容の確認ができます。",
  "Update": "更新",
  "The Mac app updates itself. For a command-line install, run the install command again: it upgrades in place and keeps your data.":
    "Mac アプリは自動でアップデートします。コマンドラインでインストールした場合は、インストールコマンドをもう一度実行すると、データを残したままアップデートされます。",
  "Run the install command again: it upgrades in place and keeps your data.":
    "インストールコマンドをもう一度実行すると、データを残したままアップデートされます。",
  "Uninstall": "アンインストール",
  "Remove it from your account": "アカウントから削除する",
  "On {{app}}, open my devices and remove the device. Every key it held is replaced, so it cannot decrypt anything written afterwards.":
    "{{app}} の「マイデバイス」でそのデバイスを削除します。そのデバイスが持っていた鍵はすべて新しいものに替わり、以後に書かれた内容は復号できません。",
  "Disconnect it": "デバイスで接続を解除する",
  "On that device, open Droppen's Settings → Access → Droppen → Disconnect this device. This deletes the Droppen keys stored on it. Removing the device from your account leaves them in place.":
    "そのデバイスで Droppen の「設定 → アクセス → Droppen → このデバイスの接続を解除」を選ぶと、デバイスに保存された Droppen の鍵が削除されます。アカウントからデバイスを削除するだけでは、鍵は残ります。",
  "Remove the program": "プログラムを削除する",
  "Mac app: quit Droppen and drag it from Applications to the Trash.": "Mac アプリ：Droppen を終了し、「アプリケーション」フォルダからゴミ箱にドラッグします。",
  "Command-line install (Mac or Linux): run this on that device, as the user who installed it:":
    "コマンドラインでのインストール（Mac または Linux）：そのデバイスで、インストールしたユーザーとして実行します：",
  "Run this on that device, as the user who installed it:": "そのデバイスで、インストールしたユーザーとして実行します：",
  "It stops and removes the service and the program, keeps your data, and shows where the data is and how to delete it.":
    "サービスとプログラムを停止して削除し、データは残したまま、データの場所と削除方法を表示します。",
  "Checks & connection details": "確認と接続の詳細",
  "Check the file before you open it": "開く前にファイルを確認する",
  "Before you open the .dmg or extract the .tar.gz, check it in a terminal:": ".dmg を開く前、または .tar.gz を展開する前に、ターミナルでファイルを確認します：",
  "The checksum must match that file's line in the release's SHA256SUMS. If it does not, do not install the file. The install script runs the same check first.":
    "チェックサムが、リリースの SHA256SUMS にあるそのファイルの行と一致することを確かめます。一致しない場合は、そのファイルをインストールしないでください。インストールスクリプトも、インストール前に同じ確認をします。",
  "Build provenance": "ビルドの来歴",
  "From v0.2.127, every file has a build provenance attestation. With the GitHub CLI, check that this repository's release workflow built it:":
    "v0.2.127 以降、すべてのファイルにビルド来歴の証明が付きます。GitHub CLI で、このリポジトリのリリースワークフローがビルドしたことを確認します：",
  "How to verify a download": "ダウンロードの検証方法",
  "Mac app": "Mac アプリ",
  "Signed with a Developer ID and notarized by Apple. It installs only updates signed for that version.":
    "Developer ID で署名され、Apple の公証を受けています。そのバージョン向けに署名された更新だけをインストールします。",
  "Install script": "インストールスクリプト",
  "Runs as you, never as root, and checks the release's SHA-256 checksums before it installs anything.":
    "実行したユーザーとして動き、root では動きません。インストール前にリリースの SHA-256 チェックサムを照合します。",
  "Background service": "バックグラウンドサービス",
  "A command-line install runs as a launchd user agent on macOS, or a systemd user service on Linux. On Linux, --system makes it a system service that starts at boot and still runs as you; sudo only sets up and manages that service.":
    "コマンドラインでのインストールは、macOS では launchd のユーザーエージェント、Linux では systemd のユーザーサービスとして動きます。Linux で --system を付けると起動時に開始するシステムサービスになり、その場合も自分のユーザーで動きます。sudo はそのサービスの設定と管理にだけ使います。",
  "Network": "ネットワーク",
  "Listens only on 127.0.0.1 and connects out to Droppen, so there is no inbound port to open. A command-line install uses port 8700, or the next free port.":
    "127.0.0.1 でのみ待ち受け、Droppen には外向きの接続でつながるため、開ける必要のある受信ポートはありません。コマンドラインでのインストールではポート 8700 を使い、使用中なら次の空きポートを使います。",
  "Encryption": "暗号化",
  "End-to-end encrypted between your devices: neither Droppen Hub nor the Droppen relays hold a key that can decrypt your sessions.":
    "デバイス間でエンドツーエンド暗号化されます。Droppen Hub も Droppen リレーも、セッションを復号できる鍵を持っていません。",
  "How the encryption works": "暗号化のしくみ",

  "Download for Mac": "Mac 版をダウンロード",
  "macOS 13 or later · Apple silicon and Intel": "macOS 13 以降 · Apple シリコンと Intel に対応",
  "checksums and other files": "チェックサムとほかのファイル",
  "Or download it:": "直接ダウンロードもできます：",
  "Paste it into a terminal on this device: Droppen installs, starts and opens its page.": "このデバイスのターミナルに貼り付けて実行すると、Droppen がインストールされ、起動してページが開きます。",
  "A server with no browser?": "ブラウザのないサーバーなら？",
  "Get Droppen:": "Droppen を入手：",
  "On your phone, Droppen works in the browser: open {{app}} and add it to your home screen. Your AI runs on a Mac or on Linux, with Droppen installed there.":
    "スマホでは、Droppen はブラウザで使います。{{app}} を開いてホーム画面に追加してください。AI は Mac か Linux で動かし、Droppen はそちらに入れます。",
  "how to set it up": "設定のしかた",
};
