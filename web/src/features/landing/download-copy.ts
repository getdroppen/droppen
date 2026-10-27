/*
 * The download page's words (`download.tsx`), and the footer's way to it —
 * merged into the landing page's tables (`copy.ts`), so the landing test
 * holds them to the same rules: keys are the English text, zh-CN is written
 * as native copy first, zh-TW and ja say the same things, and every claim is
 * one the build makes good on (or, behind `APP_AVAILABLE` /
 * `APP_SIGN_IN_AVAILABLE` / `LINUX_AVAILABLE`, one it will once those are on).
 *
 * The voice is the landing page's: lead with what the reader gets (their
 * AI on their phone), then the proof and the how. Unlike the landing page's
 * selling points, this page is the product's own words, so it calls what the
 * AI runs on what the app calls it — a device: 设备 / 裝置 / デバイス,
 * 「我的设备」, 「我的裝置」, 「マイデバイス」 (`test/ui-wording.test.ts`) — and
 * names a platform (Mac, a Linux server, Windows) where that is the point;
 * the buttons are named exactly as the app names them (「登录 Droppen」,
 * 「设置 → 访问 → Droppen」, 「断开这台设备」, 「显示一次性注册码」).
 */

type Bundle = Record<string, string>;

export const downloadZhCN: Bundle = {
  "Download Droppen": "下载 Droppen",
  "Install Droppen on the Mac or Linux device where Claude Code or Codex runs: the Mac app, or one command. Your phone needs only the browser.":
    "在运行 Claude Code 或 Codex 的 Mac 或 Linux 设备上安装 Droppen：Mac 应用，或一条命令。手机只需浏览器。",

  // Hero
  "Download": "下载",
  "Install Droppen on the device that runs your AI.": "在运行 AI 的设备上安装 Droppen。",
  "Droppen goes on the Mac or Linux device where Claude Code or Codex runs, alongside them. Your phone and other devices need nothing installed: open {{app}} in the browser to see every session, answer your AI's questions, steer it and open a terminal.":
    "Droppen 安装在运行 Claude Code 或 Codex 的 Mac 或 Linux 设备上，与它们一同运行。手机和其他设备无需安装：在浏览器中打开 {{app}}，即可查看所有会话、回答 AI 的提问、发送指令和打开终端。",
  "A Droppen account. Sign-up needs an invite code.": "一个 Droppen 账户。注册需要邀请码。",
  "How to install on a Mac": "在 Mac 上的安装方式",

  // The tabs
  "Choose your system": "选择你的系统",
  "Linux": "Linux",
  "Linux server": "Linux 服务器",
  "Windows": "Windows",
  "Phone / tablet": "手机 / 平板",
  "Detected": "当前设备",

  // Mac · App
  "Droppen for Mac": "Mac 版 Droppen",
  "Open the app and Droppen runs on this Mac, with no terminal and no local password. To use it from your phone and other devices, sign in to Droppen from the app.":
    "打开 App，Droppen 即在这台 Mac 上运行，无需终端，也无需设置本地密码。要在手机和其他设备上使用，请在 App 内登录 Droppen。",
  "Open the app and Droppen runs on this Mac, with no terminal and no local password.": "打开 App，Droppen 即在这台 Mac 上运行，无需终端，也无需设置本地密码。",
  "The App is coming soon": "App 即将推出",
  "Until it is out, run Droppen in the background on your Mac: one line, and the same sign-in.": "App 推出之前，可以先以后台服务在 Mac 上运行 Droppen：只需一条命令，登录方式相同。",
  "copy the command": "复制命令",
  "the steps for running it in the background": "查看后台运行的步骤",
  "Download Droppen App (macOS)": "下载 Droppen App（macOS）",
  "macOS 13 or later · Universal: Apple silicon and Intel · Droppen-<version>.dmg": "需要 macOS 13 及以上 · 通用版：Apple 芯片和 Intel 都能用 · Droppen-<版本>.dmg",
  "macOS 13 or later · Universal: Apple silicon and Intel": "需要 macOS 13 及以上 · 通用版：Apple 芯片和 Intel 都能用",
  "release notes, checksums and other files": "更新说明、校验值和其他文件",
  "Open the .dmg": "打开 .dmg",
  "The file you just downloaded, from Downloads.": "在「下载」文件夹中打开刚下载的文件。",
  "Drag Droppen to Applications": "把 Droppen 拖进「应用程序」",
  "Like any other Mac app.": "与安装其他 Mac 应用的方式相同。",
  "Open Droppen": "打开 Droppen",
  "It opens a short setup guide, four steps.": "它会打开一个四步的设置向导。",
  "Sign in to Droppen from the app": "在 App 内登录 Droppen",
  "In the setup guide's Where to use it step, choose “On my phone and other devices too”, then click Sign in to Droppen. Your browser opens {{app}}: sign in, check that the fingerprint matches the one the app shows, then click add this device. Later, the same is under Settings → Access → Droppen.":
    "在设置向导的「在哪里用」一步，选择「在手机和其它设备上也能用」，再点击「登录 Droppen」。浏览器会打开 {{app}}：登录后，核对指纹与 App 中显示的一致，再点击「加入这台设备」。之后也可以在「设置 → 访问 → Droppen」中登录。",
  "Using it from your phone: signing in to Droppen from the app is coming in an update. Until then, the app runs Droppen on this Mac.":
    "在手机上使用：在 App 内登录 Droppen 的功能将在后续更新中提供。在此之前，App 仅在这台 Mac 上运行 Droppen。",

  // Mac · background
  "Droppen in the background on a Mac": "在 Mac 上以后台服务运行 Droppen",
  "For a Mac that stays on as a server, or if you prefer the terminal. One command installs Droppen; you then add this Mac to your account in the browser.":
    "适用于长期开机、用作服务器的 Mac，或习惯使用终端的用户。一条命令完成安装，之后在浏览器中把这台 Mac 加入你的账户。",
  "Paste it into Terminal on that Mac.": "在这台 Mac 的「终端」中粘贴并运行。",
  "It installs and starts at login": "安装并设为登录时启动",
  "The script installs Droppen, starts it now and at every login, and creates the local page's first account under your user name. It asks you to choose a password (at least 12 characters), or generates one when run without a terminal.":
    "脚本会安装 Droppen 并立即启动，此后每次登录时自动启动；同时以你的系统用户名为本地页面创建第一个账户。脚本会请你设置密码（至少 12 个字符）；在没有终端的环境中运行时，会自动生成密码。",
  "The local page opens": "打开本地页面",
  "Your browser opens Droppen's local page. Sign in with that account: everything works there without a Droppen account. To use it from your phone, continue with the next step.":
    "浏览器会打开 Droppen 的本地页面。用上一步的账户登录，无需 Droppen 账户即可使用全部功能。要在手机上使用，请继续下一步。",
  "Sign in to Droppen": "登录 Droppen",
  "On the local page, click Sign in to Droppen. A {{app}} window opens: sign in, check that its fingerprint matches the one on the local page, then click add this device.":
    "在本地页面上点击「登录 Droppen」，会弹出 {{app}} 窗口。登录后，核对窗口中的指纹与本地页面上显示的一致，再点击「加入这台设备」。",
  "It is in my devices": "出现在「我的设备」中",
  "Within seconds it shows as online in my devices, on every phone and browser in your account. Add more devices the same way, and the sessions on all of them appear in one list.":
    "几秒钟后，这台设备会在「我的设备」中显示为在线，账户中的每部手机和每个浏览器都能看到它。按同样的步骤添加更多设备后，所有设备上的会话会汇总在同一个列表中。",

  // Linux server
  "Droppen on a Linux server": "在 Linux 服务器上运行 Droppen",
  "Run your AI on a server that stays on, and follow it and answer it from your phone. The server needs no browser: it joins your account with a one-time code.":
    "让 AI 在常开的服务器上运行，并在手机上查看进度、回答它的提问。服务器上无需浏览器，使用一次性注册码即可加入你的账户。",
  "coming": "即将支持",
  "Linux support, with no browser needed on the server, is on its way. Until then, run Droppen on a Mac.":
    "Linux 支持（服务器上无需浏览器）尚未推出。在此之前，可以在 Mac 上运行 Droppen。",
  "run it in the background on a Mac": "改为在 Mac 后台运行",
  "This browser reports Linux on {{arch}}.": "当前浏览器运行在 {{arch}} 架构的 Linux 上。",
  "Droppen's Linux builds are for x86_64 and aarch64.": "Droppen 的 Linux 版本支持 x86_64 和 aarch64。",
  "<code>": "<注册码>",
  "Replace <code> with the one-time code from step 01 below.": "把命令中的 <注册码> 替换为下方第 01 步获取的一次性注册码。",
  "Get a one-time code": "获取一次性注册码",
  "On {{app}}: my devices → add a device that runs AI → show a one-time code. It is good for 10 minutes, once.":
    "在 {{app}} 打开「我的设备 → 添加可运行 AI 的设备 → 显示一次性注册码」。注册码 10 分钟内有效，仅可使用一次。",
  "Run the command on the server": "在服务器上运行命令",
  "The script installs Droppen, starts it as a systemd user service under your own user, never as root, and joins your account with the code. If it says so at the end, run sudo loginctl enable-linger to keep it running after you log out.":
    "脚本会安装 Droppen，以你的用户身份作为 systemd 用户服务启动（不使用 root），并用注册码加入你的账户。如果脚本结束时给出提示，请执行 sudo loginctl enable-linger，使服务在退出登录后继续运行。",
  "Within seconds it shows as online in my devices, on every phone and browser in your account.": "几秒钟后，这台设备会在「我的设备」中显示为在线，账户中的每部手机和每个浏览器都能看到它。",

  // Windows
  "Droppen on Windows": "Windows 版 Droppen",
  "Droppen does not run on Windows yet. From a browser on Windows you can already follow your AI and answer its questions.":
    "Droppen 暂不支持在 Windows 上运行。目前可以在 Windows 的浏览器中查看 AI 的进度并回答它的提问。",
  "What works on Windows today:": "目前在 Windows 上可以：",
  "Control your other devices from this browser: sign in at {{app}}.": "登录 {{app}}，在这个浏览器中控制你的其他设备。",
  "Run the AI on a Mac, with the Droppen App.": "在 Mac 上运行 AI：使用 Droppen App。",
  "Run the AI on a Mac: in the background today, with the Droppen App soon.": "在 Mac 上运行 AI：目前以后台服务运行，Droppen App 即将推出。",
  "Run the AI on a Linux server, or in WSL with systemd turned on.": "在 Linux 服务器或已开启 systemd 的 WSL 中运行 AI。",
  "how to turn on systemd in WSL": "如何在 WSL 中开启 systemd",
  "Once Linux is supported: a Linux server, or WSL.": "Linux 支持推出后：可使用 Linux 服务器或 WSL。",
  "use {{app}} in this browser": "在这个浏览器中打开 {{app}}",
  "the Mac options": "Mac 安装方式",
  "the Linux server options": "Linux 服务器安装方式",

  // Phone / tablet
  "Then, use it from your phone": "安装之后，在手机上使用",
  "Open {{app}} in your phone's browser and sign in to the same account.": "在手机浏览器中打开 {{app}}，登录同一个账户。",
  "open {{app}}": "打开 {{app}}",
  "Open {{app}} on your phone": "在手机上打开 {{app}}",
  "iPhone and iPad: in Safari, tap Share, then Add to Home Screen. Android: in Chrome, tap ⋮, then Add to Home screen or Install app. Then you can turn on notifications.":
    "iPhone 和 iPad：在 Safari 中点「分享」，再点「添加到主屏幕」。Android：在 Chrome 中点 ⋮，再点「添加到主屏幕」或「安装应用」。添加后即可开启通知。",
  "Add it to the home screen": "添加到主屏幕",
  "On a device already in your account, open my devices → add a browser or phone, then scan the QR code the phone shows, or enter its number code.":
    "在已加入账户的设备上，打开「我的设备 → 添加浏览器或手机」，扫描手机上显示的二维码，或输入数字码。",
  "Install it where your AI runs": "装在跑 AI 的设备上",
  "You are on a phone. To install, open this page on the device that runs your AI:": "你正在使用手机。请在运行 AI 的设备上打开此页面进行安装：",
  "copy": "复制",

  // System requirements
  "Approve the phone": "批准这部手机",
  "What gets installed, and how to verify it": "安装了什么，以及如何核对",
  "Mac": "Mac",
  "Signed with a Developer ID and notarized by Apple. It installs only updates signed for that version.": "使用 Developer ID 签名并经 Apple 公证；只安装为对应版本签名的更新。",
  "Runs as you, never as root, and checks the release's SHA-256 checksums before it installs anything.": "以当前用户身份运行，从不使用 root；安装前先核对发布文件的 SHA-256 校验值。",
  "For a command-line install: a launchd user agent on macOS, a systemd user service on Linux. On Linux, --system makes it a system service that starts at boot and still runs as you; sudo is used only to set up and manage that service.":
    "命令行安装：macOS 上为 launchd 用户代理，Linux 上为 systemd 用户服务。在 Linux 上加 --system 可改为开机启动的系统服务，Droppen 仍以你的用户身份运行；sudo 仅用于配置和管理该服务。",
  "Listens only on 127.0.0.1 and reaches Droppen over outbound connections, so there is no inbound port to open. A command-line install uses port 8700, or the next free port.":
    "只在 127.0.0.1 上监听，通过出站连接接入 Droppen，无需开放入站端口。命令行安装使用 8700 端口，被占用时改用下一个空闲端口。",
  "Mac app (recommended)": "Mac 应用（推荐）",
  "Command line": "命令行",
  "Claude Code or Codex, installed and signed in on that device": "该设备上已安装并登录 Claude Code 或 Codex",

  // Manual download and checksums
  "Manual download": "手动下载",
  "macOS 13 or later (Apple silicon or Intel), or Linux (x86_64 or aarch64, glibc 2.28 or later)":
    "macOS 13 及以上（Apple 芯片或 Intel），或 Linux（x86_64、aarch64，glibc 2.28 及以上）",
  "A free Droppen account. You can also create one when you sign in to Droppen on the device.": "一个免费的 Droppen 账户，也可以在设备上登录 Droppen 时创建。",
  "the latest release on GitHub (getdroppen/droppen)": "GitHub 上的最新版本（getdroppen/droppen）",
  "Before you open the .dmg or extract the .tar.gz, check it in a terminal:": "打开 .dmg 或解压 .tar.gz 之前，先在终端中核对文件：",
  "The checksum it prints must match that file's line in the release's SHA256SUMS; if it does not, do not install it. The install script runs the same check before it installs anything.":
    "输出的校验值必须与发布页 SHA256SUMS 中该文件的值一致；不一致时请勿安装。安装脚本在安装前也会执行同样的核对。",

  // Uninstall
  "Updating and uninstalling": "更新与卸载",
  "The Mac app checks for updates by itself. To update a command-line install, run the install command again: it upgrades in place and keeps your data.":
    "Mac 应用会自动检查更新。命令行安装的设备，再次运行安装命令即可原地升级，数据保留。",
  "Remove it from your account": "从账户中移除",
  "On {{app}}, open my devices and remove the device. Every key it held is replaced, so it cannot decrypt anything written afterwards.":
    "在 {{app}} 的「我的设备」中移除这台设备。它持有过的密钥都会更换，此后写入的内容，它无法解密。",
  "Disconnect it": "在设备上断开连接",
  "On that device, open Droppen's Settings → Access → Droppen → Disconnect this device. It deletes the Droppen keys stored there, which removing it from the account leaves in place.":
    "在那台设备上打开 Droppen 的「设置 → 访问 → Droppen → 断开这台设备」，删除保存在设备上的 Droppen 密钥。从账户中移除不会删除这些密钥。",
  "Remove the program": "删除程序",
  "The App: quit Droppen and drag it from Applications to the Trash.": "Mac 应用：退出 Droppen，将它从「应用程序」文件夹拖到废纸篓。",
  "Command-line install (Mac or Linux): run this on that device, as the user who installed it:": "命令行安装（Mac 或 Linux）：在那台设备上，以安装时的用户运行：",
  "Run this on that device, as the user who installed it:": "在那台设备上，以安装时的用户运行：",
  "It stops and removes the service and the program and keeps your data; at the end it prints where the data is, and the command that deletes it.":
    "该命令会停止并移除服务和程序，保留数据；结束时会列出数据所在目录，以及删除数据的命令。",

  // The end of the page
  "Before you install:": "安装前需要：",
  "Mac app": "Mac 应用",
  "A Droppen account. Sign-up is closed for now.": "一个 Droppen 账户。目前暂停注册。",
  "Open my devices": "打开「我的设备」",
  "You are signed in.": "你已登录。",

  // The home page's hero: the way onto the device reading it (hero-get.tsx)
  "Download for Mac": "下载 Mac 版",
  "macOS 13 or later · Apple silicon and Intel": "需要 macOS 13 及以上 · 支持 Apple 芯片和 Intel",
  "Install script": "安装脚本",
  "Background service": "后台服务",
  "Network": "网络",
  "Encryption": "加密",
  "End-to-end encrypted between your devices: neither Droppen Hub nor the Droppen relays hold a key that can decrypt your sessions.":
    "在你的设备之间端到端加密：Droppen Hub 和 Droppen 中继都不持有能解密你的会话的密钥。",
  "To update, run the install command again: it upgrades in place and keeps your data.": "要更新，再次运行安装命令即可原地升级，数据保留。",
  "checksums and other files": "校验值和其他文件",
  "Paste it into a terminal on this device: Droppen installs, starts and opens its page.": "在这台设备的终端里粘贴运行：Droppen 会自动安装、启动，并打开它的页面。",
  "A server with no browser?": "服务器上没有浏览器？",
  "No Windows app yet. Droppen works in this browser: watch and answer your AI from here, while it runs on a Mac or a Linux server.":
    "Windows 版 App 还没有。Droppen 在这个浏览器里就能用：在这里看 AI 干活、回答它的问题，AI 本身跑在 Mac 或 Linux 服务器上。",
  "On your phone, Droppen works in the browser: open {{app}} and add it to your home screen. Your AI runs on a Mac or on Linux, with Droppen installed there.":
    "在手机上，Droppen 直接在浏览器里用：打开 {{app}}，添加到主屏幕。AI 跑在 Mac 或 Linux 上，Droppen 装在那里。",
  "how to set it up": "怎么设置",
  "Get Droppen:": "获取 Droppen：",
  "Or download it:": "也可以直接下载：",
};

export const downloadZhTW: Bundle = {
  "Download Droppen": "下載 Droppen",
  "Install Droppen on the Mac or Linux device where Claude Code or Codex runs: the Mac app, or one command. Your phone needs only the browser.":
    "在執行 Claude Code 或 Codex 的 Mac 或 Linux 裝置上安裝 Droppen：Mac 應用程式，或一行指令。手機只需瀏覽器。",

  "Download": "下載",
  "Install Droppen on the device that runs your AI.": "在執行 AI 的裝置上安裝 Droppen。",
  "Droppen goes on the Mac or Linux device where Claude Code or Codex runs, alongside them. Your phone and other devices need nothing installed: open {{app}} in the browser to see every session, answer your AI's questions, steer it and open a terminal.":
    "Droppen 安裝在執行 Claude Code 或 Codex 的 Mac 或 Linux 裝置上，與它們一同執行。手機和其他裝置無需安裝：在瀏覽器中打開 {{app}}，即可查看所有工作階段、回答 AI 的提問、傳送指令和開啟終端機。",
  "A Droppen account. Sign-up needs an invite code.": "一個 Droppen 帳戶。註冊需要邀請碼。",
  "How to install on a Mac": "在 Mac 上的安裝方式",

  "Choose your system": "選擇你的系統",
  "Linux": "Linux",
  "Linux server": "Linux 伺服器",
  "Windows": "Windows",
  "Phone / tablet": "手機 / 平板",
  "Detected": "目前裝置",

  "Droppen for Mac": "Mac 版 Droppen",
  "Open the app and Droppen runs on this Mac, with no terminal and no local password. To use it from your phone and other devices, sign in to Droppen from the app.":
    "打開 App，Droppen 即在這台 Mac 上執行，無需終端機，也無需設定本地密碼。要在手機和其他裝置上使用，請在 App 內登入 Droppen。",
  "Open the app and Droppen runs on this Mac, with no terminal and no local password.": "打開 App，Droppen 即在這台 Mac 上執行，無需終端機，也無需設定本地密碼。",
  "The App is coming soon": "App 即將推出",
  "Until it is out, run Droppen in the background on your Mac: one line, and the same sign-in.": "App 推出之前，可以先以背景服務在 Mac 上執行 Droppen：只需一行指令，登入方式相同。",
  "copy the command": "複製指令",
  "the steps for running it in the background": "查看背景執行的步驟",
  "Download Droppen App (macOS)": "下載 Droppen App（macOS）",
  "macOS 13 or later · Universal: Apple silicon and Intel · Droppen-<version>.dmg": "需要 macOS 13 以上 · 通用版：Apple 晶片和 Intel 都能用 · Droppen-<版本>.dmg",
  "macOS 13 or later · Universal: Apple silicon and Intel": "需要 macOS 13 以上 · 通用版：Apple 晶片和 Intel 都能用",
  "release notes, checksums and other files": "更新說明、校驗值和其他檔案",
  "Open the .dmg": "打開 .dmg",
  "The file you just downloaded, from Downloads.": "在「下載項目」資料夾中打開剛下載的檔案。",
  "Drag Droppen to Applications": "把 Droppen 拖進「應用程式」",
  "Like any other Mac app.": "與安裝其他 Mac 應用程式的方式相同。",
  "Open Droppen": "打開 Droppen",
  "It opens a short setup guide, four steps.": "它會打開一個四步的設定引導。",
  "Sign in to Droppen from the app": "在 App 內登入 Droppen",
  "In the setup guide's Where to use it step, choose “On my phone and other devices too”, then click Sign in to Droppen. Your browser opens {{app}}: sign in, check that the fingerprint matches the one the app shows, then click add this device. Later, the same is under Settings → Access → Droppen.":
    "在設定引導的「在哪裡用」一步，選擇「在手機和其他裝置上也能用」，再點擊「登入 Droppen」。瀏覽器會打開 {{app}}：登入後，核對指紋與 App 中顯示的一致，再點擊「加入這台裝置」。之後也可以在「設定 → 存取 → Droppen」中登入。",
  "Using it from your phone: signing in to Droppen from the app is coming in an update. Until then, the app runs Droppen on this Mac.":
    "在手機上使用：在 App 內登入 Droppen 的功能將在後續更新中提供。在此之前，App 僅在這台 Mac 上執行 Droppen。",

  "Droppen in the background on a Mac": "在 Mac 上以背景服務執行 Droppen",
  "For a Mac that stays on as a server, or if you prefer the terminal. One command installs Droppen; you then add this Mac to your account in the browser.":
    "適用於長期開機、作為伺服器使用的 Mac，或習慣使用終端機的使用者。一行指令完成安裝，之後在瀏覽器中把這台 Mac 加入你的帳戶。",
  "Paste it into Terminal on that Mac.": "在這台 Mac 的「終端機」中貼上並執行。",
  "It installs and starts at login": "安裝並設為登入時啟動",
  "The script installs Droppen, starts it now and at every login, and creates the local page's first account under your user name. It asks you to choose a password (at least 12 characters), or generates one when run without a terminal.":
    "腳本會安裝 Droppen 並立即啟動，此後每次登入時自動啟動；同時以你的系統使用者名稱為本地頁面建立第一個帳戶。腳本會請你設定密碼（至少 12 個字元）；在沒有終端機的環境中執行時，會自動產生密碼。",
  "The local page opens": "打開本地頁面",
  "Your browser opens Droppen's local page. Sign in with that account: everything works there without a Droppen account. To use it from your phone, continue with the next step.":
    "瀏覽器會打開 Droppen 的本地頁面。用上一步的帳戶登入，無需 Droppen 帳戶即可使用全部功能。要在手機上使用，請繼續下一步。",
  "Sign in to Droppen": "登入 Droppen",
  "On the local page, click Sign in to Droppen. A {{app}} window opens: sign in, check that its fingerprint matches the one on the local page, then click add this device.":
    "在本地頁面上點擊「登入 Droppen」，會彈出 {{app}} 視窗。登入後，核對視窗中的指紋與本地頁面上顯示的一致，再點擊「加入這台裝置」。",
  "It is in my devices": "出現在「我的裝置」中",
  "Within seconds it shows as online in my devices, on every phone and browser in your account. Add more devices the same way, and the sessions on all of them appear in one list.":
    "幾秒鐘後，這台裝置會在「我的裝置」中顯示為上線，帳戶中的每支手機和每個瀏覽器都能看到它。按同樣的步驟新增更多裝置後，所有裝置上的工作階段會彙整在同一個列表中。",

  "Droppen on a Linux server": "在 Linux 伺服器上執行 Droppen",
  "Run your AI on a server that stays on, and follow it and answer it from your phone. The server needs no browser: it joins your account with a one-time code.":
    "讓 AI 在常開的伺服器上執行，並在手機上查看進度、回答它的提問。伺服器上無需瀏覽器，使用一次性註冊碼即可加入你的帳戶。",
  "coming": "即將支援",
  "Linux support, with no browser needed on the server, is on its way. Until then, run Droppen on a Mac.":
    "Linux 支援（伺服器上無需瀏覽器）尚未推出。在此之前，可以在 Mac 上執行 Droppen。",
  "run it in the background on a Mac": "改為在 Mac 背景執行",
  "This browser reports Linux on {{arch}}.": "目前瀏覽器執行在 {{arch}} 架構的 Linux 上。",
  "Droppen's Linux builds are for x86_64 and aarch64.": "Droppen 的 Linux 版本支援 x86_64 和 aarch64。",
  "<code>": "<註冊碼>",
  "Replace <code> with the one-time code from step 01 below.": "把指令中的 <註冊碼> 替換為下方第 01 步取得的一次性註冊碼。",
  "Get a one-time code": "取得一次性註冊碼",
  "On {{app}}: my devices → add a device that runs AI → show a one-time code. It is good for 10 minutes, once.":
    "在 {{app}} 打開「我的裝置 → 新增可執行 AI 的裝置 → 顯示一次性註冊碼」。註冊碼 10 分鐘內有效，僅可使用一次。",
  "Run the command on the server": "在伺服器上執行指令",
  "The script installs Droppen, starts it as a systemd user service under your own user, never as root, and joins your account with the code. If it says so at the end, run sudo loginctl enable-linger to keep it running after you log out.":
    "腳本會安裝 Droppen，以你的使用者身分作為 systemd 使用者服務啟動（不使用 root），並用註冊碼加入你的帳戶。如果腳本結束時給出提示，請執行 sudo loginctl enable-linger，使服務在登出後繼續執行。",
  "Within seconds it shows as online in my devices, on every phone and browser in your account.": "幾秒鐘後，這台裝置會在「我的裝置」中顯示為上線，帳戶中的每支手機和每個瀏覽器都能看到它。",

  "Droppen on Windows": "Windows 版 Droppen",
  "Droppen does not run on Windows yet. From a browser on Windows you can already follow your AI and answer its questions.":
    "Droppen 暫不支援在 Windows 上執行。目前可以在 Windows 的瀏覽器中查看 AI 的進度並回答它的提問。",
  "What works on Windows today:": "目前在 Windows 上可以：",
  "Control your other devices from this browser: sign in at {{app}}.": "登入 {{app}}，在這個瀏覽器中控制你的其他裝置。",
  "Run the AI on a Mac, with the Droppen App.": "在 Mac 上執行 AI：使用 Droppen App。",
  "Run the AI on a Mac: in the background today, with the Droppen App soon.": "在 Mac 上執行 AI：目前以背景服務執行，Droppen App 即將推出。",
  "Run the AI on a Linux server, or in WSL with systemd turned on.": "在 Linux 伺服器或已開啟 systemd 的 WSL 中執行 AI。",
  "how to turn on systemd in WSL": "如何在 WSL 中開啟 systemd",
  "Once Linux is supported: a Linux server, or WSL.": "Linux 支援推出後：可使用 Linux 伺服器或 WSL。",
  "use {{app}} in this browser": "在這個瀏覽器中打開 {{app}}",
  "the Mac options": "Mac 安裝方式",
  "the Linux server options": "Linux 伺服器安裝方式",

  "Then, use it from your phone": "安裝之後，在手機上使用",
  "Open {{app}} in your phone's browser and sign in to the same account.": "在手機瀏覽器中打開 {{app}}，登入同一個帳戶。",
  "open {{app}}": "打開 {{app}}",
  "Open {{app}} on your phone": "在手機上打開 {{app}}",
  "iPhone and iPad: in Safari, tap Share, then Add to Home Screen. Android: in Chrome, tap ⋮, then Add to Home screen or Install app. Then you can turn on notifications.":
    "iPhone 和 iPad：在 Safari 中點「分享」，再點「加入主畫面」。Android：在 Chrome 中點 ⋮，再點「加到主畫面」或「安裝應用程式」。加入後即可開啟通知。",
  "Add it to the home screen": "加入主畫面",
  "On a device already in your account, open my devices → add a browser or phone, then scan the QR code the phone shows, or enter its number code.":
    "在已加入帳戶的裝置上，打開「我的裝置 → 新增瀏覽器或手機」，掃描手機上顯示的 QR 碼，或輸入數字碼。",
  "Install it where your AI runs": "裝在執行 AI 的裝置上",
  "You are on a phone. To install, open this page on the device that runs your AI:": "你正在使用手機。請在執行 AI 的裝置上打開此頁面進行安裝：",
  "copy": "複製",

  "Approve the phone": "核准這支手機",
  "What gets installed, and how to verify it": "安裝了什麼，以及如何核對",
  "Mac": "Mac",
  "Signed with a Developer ID and notarized by Apple. It installs only updates signed for that version.": "使用 Developer ID 簽署並經 Apple 公證；只安裝為對應版本簽署的更新。",
  "Runs as you, never as root, and checks the release's SHA-256 checksums before it installs anything.": "以目前使用者身分執行，從不使用 root；安裝前先核對發布檔案的 SHA-256 校驗值。",
  "For a command-line install: a launchd user agent on macOS, a systemd user service on Linux. On Linux, --system makes it a system service that starts at boot and still runs as you; sudo is used only to set up and manage that service.":
    "命令列安裝：macOS 上為 launchd 使用者代理，Linux 上為 systemd 使用者服務。在 Linux 上加 --system 可改為開機啟動的系統服務，Droppen 仍以你的使用者身分執行；sudo 僅用於設定和管理該服務。",
  "Listens only on 127.0.0.1 and reaches Droppen over outbound connections, so there is no inbound port to open. A command-line install uses port 8700, or the next free port.":
    "只在 127.0.0.1 上監聽，透過對外連線接入 Droppen，無需開放對內連接埠。命令列安裝使用 8700 連接埠，被佔用時改用下一個空閒連接埠。",
  "Mac app (recommended)": "Mac 應用程式（推薦）",
  "Command line": "命令列",
  "Claude Code or Codex, installed and signed in on that device": "該裝置上已安裝並登入 Claude Code 或 Codex",

  "Manual download": "手動下載",
  "macOS 13 or later (Apple silicon or Intel), or Linux (x86_64 or aarch64, glibc 2.28 or later)":
    "macOS 13 以上（Apple 晶片或 Intel），或 Linux（x86_64、aarch64，glibc 2.28 以上）",
  "A free Droppen account. You can also create one when you sign in to Droppen on the device.": "一個免費的 Droppen 帳戶，也可以在裝置上登入 Droppen 時建立。",
  "the latest release on GitHub (getdroppen/droppen)": "GitHub 上的最新版本（getdroppen/droppen）",
  "Before you open the .dmg or extract the .tar.gz, check it in a terminal:": "打開 .dmg 或解壓 .tar.gz 之前，先在終端機中核對檔案：",
  "The checksum it prints must match that file's line in the release's SHA256SUMS; if it does not, do not install it. The install script runs the same check before it installs anything.":
    "輸出的校驗值必須與發布頁 SHA256SUMS 中該檔案的值一致；不一致時請勿安裝。安裝腳本在安裝前也會執行同樣的核對。",

  "Updating and uninstalling": "更新與解除安裝",
  "The Mac app checks for updates by itself. To update a command-line install, run the install command again: it upgrades in place and keeps your data.":
    "Mac 應用程式會自動檢查更新。以命令列安裝的裝置，再次執行安裝指令即可原地升級，資料保留。",
  "Remove it from your account": "從帳戶中移除",
  "On {{app}}, open my devices and remove the device. Every key it held is replaced, so it cannot decrypt anything written afterwards.":
    "在 {{app}} 的「我的裝置」中移除這台裝置。它持有過的金鑰都會更換，此後寫入的內容，它無法解密。",
  "Disconnect it": "在裝置上中斷連線",
  "On that device, open Droppen's Settings → Access → Droppen → Disconnect this device. It deletes the Droppen keys stored there, which removing it from the account leaves in place.":
    "在那台裝置上打開 Droppen 的「設定 → 存取 → Droppen → 中斷這台裝置」，刪除儲存在裝置上的 Droppen 金鑰。從帳戶中移除不會刪除這些金鑰。",
  "Remove the program": "刪除程式",
  "The App: quit Droppen and drag it from Applications to the Trash.": "Mac 應用程式：結束 Droppen，將它從「應用程式」資料夾拖到垃圾桶。",
  "Command-line install (Mac or Linux): run this on that device, as the user who installed it:": "命令列安裝（Mac 或 Linux）：在那台裝置上，以安裝時的使用者執行：",
  "Run this on that device, as the user who installed it:": "在那台裝置上，以安裝時的使用者執行：",
  "It stops and removes the service and the program and keeps your data; at the end it prints where the data is, and the command that deletes it.":
    "該指令會停止並移除服務和程式，保留資料；結束時會列出資料所在目錄，以及刪除資料的指令。",

  "Before you install:": "安裝前需要：",
  "Mac app": "Mac 應用程式",
  "A Droppen account. Sign-up is closed for now.": "一個 Droppen 帳戶。目前暫停註冊。",
  "Open my devices": "打開「我的裝置」",
  "You are signed in.": "你已登入。",

  // The home page's hero: the way onto the device reading it (hero-get.tsx)
  "Download for Mac": "下載 Mac 版",
  "macOS 13 or later · Apple silicon and Intel": "需要 macOS 13 以上 · 支援 Apple 晶片和 Intel",
  "Install script": "安裝腳本",
  "Background service": "背景服務",
  "Network": "網路",
  "Encryption": "加密",
  "End-to-end encrypted between your devices: neither Droppen Hub nor the Droppen relays hold a key that can decrypt your sessions.":
    "在你的裝置之間端對端加密：Droppen Hub 和 Droppen 中繼都不持有能解密你的工作階段的金鑰。",
  "To update, run the install command again: it upgrades in place and keeps your data.": "要更新，再次執行安裝指令即可原地升級，資料保留。",
  "checksums and other files": "校驗值和其他檔案",
  "Paste it into a terminal on this device: Droppen installs, starts and opens its page.": "在這台裝置的終端機裡貼上執行：Droppen 會自動安裝、啟動，並打開它的頁面。",
  "A server with no browser?": "伺服器上沒有瀏覽器？",
  "No Windows app yet. Droppen works in this browser: watch and answer your AI from here, while it runs on a Mac or a Linux server.":
    "Windows 版 App 還沒有。Droppen 在這個瀏覽器裡就能用：在這裡看 AI 工作、回答它的問題，AI 本身跑在 Mac 或 Linux 伺服器上。",
  "On your phone, Droppen works in the browser: open {{app}} and add it to your home screen. Your AI runs on a Mac or on Linux, with Droppen installed there.":
    "在手機上，Droppen 直接在瀏覽器裡用：打開 {{app}}，加入主畫面。AI 跑在 Mac 或 Linux 上，Droppen 裝在那裡。",
  "how to set it up": "怎麼設定",
  "Get Droppen:": "取得 Droppen：",
  "Or download it:": "也可以直接下載：",
};

export const downloadJa: Bundle = {
  "Download Droppen": "Droppen をダウンロード",
  "Install Droppen on the Mac or Linux device where Claude Code or Codex runs: the Mac app, or one command. Your phone needs only the browser.":
    "Claude Code か Codex が動く Mac や Linux のデバイスに Droppen をインストール。Mac アプリか、コマンド 1 行で。スマホはブラウザだけで使えます。",

  "Download": "ダウンロード",
  "Install Droppen on the device that runs your AI.": "AI が動くデバイスに Droppen をインストール。",
  "Droppen goes on the Mac or Linux device where Claude Code or Codex runs, alongside them. Your phone and other devices need nothing installed: open {{app}} in the browser to see every session, answer your AI's questions, steer it and open a terminal.":
    "Droppen は、Claude Code か Codex が動く Mac や Linux のデバイスに、それらと並べてインストールします。スマホやほかのデバイスにはインストール不要です。ブラウザで {{app}} を開けば、すべてのセッションを確認し、AI の質問に答え、指示を送り、ターミナルを開けます。",
  "A Droppen account. Sign-up needs an invite code.": "Droppen アカウント。登録には招待コードが必要です。",
  "How to install on a Mac": "Mac でのインストール方法",

  "Choose your system": "システムを選ぶ",
  "Linux": "Linux",
  "Linux server": "Linux サーバー",
  "Windows": "Windows",
  "Phone / tablet": "スマホ / タブレット",
  "Detected": "このデバイス",

  "Droppen for Mac": "Mac 版 Droppen",
  "Open the app and Droppen runs on this Mac, with no terminal and no local password. To use it from your phone and other devices, sign in to Droppen from the app.":
    "アプリを開くと、この Mac で Droppen が動きます。ターミナルもローカルのパスワードも不要です。スマホやほかのデバイスから使うには、アプリから Droppen にサインインします。",
  "Open the app and Droppen runs on this Mac, with no terminal and no local password.": "アプリを開くと、この Mac で Droppen が動きます。ターミナルもローカルのパスワードも不要です。",
  "The App is coming soon": "アプリは近日公開",
  "Until it is out, run Droppen in the background on your Mac: one line, and the same sign-in.":
    "公開までは、Mac のバックグラウンドサービスとして Droppen を動かせます。コマンドは 1 行、サインインの手順は同じです。",
  "copy the command": "コマンドをコピー",
  "the steps for running it in the background": "バックグラウンドで動かす手順を見る",
  "Download Droppen App (macOS)": "Droppen アプリをダウンロード（macOS）",
  "macOS 13 or later · Universal: Apple silicon and Intel · Droppen-<version>.dmg": "macOS 13 以降 · ユニバーサル：Apple シリコンと Intel の両方に対応 · Droppen-<バージョン>.dmg",
  "macOS 13 or later · Universal: Apple silicon and Intel": "macOS 13 以降 · ユニバーサル：Apple シリコンと Intel の両方に対応",
  "release notes, checksums and other files": "リリースノート・チェックサム・ほかのファイル",
  "Open the .dmg": ".dmg を開く",
  "The file you just downloaded, from Downloads.": "「ダウンロード」フォルダにある、ダウンロードしたファイルを開きます。",
  "Drag Droppen to Applications": "Droppen を「アプリケーション」にドラッグ",
  "Like any other Mac app.": "ほかの Mac アプリと同じ手順です。",
  "Open Droppen": "Droppen を開く",
  "It opens a short setup guide, four steps.": "4 ステップの短いセットアップガイドが開きます。",
  "Sign in to Droppen from the app": "アプリから Droppen にサインイン",
  "In the setup guide's Where to use it step, choose “On my phone and other devices too”, then click Sign in to Droppen. Your browser opens {{app}}: sign in, check that the fingerprint matches the one the app shows, then click add this device. Later, the same is under Settings → Access → Droppen.":
    "セットアップガイドの「使う場所」で「スマートフォンやほかのデバイスでも使う」を選び、「Droppen にサインイン」を押します。ブラウザで {{app}} が開くので、サインインし、フィンガープリントがアプリの表示と一致することを確かめてから「このデバイスを追加」を押します。あとからは「設定 → アクセス → Droppen」でもサインインできます。",
  "Using it from your phone: signing in to Droppen from the app is coming in an update. Until then, the app runs Droppen on this Mac.":
    "スマホから使う場合：アプリからの Droppen へのサインインは、今後のアップデートで対応します。それまでは、アプリはこの Mac でのみ Droppen を動かします。",

  "Droppen in the background on a Mac": "Mac でバックグラウンドサービスとして Droppen を動かす",
  "For a Mac that stays on as a server, or if you prefer the terminal. One command installs Droppen; you then add this Mac to your account in the browser.":
    "サーバーとして常時稼働させる Mac や、ターミナルを使いたい方向けです。コマンド 1 行で Droppen をインストールし、ブラウザでこの Mac をアカウントに追加します。",
  "Paste it into Terminal on that Mac.": "その Mac の「ターミナル」に貼り付けて実行します。",
  "It installs and starts at login": "インストールしてログイン時に起動",
  "The script installs Droppen, starts it now and at every login, and creates the local page's first account under your user name. It asks you to choose a password (at least 12 characters), or generates one when run without a terminal.":
    "スクリプトが Droppen をインストールしてすぐに起動し、以後はログインのたびに起動します。あわせて、ローカルページの最初のアカウントをシステムのユーザー名で作成します。パスワードは自分で設定するよう求められます（12 文字以上）。ターミナルなしで実行した場合は自動で生成されます。",
  "The local page opens": "ローカルページが開く",
  "Your browser opens Droppen's local page. Sign in with that account: everything works there without a Droppen account. To use it from your phone, continue with the next step.":
    "ブラウザで Droppen のローカルページが開きます。そのアカウントでログインすれば、Droppen アカウントなしですべての機能を使えます。スマホから使うには、次のステップに進みます。",
  "Sign in to Droppen": "Droppen にサインイン",
  "On the local page, click Sign in to Droppen. A {{app}} window opens: sign in, check that its fingerprint matches the one on the local page, then click add this device.":
    "ローカルページで「Droppen にサインイン」を押すと、{{app}} のウィンドウが開きます。サインインし、フィンガープリントがローカルページの表示と一致することを確かめてから「このデバイスを追加」を押します。",
  "It is in my devices": "「マイデバイス」に表示される",
  "Within seconds it shows as online in my devices, on every phone and browser in your account. Add more devices the same way, and the sessions on all of them appear in one list.":
    "数秒で「マイデバイス」にオンラインと表示され、アカウントのすべてのスマホとブラウザから見えるようになります。同じ手順でデバイスを追加すると、すべてのデバイスのセッションがひとつのリストにまとまります。",

  "Droppen on a Linux server": "Linux サーバーで Droppen を動かす",
  "Run your AI on a server that stays on, and follow it and answer it from your phone. The server needs no browser: it joins your account with a one-time code.":
    "常時稼働のサーバーで AI を動かし、スマホから進み具合を確認して質問に答えられます。サーバーにブラウザは不要で、ワンタイムコードでアカウントに追加します。",
  "coming": "近日対応",
  "Linux support, with no browser needed on the server, is on its way. Until then, run Droppen on a Mac.":
    "Linux 対応（サーバーにブラウザ不要）はまだ提供していません。それまでは Mac で Droppen を動かせます。",
  "run it in the background on a Mac": "Mac のバックグラウンドで動かす",
  "This browser reports Linux on {{arch}}.": "このブラウザは {{arch}} の Linux で動いています。",
  "Droppen's Linux builds are for x86_64 and aarch64.": "Droppen の Linux 版は x86_64 と aarch64 に対応しています。",
  "<code>": "<コード>",
  "Replace <code> with the one-time code from step 01 below.": "<コード> を、下のステップ 01 で取得したワンタイムコードに置き換えます。",
  "Get a one-time code": "ワンタイムコードを取得",
  "On {{app}}: my devices → add a device that runs AI → show a one-time code. It is good for 10 minutes, once.":
    "{{app}} で「マイデバイス → AI を実行できるデバイスを追加 → ワンタイムコードを表示」を開きます。コードの有効期間は 10 分で、使えるのは 1 回だけです。",
  "Run the command on the server": "サーバーでコマンドを実行",
  "The script installs Droppen, starts it as a systemd user service under your own user, never as root, and joins your account with the code. If it says so at the end, run sudo loginctl enable-linger to keep it running after you log out.":
    "スクリプトが Droppen をインストールし、root ではなく自分のユーザーで systemd のユーザーサービスとして起動して、コードでアカウントに追加します。最後に案内が表示された場合は、sudo loginctl enable-linger を実行すると、ログアウト後も動き続けます。",
  "Within seconds it shows as online in my devices, on every phone and browser in your account.":
    "数秒で「マイデバイス」にオンラインと表示され、アカウントのすべてのスマホとブラウザから見えるようになります。",

  "Droppen on Windows": "Windows 版 Droppen",
  "Droppen does not run on Windows yet. From a browser on Windows you can already follow your AI and answer its questions.":
    "Droppen はまだ Windows では動きません。Windows のブラウザからでも、AI の進み具合を確認し、質問に答えることはできます。",
  "What works on Windows today:": "Windows で現在できること：",
  "Control your other devices from this browser: sign in at {{app}}.": "{{app}} にサインインし、このブラウザからほかのデバイスを操作する。",
  "Run the AI on a Mac, with the Droppen App.": "Mac で AI を動かす：Droppen アプリを使う。",
  "Run the AI on a Mac: in the background today, with the Droppen App soon.": "Mac で AI を動かす：現在はバックグラウンドサービスで。Droppen アプリは近日公開。",
  "Run the AI on a Linux server, or in WSL with systemd turned on.": "Linux サーバーか、systemd を有効にした WSL で AI を動かす。",
  "how to turn on systemd in WSL": "WSL で systemd を有効にする方法",
  "Once Linux is supported: a Linux server, or WSL.": "Linux 対応後は、Linux サーバーか WSL で。",
  "use {{app}} in this browser": "このブラウザで {{app}} を開く",
  "the Mac options": "Mac でのインストール方法",
  "the Linux server options": "Linux サーバーでのインストール方法",

  "Then, use it from your phone": "インストールしたら、スマホで使う",
  "Open {{app}} in your phone's browser and sign in to the same account.": "スマホのブラウザで {{app}} を開き、同じアカウントでログインします。",
  "open {{app}}": "{{app}} を開く",
  "Open {{app}} on your phone": "スマホで {{app}} を開く",
  "iPhone and iPad: in Safari, tap Share, then Add to Home Screen. Android: in Chrome, tap ⋮, then Add to Home screen or Install app. Then you can turn on notifications.":
    "iPhone と iPad：Safari で「共有」→「ホーム画面に追加」。Android：Chrome で ⋮ →「ホーム画面に追加」か「アプリをインストール」。追加すると通知をオンにできます。",
  "Add it to the home screen": "ホーム画面に追加",
  "On a device already in your account, open my devices → add a browser or phone, then scan the QR code the phone shows, or enter its number code.":
    "このアカウントの既存のデバイスで「マイデバイス → ブラウザかスマートフォンを追加」を開き、スマホに表示された QR コードを読み取るか、数字コードを入力します。",
  "Install it where your AI runs": "AI を動かすデバイスにインストール",
  "You are on a phone. To install, open this page on the device that runs your AI:": "いまスマホで見ています。インストールするには、AI が動くデバイスでこのページを開いてください：",
  "copy": "コピー",

  "Approve the phone": "スマホを承認する",
  "What gets installed, and how to verify it": "インストールされるものと、その確認方法",
  "Mac": "Mac",
  "Signed with a Developer ID and notarized by Apple. It installs only updates signed for that version.":
    "Developer ID で署名され、Apple の公証を受けています。そのバージョン向けに署名された更新だけをインストールします。",
  "Runs as you, never as root, and checks the release's SHA-256 checksums before it installs anything.":
    "実行したユーザーとして動き、root では動きません。インストール前にリリースの SHA-256 チェックサムを照合します。",
  "For a command-line install: a launchd user agent on macOS, a systemd user service on Linux. On Linux, --system makes it a system service that starts at boot and still runs as you; sudo is used only to set up and manage that service.":
    "コマンドラインでのインストール：macOS では launchd のユーザーエージェント、Linux では systemd のユーザーサービスとして動きます。Linux で --system を付けると起動時に開始するシステムサービスになり、その場合も自分のユーザーで動きます。sudo はそのサービスの設定と管理にだけ使います。",
  "Listens only on 127.0.0.1 and reaches Droppen over outbound connections, so there is no inbound port to open. A command-line install uses port 8700, or the next free port.":
    "127.0.0.1 でのみ待ち受け、Droppen には外向きの接続でつながるため、開ける必要のある受信ポートはありません。コマンドラインでのインストールではポート 8700 を使い、使用中なら次の空きポートを使います。",
  "Mac app (recommended)": "Mac アプリ（推奨）",
  "Command line": "コマンドライン",
  "Claude Code or Codex, installed and signed in on that device": "そのデバイスに Claude Code か Codex がインストールされ、ログイン済みであること",

  "Manual download": "手動ダウンロード",
  "macOS 13 or later (Apple silicon or Intel), or Linux (x86_64 or aarch64, glibc 2.28 or later)":
    "macOS 13 以降（Apple シリコンまたは Intel）、または Linux（x86_64、aarch64、glibc 2.28 以降）",
  "A free Droppen account. You can also create one when you sign in to Droppen on the device.": "無料の Droppen アカウント。デバイスで Droppen にサインインするときに作成することもできます。",
  "the latest release on GitHub (getdroppen/droppen)": "GitHub の最新リリース（getdroppen/droppen）",
  "Before you open the .dmg or extract the .tar.gz, check it in a terminal:": ".dmg を開く前、または .tar.gz を展開する前に、ターミナルでファイルを確認します：",
  "The checksum it prints must match that file's line in the release's SHA256SUMS; if it does not, do not install it. The install script runs the same check before it installs anything.":
    "表示されたチェックサムが、リリースの SHA256SUMS にあるそのファイルの値と一致することを確かめます。一致しない場合はインストールしないでください。インストールスクリプトも、インストール前に同じ確認をします。",

  "Updating and uninstalling": "アップデートとアンインストール",
  "The Mac app checks for updates by itself. To update a command-line install, run the install command again: it upgrades in place and keeps your data.":
    "Mac アプリはアップデートを自動で確認します。コマンドラインでインストールした場合は、インストールコマンドをもう一度実行すると、データを残したままアップデートされます。",
  "Remove it from your account": "アカウントから削除する",
  "On {{app}}, open my devices and remove the device. Every key it held is replaced, so it cannot decrypt anything written afterwards.":
    "{{app}} の「マイデバイス」でそのデバイスを削除します。そのデバイスが持っていた鍵はすべて新しいものに替わるため、以後に書かれた内容は復号できません。",
  "Disconnect it": "デバイスで接続を解除する",
  "On that device, open Droppen's Settings → Access → Droppen → Disconnect this device. It deletes the Droppen keys stored there, which removing it from the account leaves in place.":
    "そのデバイスで Droppen の「設定 → アクセス → Droppen → このデバイスの接続を解除」を選ぶと、デバイスに保存された Droppen の鍵が削除されます。アカウントから削除しただけでは、これらの鍵は残ります。",
  "Remove the program": "プログラムを削除する",
  "The App: quit Droppen and drag it from Applications to the Trash.": "Mac アプリ：Droppen を終了し、「アプリケーション」フォルダからゴミ箱にドラッグします。",
  "Command-line install (Mac or Linux): run this on that device, as the user who installed it:":
    "コマンドラインでのインストール（Mac または Linux）：そのデバイスで、インストールしたユーザーとして実行します：",
  "Run this on that device, as the user who installed it:": "そのデバイスで、インストールしたユーザーとして実行します：",
  "It stops and removes the service and the program and keeps your data; at the end it prints where the data is, and the command that deletes it.":
    "サービスとプログラムを停止して削除し、データは残します。最後に、データの場所とそれを削除するコマンドが表示されます。",

  "Before you install:": "インストールの前に必要なもの：",
  "Mac app": "Mac アプリ",
  "A Droppen account. Sign-up is closed for now.": "Droppen アカウント。現在、新規登録を停止しています。",
  "Open my devices": "「マイデバイス」を開く",
  "You are signed in.": "ログインしています。",

  // The home page's hero: the way onto the device reading it (hero-get.tsx)
  "Download for Mac": "Mac 版をダウンロード",
  "macOS 13 or later · Apple silicon and Intel": "macOS 13 以降 · Apple シリコンと Intel に対応",
  "Install script": "インストールスクリプト",
  "Background service": "バックグラウンドサービス",
  "Network": "ネットワーク",
  "Encryption": "暗号化",
  "End-to-end encrypted between your devices: neither Droppen Hub nor the Droppen relays hold a key that can decrypt your sessions.":
    "デバイス間でエンドツーエンド暗号化されます。Droppen Hub も Droppen リレーも、セッションを復号できる鍵を持っていません。",
  "To update, run the install command again: it upgrades in place and keeps your data.": "アップデートするには、インストールコマンドをもう一度実行します。データを残したままアップデートされます。",
  "checksums and other files": "チェックサムとほかのファイル",
  "Paste it into a terminal on this device: Droppen installs, starts and opens its page.": "このデバイスのターミナルに貼り付けて実行すると、Droppen がインストールされ、起動してページが開きます。",
  "A server with no browser?": "ブラウザのないサーバーなら？",
  "No Windows app yet. Droppen works in this browser: watch and answer your AI from here, while it runs on a Mac or a Linux server.":
    "Windows 版アプリはまだありません。Droppen はこのブラウザで使えます。ここから AI の作業を見て質問に答えられ、AI 自体は Mac か Linux サーバーで動きます。",
  "On your phone, Droppen works in the browser: open {{app}} and add it to your home screen. Your AI runs on a Mac or on Linux, with Droppen installed there.":
    "スマホでは、Droppen はブラウザで使います。{{app}} を開いてホーム画面に追加してください。AI は Mac か Linux で動かし、Droppen はそちらに入れます。",
  "how to set it up": "設定のしかた",
  "Get Droppen:": "Droppen を入手：",
  "Or download it:": "直接ダウンロードもできます：",
};
