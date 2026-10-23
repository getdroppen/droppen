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
  "Download Droppen — your AI, on your phone": "下载 Droppen — 你的 AI，手机上接着用",
  "Get Droppen for the device Claude Code and Codex run on: the Mac app, or one line for a server. Sign it in, and it is on your phone.":
    "给跑 Claude Code 和 Codex 的设备装上 Droppen：Mac 用 App，服务器一行命令。登录之后，手机上就能接着用。",

  // Hero
  "Download": "下载",
  "Install it once, and your AI is on your phone.": "装一次，你的 AI 就到了手机上。",
  "Droppen runs next to Claude Code and Codex on your device. Open it, sign it in to Droppen, and that device is on your phone, and side by side with your other devices on one screen.":
    "Droppen 跟 Claude Code、Codex 一起跑在你的设备上。打开它、登录 Droppen，这台设备就到了你的手机上，也能和你的其他设备放进同一个屏幕。",
  "We picked the way for the device you are on; every other way is one click away.": "已按你现在用的设备选好方式，其它系统点一下就能切换。",
  "Pick the device your AI runs on: every way is one click away.": "选一下跑 AI 的设备：每种方式点一下就能切换。",

  // The tabs
  "Choose your system": "选择你的系统",
  "Mac · App": "Mac · App",
  "Mac · background": "Mac · 后台运行",
  "Linux server": "Linux 服务器",
  "Windows": "Windows",
  "Phone / tablet": "手机 / 平板",
  "Detected": "推荐",

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
  "It starts by itself and opens its own window. Nothing to type.": "它会自己启动、打开自己的窗口，不用输入任何东西。",
  "Click Sign in to Droppen in the app": "在 App 里点「登录 Droppen」",
  "A {{app}} window opens: sign in, check the fingerprint matches, and add the device. It is in my devices within seconds.":
    "会弹出 {{app}} 的窗口：登录，核对两边的指纹一致，再把这台设备加进来。几秒钟后它就出现在「我的设备」里。",
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
  "Run the AI on a Linux server, or in WSL.": "让 AI 跑在 Linux 服务器上，或者 WSL 里。",
  "Once Linux is supported: a Linux server, or WSL.": "等 Linux 支持上线后：Linux 服务器，或者 WSL。",
  "use {{app}} in this browser": "在这个浏览器里用 {{app}}",
  "the Mac options": "看看 Mac 的方式",
  "the Linux server options": "看看 Linux 服务器的方式",

  // Phone / tablet
  "On your phone, Droppen is the remote control": "在手机上，Droppen 是遥控器",
  "The AI runs on your Mac or server; your phone watches it work, answers its questions and opens a terminal. Put {{app}} on your home screen and it opens like an app.":
    "AI 跑在你的 Mac 或服务器上，手机负责看它干活、回答它的提问、打开终端。把 {{app}} 加到主屏幕，用起来就像一个 App。",
  "open {{app}}": "打开 {{app}}",
  "iPhone and iPad (Safari)": "iPhone 和 iPad（Safari）",
  "Open {{app}} in Safari, tap Share, then Add to Home Screen.": "在 Safari 里打开 {{app}}，点「分享」，再点「添加到主屏幕」。",
  "Android (Chrome)": "Android（Chrome）",
  "Open {{app}} in Chrome, tap ⋮, then Add to Home screen or Install app.": "在 Chrome 里打开 {{app}}，点 ⋮，再点「添加到主屏幕」或「安装应用」。",
  "Install it where your AI runs": "装在跑 AI 的设备上",
  "Droppen itself goes on the device your AI runs on. Open this page there:": "Droppen 本身要装在跑 AI 的那台设备上。在那台设备上打开这个页面：",
  "copy": "复制",

  // System requirements
  "System requirements": "系统要求",
  "What it runs on": "能装在哪里",
  "Mac": "Mac",
  "The Droppen App: macOS 13 or later, Apple silicon or Intel, one universal download": "Droppen App：macOS 13 及以上，Apple 芯片或 Intel 都能用，一个通用安装包",
  "Apple silicon or Intel: in the background today, the Droppen App soon": "Apple 芯片或 Intel：现在可以后台运行，App 即将推出",
  "The install command; no browser needed on the server": "一行安装命令，服务器上不需要浏览器",
  "not yet, on its way": "暂不支持，正在路上",
  "to control your devices, in the browser: iPhone, iPad, Android": "在浏览器里控制你的设备：iPhone、iPad、Android",
  "Claude Code or Codex": "Claude Code 或 Codex",
  "installed and signed in on that device: Droppen runs them, it does not replace them": "在那台设备上装好并登录：Droppen 负责运行它们，不替代它们",

  // Manual download and checksums
  "Manual download": "手动下载",
  "Want to see what you install? Download it by hand.": "想先看清楚装的是什么？手动下载也行。",
  "Every release is on GitHub, with the SHA-256 checksum of each file next to it.": "每个版本都发布在 GitHub 上，每个文件旁边都附有它的 SHA-256 校验值。",
  "latest release on GitHub": "GitHub 上的最新版本",
  "Check the file before you open it:": "打开之前先核对文件：",
  "The number it prints must be the one the release lists. The install script makes the same check before it installs anything.":
    "打印出来的值必须和发布页上列的一致。安装脚本在安装前也会做同样的核对。",

  // Uninstall
  "Uninstall": "卸载",
  "Leaving takes three steps, too.": "卸载也只要三步。",
  "Remove it from your account": "从账户里移除",
  "On {{app}}, open my devices and remove the device. Its keys are rotated, so it cannot open anything written afterwards.":
    "在 {{app}} 的「我的设备」里移除这台设备。密钥会随之轮换，之后写下的内容它都打不开。",
  "Disconnect it": "断开连接",
  "On the device, Settings → Access → Droppen → Disconnect this device deletes its Droppen keys there.": "在那台设备上，「设置 → 访问 → Droppen → 断开这台设备」会删除它本地的 Droppen 密钥。",
  "Remove the program": "删除程序",
  "The App: quit Droppen and drag it from Applications to the Trash.": "App：退出 Droppen，把它从「应用程序」拖进废纸篓。",
  "In the background: run the install script again with --uninstall:": "后台运行：加上 --uninstall 再运行一次安装脚本：",
  "Run the install script again with --uninstall:": "加上 --uninstall 再运行一次安装脚本：",

  // The end of the page
  "Ready when you are": "准备好了就开始",
  "Install it, then sign it in to Droppen.": "装好，再登录 Droppen。",
  "No account yet? Sign up with your invite code first, or in the {{app}} window when you sign the device in.": "还没有账号？先用邀请码注册，或者在设备登录时弹出的 {{app}} 窗口里注册。",
  "Open my devices": "打开「我的设备」",
  "Once it is signed in, it is in my devices: add another device the same way to see both on one screen.": "登录之后，它就在「我的设备」里了。再用同样的方法加一台设备，就能在一个屏里同时看两台。",

  // The home page's hero: the way onto the device reading it (hero-get.tsx)
  "Download for Mac": "下载 Mac 版",
  "macOS 13 or later · Apple silicon and Intel": "需要 macOS 13 及以上 · Apple 芯片和 Intel 都能用",
  "other systems": "其他系统",
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
  "Download Droppen — your AI, on your phone": "下載 Droppen — 你的 AI，手機上接著用",
  "Get Droppen for the device Claude Code and Codex run on: the Mac app, or one line for a server. Sign it in, and it is on your phone.":
    "替執行 Claude Code 和 Codex 的裝置裝上 Droppen：Mac 用 App，伺服器一行指令。登入之後，手機上就能接著用。",

  "Download": "下載",
  "Install it once, and your AI is on your phone.": "裝一次，你的 AI 就到了手機上。",
  "Droppen runs next to Claude Code and Codex on your device. Open it, sign it in to Droppen, and that device is on your phone, and side by side with your other devices on one screen.":
    "Droppen 跟 Claude Code、Codex 一起在你的裝置上執行。打開它、登入 Droppen，這台裝置就到了你的手機上，也能和你的其他裝置放進同一個畫面。",
  "We picked the way for the device you are on; every other way is one click away.": "已依你現在用的裝置選好方式，其他系統點一下就能切換。",
  "Pick the device your AI runs on: every way is one click away.": "選一下跑 AI 的裝置：每種方式點一下就能切換。",

  "Choose your system": "選擇你的系統",
  "Mac · App": "Mac · App",
  "Mac · background": "Mac · 背景執行",
  "Linux server": "Linux 伺服器",
  "Windows": "Windows",
  "Phone / tablet": "手機 / 平板",
  "Detected": "推薦",

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
  "It starts by itself and opens its own window. Nothing to type.": "它會自己啟動、打開自己的視窗，不用輸入任何東西。",
  "Click Sign in to Droppen in the app": "在 App 裡點「登入 Droppen」",
  "A {{app}} window opens: sign in, check the fingerprint matches, and add the device. It is in my devices within seconds.":
    "會彈出 {{app}} 的視窗：登入，核對兩邊的指紋一致，再把這台裝置加進來。幾秒鐘後它就出現在「我的裝置」裡。",
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
  "Run the AI on a Linux server, or in WSL.": "讓 AI 在 Linux 伺服器上執行，或者在 WSL 裡。",
  "Once Linux is supported: a Linux server, or WSL.": "等 Linux 支援上線後：Linux 伺服器，或者 WSL。",
  "use {{app}} in this browser": "在這個瀏覽器裡用 {{app}}",
  "the Mac options": "看看 Mac 的方式",
  "the Linux server options": "看看 Linux 伺服器的方式",

  "On your phone, Droppen is the remote control": "在手機上，Droppen 是遙控器",
  "The AI runs on your Mac or server; your phone watches it work, answers its questions and opens a terminal. Put {{app}} on your home screen and it opens like an app.":
    "AI 在你的 Mac 或伺服器上執行，手機負責看它工作、回答它的提問、打開終端機。把 {{app}} 加到主畫面，用起來就像一個 App。",
  "open {{app}}": "打開 {{app}}",
  "iPhone and iPad (Safari)": "iPhone 和 iPad（Safari）",
  "Open {{app}} in Safari, tap Share, then Add to Home Screen.": "在 Safari 裡打開 {{app}}，點「分享」，再點「加入主畫面」。",
  "Android (Chrome)": "Android（Chrome）",
  "Open {{app}} in Chrome, tap ⋮, then Add to Home screen or Install app.": "在 Chrome 裡打開 {{app}}，點 ⋮，再點「新增至主畫面」或「安裝應用程式」。",
  "Install it where your AI runs": "裝在執行 AI 的裝置上",
  "Droppen itself goes on the device your AI runs on. Open this page there:": "Droppen 本身要裝在執行 AI 的那台裝置上。在那台裝置上打開這個頁面：",
  "copy": "複製",

  "System requirements": "系統需求",
  "What it runs on": "能裝在哪裡",
  "Mac": "Mac",
  "The Droppen App: macOS 13 or later, Apple silicon or Intel, one universal download": "Droppen App：macOS 13 以上，Apple 晶片或 Intel 都能用，一個通用安裝檔",
  "Apple silicon or Intel: in the background today, the Droppen App soon": "Apple 晶片或 Intel：現在可以背景執行，App 即將推出",
  "The install command; no browser needed on the server": "一行安裝指令，伺服器上不需要瀏覽器",
  "not yet, on its way": "暫不支援，正在路上",
  "to control your devices, in the browser: iPhone, iPad, Android": "在瀏覽器裡控制你的裝置：iPhone、iPad、Android",
  "Claude Code or Codex": "Claude Code 或 Codex",
  "installed and signed in on that device: Droppen runs them, it does not replace them": "在那台裝置上安裝並登入：Droppen 負責執行它們，不取代它們",

  "Manual download": "手動下載",
  "Want to see what you install? Download it by hand.": "想先看清楚裝的是什麼？手動下載也可以。",
  "Every release is on GitHub, with the SHA-256 checksum of each file next to it.": "每個版本都發布在 GitHub 上，每個檔案旁邊都附有它的 SHA-256 校驗值。",
  "latest release on GitHub": "GitHub 上的最新版本",
  "Check the file before you open it:": "打開之前先核對檔案：",
  "The number it prints must be the one the release lists. The install script makes the same check before it installs anything.":
    "印出來的值必須和發布頁上列的一致。安裝腳本在安裝前也會做同樣的核對。",

  "Uninstall": "解除安裝",
  "Leaving takes three steps, too.": "解除安裝也只要三步。",
  "Remove it from your account": "從帳戶裡移除",
  "On {{app}}, open my devices and remove the device. Its keys are rotated, so it cannot open anything written afterwards.":
    "在 {{app}} 的「我的裝置」裡移除這台裝置。金鑰會隨之輪換，之後寫下的內容它都打不開。",
  "Disconnect it": "中斷連線",
  "On the device, Settings → Access → Droppen → Disconnect this device deletes its Droppen keys there.": "在那台裝置上，「設定 → 存取 → Droppen → 中斷這台裝置」會刪除它本地的 Droppen 金鑰。",
  "Remove the program": "刪除程式",
  "The App: quit Droppen and drag it from Applications to the Trash.": "App：結束 Droppen，把它從「應用程式」拖進垃圾桶。",
  "In the background: run the install script again with --uninstall:": "背景執行：加上 --uninstall 再執行一次安裝腳本：",
  "Run the install script again with --uninstall:": "加上 --uninstall 再執行一次安裝腳本：",

  "Ready when you are": "準備好了就開始",
  "Install it, then sign it in to Droppen.": "裝好，再登入 Droppen。",
  "No account yet? Sign up with your invite code first, or in the {{app}} window when you sign the device in.": "還沒有帳號？先用邀請碼註冊，或在裝置登入時彈出的 {{app}} 視窗裡註冊。",
  "Open my devices": "打開「我的裝置」",
  "Once it is signed in, it is in my devices: add another device the same way to see both on one screen.": "登入之後，它就在「我的裝置」裡了。再用同樣的方法加一台裝置，就能在一個畫面裡同時看兩台。",

  // The home page's hero: the way onto the device reading it (hero-get.tsx)
  "Download for Mac": "下載 Mac 版",
  "macOS 13 or later · Apple silicon and Intel": "需要 macOS 13 以上 · Apple 晶片和 Intel 都能用",
  "other systems": "其他系統",
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
  "Download Droppen — your AI, on your phone": "Droppen をダウンロード — あなたの AI を、スマホで",
  "Get Droppen for the device Claude Code and Codex run on: the Mac app, or one line for a server. Sign it in, and it is on your phone.":
    "Claude Code と Codex が動くデバイスに Droppen を。Mac はアプリ、サーバーはコマンド 1 行。サインインすれば、スマホで続きから使えます。",

  "Download": "ダウンロード",
  "Install it once, and your AI is on your phone.": "一度入れれば、あなたの AI がスマホに。",
  "Droppen runs next to Claude Code and Codex on your device. Open it, sign it in to Droppen, and that device is on your phone, and side by side with your other devices on one screen.":
    "Droppen は、あなたのデバイスで Claude Code や Codex と一緒に動きます。開いて Droppen にサインインすれば、そのデバイスがスマホから使え、ほかのデバイスとひとつの画面に並べられます。",
  "We picked the way for the device you are on; every other way is one click away.": "今お使いのデバイスに合う方法を選んであります。ほかの OS もワンクリックで切り替えられます。",
  "Pick the device your AI runs on: every way is one click away.": "AI を動かすデバイスを選んでください。どの方法もワンクリックで切り替えられます。",

  "Choose your system": "システムを選ぶ",
  "Mac · App": "Mac・アプリ",
  "Mac · background": "Mac・バックグラウンド",
  "Linux server": "Linux サーバー",
  "Windows": "Windows",
  "Phone / tablet": "スマホ / タブレット",
  "Detected": "おすすめ",

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
  "It starts by itself and opens its own window. Nothing to type.": "自動で起動し、自分のウィンドウを開きます。入力するものはありません。",
  "Click Sign in to Droppen in the app": "アプリで「Droppen にサインイン」を押す",
  "A {{app}} window opens: sign in, check the fingerprint matches, and add the device. It is in my devices within seconds.":
    "{{app}} のウィンドウが開きます。サインインし、フィンガープリントが一致することを確かめて、デバイスを追加します。数秒で「マイデバイス」に現れます。",
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
  "Run the AI on a Linux server, or in WSL.": "AI は Linux サーバーか WSL で動かす。",
  "Once Linux is supported: a Linux server, or WSL.": "Linux 対応後は、Linux サーバーか WSL で。",
  "use {{app}} in this browser": "このブラウザで {{app}} を使う",
  "the Mac options": "Mac の方法を見る",
  "the Linux server options": "Linux サーバーの方法を見る",

  "On your phone, Droppen is the remote control": "スマホでは、Droppen はリモコンです",
  "The AI runs on your Mac or server; your phone watches it work, answers its questions and opens a terminal. Put {{app}} on your home screen and it opens like an app.":
    "AI は Mac やサーバーで動き、スマホでその作業を見て、質問に答え、ターミナルを開きます。{{app}} をホーム画面に追加すれば、アプリのように開けます。",
  "open {{app}}": "{{app}} を開く",
  "iPhone and iPad (Safari)": "iPhone と iPad（Safari）",
  "Open {{app}} in Safari, tap Share, then Add to Home Screen.": "Safari で {{app}} を開き、「共有」から「ホーム画面に追加」をタップします。",
  "Android (Chrome)": "Android（Chrome）",
  "Open {{app}} in Chrome, tap ⋮, then Add to Home screen or Install app.": "Chrome で {{app}} を開き、⋮ から「ホーム画面に追加」または「アプリをインストール」をタップします。",
  "Install it where your AI runs": "AI を動かすデバイスにインストール",
  "Droppen itself goes on the device your AI runs on. Open this page there:": "Droppen 本体は AI を動かすデバイスに入れます。そのデバイスでこのページを開いてください：",
  "copy": "コピー",

  "System requirements": "動作環境",
  "What it runs on": "対応環境",
  "Mac": "Mac",
  "The Droppen App: macOS 13 or later, Apple silicon or Intel, one universal download": "Droppen アプリ：macOS 13 以降、Apple シリコンと Intel に対応したユニバーサル版",
  "Apple silicon or Intel: in the background today, the Droppen App soon": "Apple シリコンまたは Intel：今はバックグラウンドで、アプリは近日公開",
  "The install command; no browser needed on the server": "インストールコマンドで。サーバーにブラウザは不要",
  "not yet, on its way": "未対応（準備中）",
  "to control your devices, in the browser: iPhone, iPad, Android": "ブラウザからデバイスを操作：iPhone、iPad、Android",
  "Claude Code or Codex": "Claude Code または Codex",
  "installed and signed in on that device: Droppen runs them, it does not replace them":
    "そのデバイスにインストールしてサインイン済みであること。Droppen はそれらを動かすもので、置き換えるものではありません",

  "Manual download": "手動ダウンロード",
  "Want to see what you install? Download it by hand.": "何をインストールするか確かめたいなら、手動でダウンロードできます。",
  "Every release is on GitHub, with the SHA-256 checksum of each file next to it.": "すべてのリリースは GitHub にあり、各ファイルの SHA-256 チェックサムが添えられています。",
  "latest release on GitHub": "GitHub の最新リリース",
  "Check the file before you open it:": "開く前にファイルを確認します：",
  "The number it prints must be the one the release lists. The install script makes the same check before it installs anything.":
    "表示された値がリリースに載っている値と一致すれば OK です。インストールスクリプトも、インストール前に同じ確認をします。",

  "Uninstall": "アンインストール",
  "Leaving takes three steps, too.": "やめるときも 3 ステップです。",
  "Remove it from your account": "アカウントから削除する",
  "On {{app}}, open my devices and remove the device. Its keys are rotated, so it cannot open anything written afterwards.":
    "{{app}} の「マイデバイス」でそのデバイスを削除します。鍵がローテーションされ、以後に書かれた内容は開けなくなります。",
  "Disconnect it": "接続を解除する",
  "On the device, Settings → Access → Droppen → Disconnect this device deletes its Droppen keys there.":
    "そのデバイスで「設定 → アクセス → Droppen → このデバイスの接続を解除」を選ぶと、そこにある Droppen の鍵が削除されます。",
  "Remove the program": "プログラムを削除する",
  "The App: quit Droppen and drag it from Applications to the Trash.": "アプリ：Droppen を終了し、「アプリケーション」からゴミ箱にドラッグします。",
  "In the background: run the install script again with --uninstall:": "バックグラウンド版：インストールスクリプトを --uninstall 付きでもう一度実行します：",
  "Run the install script again with --uninstall:": "インストールスクリプトを --uninstall 付きでもう一度実行します：",

  "Ready when you are": "準備ができたら",
  "Install it, then sign it in to Droppen.": "インストールして、Droppen にサインイン。",
  "No account yet? Sign up with your invite code first, or in the {{app}} window when you sign the device in.":
    "アカウントがまだなら、先に招待コードで登録するか、デバイスをサインインするときに開く {{app}} のウィンドウで登録できます。",
  "Open my devices": "「マイデバイス」を開く",
  "Once it is signed in, it is in my devices: add another device the same way to see both on one screen.":
    "サインインすれば「マイデバイス」に入ります。同じ手順でもう 1 台追加すれば、2 台をひとつの画面で見られます。",

  // The home page's hero: the way onto the device reading it (hero-get.tsx)
  "Download for Mac": "Mac 版をダウンロード",
  "macOS 13 or later · Apple silicon and Intel": "macOS 13 以降 · Apple シリコンと Intel に対応",
  "other systems": "ほかの OS",
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
