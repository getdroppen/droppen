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
  "Droppen goes on the Mac or Linux device where Claude Code or Codex runs, alongside them. Your phone and other devices need nothing installed: open {{app}} in the browser.":
    "Droppen 安装在运行 Claude Code 或 Codex 的 Mac 或 Linux 设备上，与它们一同运行。手机和其他设备无需安装，在浏览器中打开 {{app}} 即可。",
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
  "Open the app and Droppen is running on your Mac: no terminal, no login page. Then sign it in to Droppen from inside the app.":
    "打开 App，Droppen 就在你的 Mac 上跑起来了：不用终端，也没有登录页。然后在 App 里登录 Droppen 就行。",
  "Open the app and Droppen is running on your Mac: no terminal, no login page.": "打开 App，Droppen 就在你的 Mac 上跑起来了：不用终端，也没有登录页。",
  "The App is coming soon": "App 即将推出",
  "Until it is out, run Droppen in the background on your Mac: one line, and the same sign-in.": "在它推出之前，可以先让 Droppen 在 Mac 后台运行：一行命令，登录方式一样。",
  "copy the command": "复制命令",
  "the steps for running it in the background": "查看后台运行的步骤",
  "Download Droppen App (macOS)": "下载 Droppen App（macOS）",
  "macOS 13 or later · Universal: Apple silicon and Intel · Droppen-<version>.dmg": "需要 macOS 13 及以上 · 通用版：Apple 芯片和 Intel 都能用 · Droppen-<版本>.dmg",
  "macOS 13 or later · Universal: Apple silicon and Intel": "需要 macOS 13 及以上 · 通用版：Apple 芯片和 Intel 都能用",
  "release notes, checksums and other files": "更新说明、校验值和其他文件",
  "Open the .dmg": "打开 .dmg",
  "The file you just downloaded, from Downloads.": "就是刚下载好的那个文件，在「下载」文件夹里。",
  "Drag Droppen to Applications": "把 Droppen 拖进「应用程序」",
  "Like any other Mac app.": "和装其他 Mac App 一样。",
  "Open Droppen": "打开 Droppen",
  "It opens a short setup guide, four steps.": "它会打开一个四步的设置向导。",
  "Click Sign in to Droppen in the app": "在 App 里点「登录 Droppen」",
  "In the guide's step 02 (Where to use it), choose “On my phone and other devices too”, then Sign in to Droppen (later: Settings → Access → Droppen). A {{app}} window opens: sign in, then click add this device.":
    "在向导第 02 步「在哪里用」选「在手机和其它设备上也能用」，再点「登录 Droppen」（之后也可以在「设置 → 访问 → Droppen」里找到）。会弹出 {{app}} 窗口：登录后点「加入这台设备」。",
  "Using it from your phone: signing in to Droppen from the app is coming in an update. Until then, the app runs Droppen on this Mac.":
    "想在手机上用？在 App 里登录 Droppen 的功能会在后续更新中推出。在那之前，App 先在这台 Mac 上运行 Droppen。",

  // Mac · background
  "Droppen in the background on a Mac": "让 Droppen 在 Mac 后台运行",
  "For a Mac that stays on as a server, or if you would rather use the terminal: one line, then everything else happens in the browser.":
    "适合一直开着、当服务器用的 Mac，或者你更习惯终端：一行命令，剩下的都在浏览器里完成。",
  "Paste it into Terminal on that Mac.": "粘贴到那台 Mac 的「终端」里运行。",
  "It installs and starts at login": "安装，并设为登录时启动",
  "The script installs Droppen, starts it now and at every login, and creates your first local login: it prints it, or asks you to choose one.":
    "脚本会安装 Droppen，现在就启动，以后每次登录也会自动启动；同时创建第一个本地登录：要么直接打印出来，要么请你自己设一个。",
  "Its page opens": "它的页面自动打开",
  "Your browser opens Droppen on that device. Sign in with the local login: everything already works there, with no account.":
    "浏览器会打开这台设备上的 Droppen。用本地登录进去，所有功能已经能用，不需要账号。",
  "Sign in to Droppen": "登录 Droppen",
  "Click Sign in to Droppen on that page. A {{app}} window opens: sign in, check the fingerprint matches, and add the device.":
    "在那个页面上点「登录 Droppen」，会弹出 {{app}} 的窗口：登录，核对两边的指纹一致，再把这台设备加进来。",
  "It is in my devices": "出现在「我的设备」里",
  "Within seconds it is online in my devices, on every phone and browser of your account. Add another device the same way, and put both on one screen.":
    "几秒钟后，它就在「我的设备」里上线了，你账户里的每部手机、每个浏览器都看得到。再用同样的方法加一台设备，就能把两台放进同一个屏幕。",

  // Linux server
  "Droppen on a Linux server": "在 Linux 服务器上运行 Droppen",
  "Run your AI on a server that never sleeps, and watch it from your phone. No browser needed on the server: it joins your account with a one-time code.":
    "让 AI 跑在从不休眠的服务器上，在手机上看着它干活。服务器上不需要浏览器：用一次性注册码就能加入你的账户。",
  "coming": "即将支持",
  "Linux support, with no browser needed on the server, is on its way. Until then, run Droppen on a Mac.": "Linux 支持（服务器上不需要浏览器）正在路上。在那之前，可以先在 Mac 上运行 Droppen。",
  "run it in the background on a Mac": "改为在 Mac 后台运行",
  "This browser reports Linux on {{arch}}.": "这个浏览器报告的系统是 {{arch}} 架构的 Linux。",
  "Droppen's Linux builds are for x86_64 and aarch64.": "Droppen 的 Linux 版本支持 x86_64 和 aarch64。",
  "<code>": "<注册码>",
  "Replace <code> with a one-time code: on {{app}}, open my devices → add a device that runs AI → show a one-time code.":
    "把 <注册码> 换成一次性注册码：在 {{app}} 打开「我的设备 → 添加可运行 AI 的设备 → 显示一次性注册码」。",
  "Get a one-time code": "拿一个一次性注册码",
  "On {{app}}: my devices → add a device that runs AI → show a one-time code. It is good for 10 minutes, once.": "在 {{app}}：「我的设备 → 添加可运行 AI 的设备 → 显示一次性注册码」。10 分钟内有效，只能用一次。",
  "Run the command on the server": "在服务器上运行命令",
  "It installs Droppen, starts it as a service, and joins your account with the code.": "它会安装 Droppen、作为服务启动，并用注册码加入你的账户。",
  "Within seconds it is online in my devices, on every phone and browser of your account.": "几秒钟后，它就在「我的设备」里上线了，你账户里的每部手机、每个浏览器都看得到。",

  // Windows
  "Droppen on Windows": "Windows 版 Droppen",
  "Not supported yet: it is on its way. On Windows you can already watch and answer your AI.": "暂不支持，正在路上。不过在 Windows 上现在就能看 AI 干活、回答它的提问。",
  "not yet": "暂不支持",
  "What works on Windows today:": "在 Windows 上现在就能做的：",
  "Control your other devices from this browser: sign in at {{app}}.": "在这个浏览器里控制你的其他设备：登录 {{app}} 就行。",
  "Run the AI on a Mac, with the Droppen App.": "让 AI 跑在 Mac 上，用 Droppen App。",
  "Run the AI on a Mac: in the background today, with the Droppen App soon.": "让 AI 跑在 Mac 上：现在可以后台运行，App 也快来了。",
  "Run the AI on a Linux server, or in WSL with systemd turned on.": "让 AI 跑在 Linux 服务器上，或者开启了 systemd 的 WSL 里。",
  "how to turn on systemd in WSL": "如何在 WSL 里开启 systemd",
  "Once Linux is supported: a Linux server, or WSL.": "等 Linux 支持上线后：Linux 服务器，或者 WSL。",
  "use {{app}} in this browser": "在这个浏览器里用 {{app}}",
  "the Mac options": "看看 Mac 的方式",
  "the Linux server options": "看看 Linux 服务器的方式",

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
  "A launchd user agent on macOS, a systemd user service on Linux. With --system, a system service that starts at boot; sudo is used only to write its service file.":
    "macOS 上为 launchd 用户代理，Linux 上为 systemd 用户服务。加上 --system 可改为开机启动的系统服务，sudo 仅用于写入服务文件。",
  "Listens only on 127.0.0.1 (port 8700 by default) and reaches Droppen over outbound connections, so there is no inbound port to open.":
    "只在 127.0.0.1 上监听（默认端口 8700），通过出站连接接入 Droppen，无需开放入站端口。",
  "Mac app (recommended)": "Mac 应用（推荐）",
  "Command line": "命令行",
  "Claude Code or Codex, installed and signed in on that device": "该设备上已安装并登录 Claude Code 或 Codex",

  // Manual download and checksums
  "Manual download": "手动下载",
  "macOS 13 or later (Apple silicon or Intel), or Linux (x86_64 or aarch64)": "macOS 13 及以上（Apple 芯片或 Intel），或 Linux（x86_64、aarch64）",
  "A free Droppen account. You can also create it when you sign the device in.": "一个免费的 Droppen 账户，也可以在设备登录时创建。",
  "latest release on GitHub": "GitHub 上的最新版本",
  "Check the file before you open it:": "打开之前先核对文件：",
  "The number it prints must be the one the release lists. The install script makes the same check before it installs anything.":
    "打印出来的值必须和发布页上列的一致。安装脚本在安装前也会做同样的核对。",

  // Uninstall
  "Updating and uninstalling": "更新与卸载",
  "The Mac app checks for updates by itself. To update a command-line install, run the install command again: it upgrades in place and keeps your data.":
    "Mac 应用会自动检查更新。用命令行安装的，再次运行安装命令即可原地升级，数据保留。",
  "Remove it from your account": "从账户里移除",
  "On {{app}}, open my devices and remove the device. Its keys are rotated, so it cannot open anything written afterwards.":
    "在 {{app}} 的「我的设备」里移除这台设备。密钥会随之轮换，之后写下的内容它都打不开。",
  "Disconnect it": "断开连接",
  "On the device, Settings → Access → Droppen → Disconnect this device deletes its Droppen keys there.": "在那台设备上，「设置 → 访问 → Droppen → 断开这台设备」会删除它本地的 Droppen 密钥。",
  "Remove the program": "删除程序",
  "The App: quit Droppen and drag it from Applications to the Trash.": "App：退出 Droppen，把它从「应用程序」拖进废纸篓。",
  "Command line: run the install script again with --uninstall:": "命令行安装：加上 --uninstall 再运行一次安装脚本：",
  "Run the install script again with --uninstall:": "加上 --uninstall 再运行一次安装脚本：",

  // The end of the page
  "AI tools": "AI 工具",
  "System": "系统",
  "Account": "账户",
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
  "Droppen goes on the Mac or Linux device where Claude Code or Codex runs, alongside them. Your phone and other devices need nothing installed: open {{app}} in the browser.":
    "Droppen 安裝在執行 Claude Code 或 Codex 的 Mac 或 Linux 裝置上，與它們一同執行。手機和其他裝置無需安裝，在瀏覽器中打開 {{app}} 即可。",
  "A Droppen account. Sign-up needs an invite code.": "一個 Droppen 帳戶。註冊需要邀請碼。",
  "How to install on a Mac": "在 Mac 上的安裝方式",

  "Choose your system": "選擇你的系統",
  "Linux": "Linux",
  "Linux server": "Linux 伺服器",
  "Windows": "Windows",
  "Phone / tablet": "手機 / 平板",
  "Detected": "目前裝置",

  "Droppen for Mac": "Mac 版 Droppen",
  "Open the app and Droppen is running on your Mac: no terminal, no login page. Then sign it in to Droppen from inside the app.":
    "打開 App，Droppen 就在你的 Mac 上執行了：不用終端機，也沒有登入頁。接著在 App 裡登入 Droppen 就好。",
  "Open the app and Droppen is running on your Mac: no terminal, no login page.": "打開 App，Droppen 就在你的 Mac 上執行了：不用終端機，也沒有登入頁。",
  "The App is coming soon": "App 即將推出",
  "Until it is out, run Droppen in the background on your Mac: one line, and the same sign-in.": "在它推出之前，可以先讓 Droppen 在 Mac 背景執行：一行指令，登入方式一樣。",
  "copy the command": "複製指令",
  "the steps for running it in the background": "查看背景執行的步驟",
  "Download Droppen App (macOS)": "下載 Droppen App（macOS）",
  "macOS 13 or later · Universal: Apple silicon and Intel · Droppen-<version>.dmg": "需要 macOS 13 以上 · 通用版：Apple 晶片和 Intel 都能用 · Droppen-<版本>.dmg",
  "macOS 13 or later · Universal: Apple silicon and Intel": "需要 macOS 13 以上 · 通用版：Apple 晶片和 Intel 都能用",
  "release notes, checksums and other files": "更新說明、校驗值和其他檔案",
  "Open the .dmg": "打開 .dmg",
  "The file you just downloaded, from Downloads.": "就是剛下載好的那個檔案，在「下載項目」資料夾裡。",
  "Drag Droppen to Applications": "把 Droppen 拖進「應用程式」",
  "Like any other Mac app.": "和安裝其他 Mac App 一樣。",
  "Open Droppen": "打開 Droppen",
  "It opens a short setup guide, four steps.": "它會打開一個四步的設定引導。",
  "Click Sign in to Droppen in the app": "在 App 裡點「登入 Droppen」",
  "In the guide's step 02 (Where to use it), choose “On my phone and other devices too”, then Sign in to Droppen (later: Settings → Access → Droppen). A {{app}} window opens: sign in, then click add this device.":
    "在引導第 02 步「在哪裡用」選「在手機和其他裝置上也能用」，再點「登入 Droppen」（之後也可以在「設定 → 存取 → Droppen」裡找到）。會彈出 {{app}} 視窗：登入後點「加入這台裝置」。",
  "Using it from your phone: signing in to Droppen from the app is coming in an update. Until then, the app runs Droppen on this Mac.":
    "想在手機上用？在 App 裡登入 Droppen 的功能會在後續更新中推出。在那之前，App 先在這台 Mac 上執行 Droppen。",

  "Droppen in the background on a Mac": "讓 Droppen 在 Mac 背景執行",
  "For a Mac that stays on as a server, or if you would rather use the terminal: one line, then everything else happens in the browser.":
    "適合一直開著、當伺服器用的 Mac，或者你更習慣終端機：一行指令，剩下的都在瀏覽器裡完成。",
  "Paste it into Terminal on that Mac.": "貼到那台 Mac 的「終端機」裡執行。",
  "It installs and starts at login": "安裝，並設為登入時啟動",
  "The script installs Droppen, starts it now and at every login, and creates your first local login: it prints it, or asks you to choose one.":
    "腳本會安裝 Droppen，現在就啟動，之後每次登入也會自動啟動；同時建立第一個本地登入：直接印出來，或請你自己設定一個。",
  "Its page opens": "它的頁面自動打開",
  "Your browser opens Droppen on that device. Sign in with the local login: everything already works there, with no account.":
    "瀏覽器會打開這台裝置上的 Droppen。用本地登入進去，所有功能都已經能用，不需要帳號。",
  "Sign in to Droppen": "登入 Droppen",
  "Click Sign in to Droppen on that page. A {{app}} window opens: sign in, check the fingerprint matches, and add the device.":
    "在那個頁面上點「登入 Droppen」，會彈出 {{app}} 的視窗：登入，核對兩邊的指紋一致，再把這台裝置加進來。",
  "It is in my devices": "出現在「我的裝置」裡",
  "Within seconds it is online in my devices, on every phone and browser of your account. Add another device the same way, and put both on one screen.":
    "幾秒鐘後，它就在「我的裝置」裡上線了，你帳戶裡的每支手機、每個瀏覽器都看得到。再用同樣的方法加一台裝置，就能把兩台放進同一個畫面。",

  "Droppen on a Linux server": "在 Linux 伺服器上執行 Droppen",
  "Run your AI on a server that never sleeps, and watch it from your phone. No browser needed on the server: it joins your account with a one-time code.":
    "讓 AI 在從不休眠的伺服器上執行，在手機上看著它工作。伺服器上不需要瀏覽器：用一次性註冊碼就能加入你的帳戶。",
  "coming": "即將支援",
  "Linux support, with no browser needed on the server, is on its way. Until then, run Droppen on a Mac.": "Linux 支援（伺服器上不需要瀏覽器）正在路上。在那之前，可以先在 Mac 上執行 Droppen。",
  "run it in the background on a Mac": "改為在 Mac 背景執行",
  "This browser reports Linux on {{arch}}.": "這個瀏覽器回報的系統是 {{arch}} 架構的 Linux。",
  "Droppen's Linux builds are for x86_64 and aarch64.": "Droppen 的 Linux 版本支援 x86_64 和 aarch64。",
  "<code>": "<註冊碼>",
  "Replace <code> with a one-time code: on {{app}}, open my devices → add a device that runs AI → show a one-time code.":
    "把 <註冊碼> 換成一次性註冊碼：在 {{app}} 打開「我的裝置 → 新增可執行 AI 的裝置 → 顯示一次性註冊碼」。",
  "Get a one-time code": "拿一個一次性註冊碼",
  "On {{app}}: my devices → add a device that runs AI → show a one-time code. It is good for 10 minutes, once.": "在 {{app}}：「我的裝置 → 新增可執行 AI 的裝置 → 顯示一次性註冊碼」。10 分鐘內有效，只能用一次。",
  "Run the command on the server": "在伺服器上執行指令",
  "It installs Droppen, starts it as a service, and joins your account with the code.": "它會安裝 Droppen、作為服務啟動，並用註冊碼加入你的帳戶。",
  "Within seconds it is online in my devices, on every phone and browser of your account.": "幾秒鐘後，它就在「我的裝置」裡上線了，你帳戶裡的每支手機、每個瀏覽器都看得到。",

  "Droppen on Windows": "Windows 版 Droppen",
  "Not supported yet: it is on its way. On Windows you can already watch and answer your AI.": "暫不支援，正在路上。不過在 Windows 上現在就能看 AI 工作、回答它的提問。",
  "not yet": "暫不支援",
  "What works on Windows today:": "在 Windows 上現在就能做的：",
  "Control your other devices from this browser: sign in at {{app}}.": "在這個瀏覽器裡控制你的其他裝置：登入 {{app}} 就好。",
  "Run the AI on a Mac, with the Droppen App.": "讓 AI 在 Mac 上執行，用 Droppen App。",
  "Run the AI on a Mac: in the background today, with the Droppen App soon.": "讓 AI 在 Mac 上執行：現在可以背景執行，App 也快來了。",
  "Run the AI on a Linux server, or in WSL with systemd turned on.": "讓 AI 在 Linux 伺服器上執行，或者在開啟了 systemd 的 WSL 裡。",
  "how to turn on systemd in WSL": "如何在 WSL 裡開啟 systemd",
  "Once Linux is supported: a Linux server, or WSL.": "等 Linux 支援上線後：Linux 伺服器，或者 WSL。",
  "use {{app}} in this browser": "在這個瀏覽器裡用 {{app}}",
  "the Mac options": "看看 Mac 的方式",
  "the Linux server options": "看看 Linux 伺服器的方式",

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
  "A launchd user agent on macOS, a systemd user service on Linux. With --system, a system service that starts at boot; sudo is used only to write its service file.":
    "macOS 上為 launchd 使用者代理，Linux 上為 systemd 使用者服務。加上 --system 可改為開機啟動的系統服務，sudo 僅用於寫入服務檔案。",
  "Listens only on 127.0.0.1 (port 8700 by default) and reaches Droppen over outbound connections, so there is no inbound port to open.":
    "只在 127.0.0.1 上監聽（預設連接埠 8700），透過對外連線接入 Droppen，無需開放對內連接埠。",
  "Mac app (recommended)": "Mac 應用程式（推薦）",
  "Command line": "命令列",
  "Claude Code or Codex, installed and signed in on that device": "該裝置上已安裝並登入 Claude Code 或 Codex",

  "Manual download": "手動下載",
  "macOS 13 or later (Apple silicon or Intel), or Linux (x86_64 or aarch64)": "macOS 13 以上（Apple 晶片或 Intel），或 Linux（x86_64、aarch64）",
  "A free Droppen account. You can also create it when you sign the device in.": "一個免費的 Droppen 帳戶，也可以在裝置登入時建立。",
  "latest release on GitHub": "GitHub 上的最新版本",
  "Check the file before you open it:": "打開之前先核對檔案：",
  "The number it prints must be the one the release lists. The install script makes the same check before it installs anything.":
    "印出來的值必須和發布頁上列的一致。安裝腳本在安裝前也會做同樣的核對。",

  "Updating and uninstalling": "更新與解除安裝",
  "The Mac app checks for updates by itself. To update a command-line install, run the install command again: it upgrades in place and keeps your data.":
    "Mac 應用程式會自動檢查更新。以命令列安裝的，再次執行安裝指令即可原地升級，資料保留。",
  "Remove it from your account": "從帳戶裡移除",
  "On {{app}}, open my devices and remove the device. Its keys are rotated, so it cannot open anything written afterwards.":
    "在 {{app}} 的「我的裝置」裡移除這台裝置。金鑰會隨之輪換，之後寫下的內容它都打不開。",
  "Disconnect it": "中斷連線",
  "On the device, Settings → Access → Droppen → Disconnect this device deletes its Droppen keys there.": "在那台裝置上，「設定 → 存取 → Droppen → 中斷這台裝置」會刪除它本地的 Droppen 金鑰。",
  "Remove the program": "刪除程式",
  "The App: quit Droppen and drag it from Applications to the Trash.": "App：結束 Droppen，把它從「應用程式」拖進垃圾桶。",
  "Command line: run the install script again with --uninstall:": "命令列安裝：加上 --uninstall 再執行一次安裝腳本：",
  "Run the install script again with --uninstall:": "加上 --uninstall 再執行一次安裝腳本：",

  "AI tools": "AI 工具",
  "System": "系統",
  "Account": "帳戶",
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
  "Droppen goes on the Mac or Linux device where Claude Code or Codex runs, alongside them. Your phone and other devices need nothing installed: open {{app}} in the browser.":
    "Droppen は、Claude Code か Codex が動く Mac や Linux のデバイスに、それらと並べてインストールします。スマホやほかのデバイスにはインストール不要で、ブラウザで {{app}} を開くだけです。",
  "A Droppen account. Sign-up needs an invite code.": "Droppen アカウント。登録には招待コードが必要です。",
  "How to install on a Mac": "Mac でのインストール方法",

  "Choose your system": "システムを選ぶ",
  "Linux": "Linux",
  "Linux server": "Linux サーバー",
  "Windows": "Windows",
  "Phone / tablet": "スマホ / タブレット",
  "Detected": "このデバイス",

  "Droppen for Mac": "Mac 版 Droppen",
  "Open the app and Droppen is running on your Mac: no terminal, no login page. Then sign it in to Droppen from inside the app.":
    "アプリを開けば、Mac で Droppen が動き出します。ターミナルもログイン画面もいりません。あとはアプリから Droppen にサインインするだけ。",
  "Open the app and Droppen is running on your Mac: no terminal, no login page.": "アプリを開けば、Mac で Droppen が動き出します。ターミナルもログイン画面もいりません。",
  "The App is coming soon": "アプリは近日公開",
  "Until it is out, run Droppen in the background on your Mac: one line, and the same sign-in.": "公開までは、Mac のバックグラウンドで Droppen を動かせます。コマンド 1 行、サインインの手順は同じです。",
  "copy the command": "コマンドをコピー",
  "the steps for running it in the background": "バックグラウンドで動かす手順を見る",
  "Download Droppen App (macOS)": "Droppen アプリをダウンロード（macOS）",
  "macOS 13 or later · Universal: Apple silicon and Intel · Droppen-<version>.dmg": "macOS 13 以降 · ユニバーサル：Apple シリコンと Intel の両方に対応 · Droppen-<バージョン>.dmg",
  "macOS 13 or later · Universal: Apple silicon and Intel": "macOS 13 以降 · ユニバーサル：Apple シリコンと Intel の両方に対応",
  "release notes, checksums and other files": "リリースノート・チェックサム・ほかのファイル",
  "Open the .dmg": ".dmg を開く",
  "The file you just downloaded, from Downloads.": "「ダウンロード」フォルダにある、いまダウンロードしたファイルです。",
  "Drag Droppen to Applications": "Droppen を「アプリケーション」にドラッグ",
  "Like any other Mac app.": "ほかの Mac アプリと同じです。",
  "Open Droppen": "Droppen を開く",
  "It opens a short setup guide, four steps.": "4 ステップの短いセットアップガイドが開きます。",
  "Click Sign in to Droppen in the app": "アプリで「Droppen にサインイン」を押す",
  "In the guide's step 02 (Where to use it), choose “On my phone and other devices too”, then Sign in to Droppen (later: Settings → Access → Droppen). A {{app}} window opens: sign in, then click add this device.":
    "ガイドのステップ 02「使う場所」で「スマートフォンやほかのデバイスでも使う」を選び、「Droppen にサインイン」を押します（あとからは「設定 → アクセス → Droppen」）。{{app}} のウィンドウが開くので、サインインして「このデバイスを追加」を押します。",
  "Using it from your phone: signing in to Droppen from the app is coming in an update. Until then, the app runs Droppen on this Mac.":
    "スマホから使う場合：アプリからの Droppen へのサインインは、今後のアップデートで対応します。それまでは、アプリはこの Mac で Droppen を動かします。",

  "Droppen in the background on a Mac": "Mac のバックグラウンドで Droppen を動かす",
  "For a Mac that stays on as a server, or if you would rather use the terminal: one line, then everything else happens in the browser.":
    "サーバーとして常時動かす Mac や、ターミナル派の方に。コマンド 1 行で、あとはすべてブラウザで完了します。",
  "Paste it into Terminal on that Mac.": "その Mac の「ターミナル」に貼り付けて実行します。",
  "It installs and starts at login": "インストールし、ログイン時に起動",
  "The script installs Droppen, starts it now and at every login, and creates your first local login: it prints it, or asks you to choose one.":
    "スクリプトが Droppen をインストールし、すぐに、そして毎回のログイン時に起動します。最初のローカルログインも作ります。表示されるか、自分で決めるよう求められます。",
  "Its page opens": "ページが開く",
  "Your browser opens Droppen on that device. Sign in with the local login: everything already works there, with no account.":
    "ブラウザでそのデバイスの Droppen が開きます。ローカルログインで入れば、アカウントなしですべて使えます。",
  "Sign in to Droppen": "Droppen にサインイン",
  "Click Sign in to Droppen on that page. A {{app}} window opens: sign in, check the fingerprint matches, and add the device.":
    "そのページで「Droppen にサインイン」を押すと、{{app}} のウィンドウが開きます。サインインし、フィンガープリントが一致することを確かめて、デバイスを追加します。",
  "It is in my devices": "「マイデバイス」に現れる",
  "Within seconds it is online in my devices, on every phone and browser of your account. Add another device the same way, and put both on one screen.":
    "数秒で「マイデバイス」にオンラインで現れ、アカウントのすべてのスマホとブラウザから見えます。同じ手順でもう 1 台追加すれば、2 台をひとつの画面に並べられます。",

  "Droppen on a Linux server": "Linux サーバーで Droppen を動かす",
  "Run your AI on a server that never sleeps, and watch it from your phone. No browser needed on the server: it joins your account with a one-time code.":
    "眠らないサーバーで AI を動かし、スマホから見守れます。サーバーにブラウザは不要。ワンタイムコードでアカウントに加わります。",
  "coming": "近日対応",
  "Linux support, with no browser needed on the server, is on its way. Until then, run Droppen on a Mac.": "Linux 対応（サーバーにブラウザ不要）は準備中です。それまでは Mac で Droppen を動かせます。",
  "run it in the background on a Mac": "Mac のバックグラウンドで動かす",
  "This browser reports Linux on {{arch}}.": "このブラウザは {{arch}} の Linux と報告しています。",
  "Droppen's Linux builds are for x86_64 and aarch64.": "Droppen の Linux 版は x86_64 と aarch64 に対応しています。",
  "<code>": "<コード>",
  "Replace <code> with a one-time code: on {{app}}, open my devices → add a device that runs AI → show a one-time code.":
    "<コード> をワンタイムコードに置き換えます。{{app}} で「マイデバイス → AI を実行できるデバイスを追加 → ワンタイムコードを表示」を開きます。",
  "Get a one-time code": "ワンタイムコードを取得",
  "On {{app}}: my devices → add a device that runs AI → show a one-time code. It is good for 10 minutes, once.": "{{app}} で「マイデバイス → AI を実行できるデバイスを追加 → ワンタイムコードを表示」。有効期間は 10 分、1 回限りです。",
  "Run the command on the server": "サーバーでコマンドを実行",
  "It installs Droppen, starts it as a service, and joins your account with the code.": "Droppen をインストールしてサービスとして起動し、コードでアカウントに加わります。",
  "Within seconds it is online in my devices, on every phone and browser of your account.": "数秒で「マイデバイス」にオンラインで現れ、アカウントのすべてのスマホとブラウザから見えます。",

  "Droppen on Windows": "Windows 版 Droppen",
  "Not supported yet: it is on its way. On Windows you can already watch and answer your AI.": "まだ対応していませんが、準備中です。Windows からでも、AI の作業を見たり質問に答えたりはもうできます。",
  "not yet": "未対応",
  "What works on Windows today:": "Windows で今すぐできること：",
  "Control your other devices from this browser: sign in at {{app}}.": "このブラウザでほかのデバイスを操作する：{{app}} にサインインするだけ。",
  "Run the AI on a Mac, with the Droppen App.": "AI は Mac で動かす：Droppen アプリで。",
  "Run the AI on a Mac: in the background today, with the Droppen App soon.": "AI は Mac で動かす：今はバックグラウンドで、もうすぐアプリでも。",
  "Run the AI on a Linux server, or in WSL with systemd turned on.": "AI は Linux サーバーか、systemd を有効にした WSL で動かす。",
  "how to turn on systemd in WSL": "WSL で systemd を有効にする方法",
  "Once Linux is supported: a Linux server, or WSL.": "Linux 対応後は、Linux サーバーか WSL で。",
  "use {{app}} in this browser": "このブラウザで {{app}} を使う",
  "the Mac options": "Mac の方法を見る",
  "the Linux server options": "Linux サーバーの方法を見る",

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
  "A launchd user agent on macOS, a systemd user service on Linux. With --system, a system service that starts at boot; sudo is used only to write its service file.":
    "macOS では launchd のユーザーエージェント、Linux では systemd のユーザーサービスとして動きます。--system を付けると起動時に開始するシステムサービスになり、sudo はサービスファイルの書き込みにだけ使います。",
  "Listens only on 127.0.0.1 (port 8700 by default) and reaches Droppen over outbound connections, so there is no inbound port to open.":
    "127.0.0.1（既定のポートは 8700）でのみ待ち受け、Droppen には外向きの接続でつながるため、開けるべき受信ポートはありません。",
  "Mac app (recommended)": "Mac アプリ（推奨）",
  "Command line": "コマンドライン",
  "Claude Code or Codex, installed and signed in on that device": "そのデバイスに Claude Code か Codex がインストールされ、ログイン済みであること",

  "Manual download": "手動ダウンロード",
  "macOS 13 or later (Apple silicon or Intel), or Linux (x86_64 or aarch64)": "macOS 13 以降（Apple シリコンまたは Intel）、または Linux（x86_64、aarch64）",
  "A free Droppen account. You can also create it when you sign the device in.": "無料の Droppen アカウント。デバイスのログイン時に作成することもできます。",
  "latest release on GitHub": "GitHub の最新リリース",
  "Check the file before you open it:": "開く前にファイルを確認します：",
  "The number it prints must be the one the release lists. The install script makes the same check before it installs anything.":
    "表示された値がリリースに載っている値と一致すれば OK です。インストールスクリプトも、インストール前に同じ確認をします。",

  "Updating and uninstalling": "アップデートとアンインストール",
  "The Mac app checks for updates by itself. To update a command-line install, run the install command again: it upgrades in place and keeps your data.":
    "Mac アプリはアップデートを自動で確認します。コマンドラインでインストールした場合は、インストールコマンドをもう一度実行すると、データを残したままアップデートされます。",
  "Remove it from your account": "アカウントから削除する",
  "On {{app}}, open my devices and remove the device. Its keys are rotated, so it cannot open anything written afterwards.":
    "{{app}} の「マイデバイス」でそのデバイスを削除します。鍵がローテーションされ、以後に書かれた内容は開けなくなります。",
  "Disconnect it": "接続を解除する",
  "On the device, Settings → Access → Droppen → Disconnect this device deletes its Droppen keys there.":
    "そのデバイスで「設定 → アクセス → Droppen → このデバイスの接続を解除」を選ぶと、そこにある Droppen の鍵が削除されます。",
  "Remove the program": "プログラムを削除する",
  "The App: quit Droppen and drag it from Applications to the Trash.": "アプリ：Droppen を終了し、「アプリケーション」からゴミ箱にドラッグします。",
  "Command line: run the install script again with --uninstall:": "コマンドライン：--uninstall を付けてインストールスクリプトをもう一度実行します：",
  "Run the install script again with --uninstall:": "インストールスクリプトを --uninstall 付きでもう一度実行します：",

  "AI tools": "AI ツール",
  "System": "システム",
  "Account": "アカウント",
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
