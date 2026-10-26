// The zh-CN table of `onboarding.ts`, which says what it covers: a file of its
// own, so that it is in the zh-CN chunk alone (`lang-zh-CN.ts`).

export const onboardingZhCN: Record<string, string> = {
  // The App first, the server way second
  "01 · install the Droppen App on that device": "01 · 在那台设备上安装 Droppen App",
  "On the device your AI runs on (a Mac, for now), download the Droppen App and open it: Droppen starts there by itself, with no login page.": "在跑 AI 的那台设备上（目前支持 Mac）下载并打开 Droppen App：Droppen 会自己在那台设备上启动，不需要登录页。",
  "Download Droppen App (macOS)": "下载 Droppen App（macOS）",
  "The Droppen App for Mac is coming soon; until then, Droppen runs in the background.": "Mac 版 Droppen App 即将推出，在那之前先让 Droppen 在后台运行。",
  "The Droppen App cannot sign in to Droppen yet (that is coming in an update): to add a Mac now, run Droppen in the background.":
    "Droppen App 暂时还不能登录 Droppen（会在后续更新中推出）。现在要添加 Mac，请先让 Droppen 在后台运行。",
  "02 · in the App, click Sign in to Droppen": "02 · 在 App 里点「登录 Droppen」",
  "In the Droppen App on that device, click Sign in to Droppen (it is also under Settings → Access → Droppen).": "在那台设备的 Droppen App 里点「登录 Droppen」（也在「设置 → 访问 → Droppen」里）。",
  "Server or running in the background?": "服务器或后台运行？",
  "Install it with one line:": "一行命令安装：",
  "Mac in the background": "Mac 后台运行",
  "Linux server": "Linux 服务器",
  "— no browser on it? Add it with a one-time code:": "没有浏览器？用一次性注册码添加：",
  // The guide
  "01 · install Droppen on that device": "01 · 在那台设备上安装 Droppen",
  "On the device your AI runs on (a Mac, for now), open Terminal and run:": "在跑 AI 的那台设备上（目前支持 Mac），打开「终端」运行：",
  "download page": "下载页",
  "It starts Droppen there, makes a local login for it, and opens its page.": "它会在那台设备上启动 Droppen，创建本地登录，并打开它的页面。",
  "02 · on its page, sign in to Droppen": "02 · 在它的页面上登录 Droppen",
  "On the page that opens on that device, sign in with its local login and click Sign in to Droppen (it is also under Settings → Access → Droppen).":
    "在那台设备打开的页面上，用本地登录进去，点「登录 Droppen」（也在「设置 → 访问 → Droppen」里）。",
  "A {{app}} window opens there: sign in to this account, check that the fingerprint matches the one its page shows, and click add this device.":
    "那边会弹出 {{app}} 窗口：登录这个账户，核对指纹和它页面上显示的一致，然后点「加入这台设备」。",
  "This account is on {{hub}}, not droppen.app: on that page, open advanced first and set the hub address to {{hub}}.":
    "这个账户在 {{hub}} 上，不在 droppen.app：先在那个页面上打开「高级选项」，把 Hub 地址设成 {{hub}}。",
  "Is that window's browser new to your account? It shows a code first: add it from here.": "那个窗口所在的浏览器还没加入你的账户？它会先显示一个配对码，在这里添加它：",
  "03 · it comes online here": "03 · 在这里等它上线",
  "{{name}} joined the account; waiting for it to come online…": "{{name}} 已加入账户，正在等它上线…",
  "waiting for the new device to come online…": "正在等新设备上线…",
  "Keep this page open: it notices the new device by itself.": "这个页面开着就行，新设备上线会自动提示。",
  "{{name}} is online": "{{name}} 已上线",
  "{{name}} is in your account": "{{name}} 已加入你的账户",
  "Now you can put the two devices on one screen.": "现在可以把两台设备放进同一个屏幕",
  "{{a}} and {{b}}, side by side: each pane opens on its own device's latest session, or a terminal on it, and you can change what each one shows.":
    "{{a}} 和 {{b}} 并排：每个格子打开各自设备上最近的会话（没有会话就开一个终端），每个格子里放什么都可以再换。",
  "Open it to start a session there. Add another device the same way to see both on one screen.": "打开它，就能在上面开会话。用同样的方法再加一台设备，就能在一个屏里同时看。",
  "put them on one screen": "放进同一个屏幕",
  "open {{name}}": "打开 {{name}}",
  "{{name}} keeps your screens and cannot be reached right now. Try again once it is online.": "屏幕保存在 {{name}} 上，它现在连不上。等它上线后再试。",
  "The screen could not be made: {{message}}": "屏幕没能建好：{{message}}",

  // 「我的设备」's first-run card, while the account has no device that runs AI
  "next step": "下一步",
  "Connect the device your AI runs on": "连接跑 AI 的那台设备",
  "Install Droppen on the Mac or Linux device where you use Claude Code or Codex. Then you can watch and answer your AI from here, and from your phone.": "在你用 Claude Code 或 Codex 的那台 Mac 或 Linux 设备上装好 Droppen，之后就能在这里和手机上看 AI 干活、回答它的问题。",
  "You need:": "需要：",
  "Claude Code or Codex, installed and signed in on that device.": "那台设备上已安装并登录 Claude Code 或 Codex。",
  "Download Droppen for Mac": "下载 Mac 版 Droppen",
  "macOS 13 or later": "macOS 13 或更高版本",
  "macOS 13 or later, Apple silicon or Intel": "macOS 13 或更高版本，Apple 芯片或 Intel",
  "Install it": "安装",
  "Open the .dmg and drag Droppen to Applications.": "打开 .dmg，把 Droppen 拖进「应用程序」。",
  "Open Droppen": "打开 Droppen",
  "It opens a short setup guide, four steps.": "它会打开一个四步的设置向导。",
  "In the guide's step 02 (Where to use it), choose “On my phone and other devices too”, then Sign in to Droppen (later: Settings → Access → Droppen). A {{app}} window opens: sign in, then click add this device.": "在向导第 02 步「在哪里用」选「在手机和其它设备上也能用」，再点「登录 Droppen」（之后也可以在「设置 → 访问 → Droppen」里找到）。会弹出 {{app}} 窗口：登录后点「加入这台设备」。",
  "Sign in to Droppen in the App": "在 App 里点「登录 Droppen」",
  "A {{app}} window opens: sign in, then click add this device.": "会弹出 {{app}} 窗口：登录后点「加入这台设备」。",
  "Open Terminal on that Mac and run:": "在那台 Mac 上打开「终端」运行：",
  "Run this in a terminal on that device:": "在那台设备的终端里运行：",
  "On a Chromebook, turn on Linux first: in Settings, search for “Linux development environment”.": "Chromebook 上要先开启 Linux：在「设置」里搜索「Linux 开发环境」。",
  "Run the command": "运行命令",
  "It installs and starts Droppen, and makes a local login for it.": "它会安装并启动 Droppen，并创建一个本地登录。",
  "Sign in on its page": "在它的页面上登录",
  "On the page it opens, sign in with that login, then click Sign in to Droppen.": "在它打开的页面上用这个本地登录进去，再点「登录 Droppen」。",
  "Add it": "加入账户",
  "No browser on that device, like a server?": "那台设备没有浏览器（比如服务器）？",
  "use a one-time code": "用一次性注册码",
  "There is no Windows app yet.": "暂时还没有 Windows 版 App。",
  "Your AI needs a Mac or a Linux device with Droppen on it (WSL works too, with systemd turned on). Once one is connected, use {{app}} right here in this browser to watch and answer it.": "AI 要跑在装了 Droppen 的 Mac 或 Linux 设备上（开启了 systemd 的 WSL 也可以）。连上一台之后，就能在这个浏览器里用 {{app}} 看它干活、回答它。",
  "how to turn on systemd in WSL": "如何在 WSL 里开启 systemd",
  "set up a Mac": "在 Mac 上安装",
  "set up Linux or WSL": "在 Linux 或 WSL 上安装",
  "Set up Droppen": "安装 Droppen",
  "Set it up on the device your AI runs on.": "装在跑 AI 的那台设备上。",
  "Droppen goes on the Mac or Linux device where your AI runs, not on this one. Open this link there:": "Droppen 要装在跑 AI 的 Mac 或 Linux 设备上，不是这台。在那台设备上打开这个链接：",
  "share link": "分享链接",
  "Once it is connected, you can watch and answer your AI from here too.": "连上之后，在这里也能看 AI 干活、回答它。",
  "Droppen there opens a sign-in window. If it shows a code first, add it from here:": "那台设备上的 Droppen 会弹出登录窗口；如果它先显示一个配对码，在这里添加它：",
  "Another kind of device?": "换一种设备？",
  "Mac": "Mac",
  "Linux": "Linux",
  "Windows": "Windows",
  "phone or tablet": "手机或平板",
  "all the ways to add one": "所有添加方式",
  "more settings": "更多设置",
  "connections, relays and keys": "连接、中继和密钥",

  // The hints
  "Add another device to see both on one screen.": "再加一台设备，就能在一个屏里同时看",
  "add": "添加",
  "Each pane can show a different device's content.": "每个格子都可以放不同设备的内容",
  "Make a screen with the rail's +, then pick a device in each pane.": "用左侧栏的「+」建一个屏幕，再给每个格子选设备。",

  // The local page's first run
  "Sign in to Droppen to see your other devices here, and to use this one from your phone.": "登录 Droppen，就能在这里看到你的其它设备，也能在手机上用这一台。",
  "not now": "以后再说",

  // droppen.app/link opened by hand
  "Droppen is not on that device yet?": "那台设备上还没装 Droppen？",

  // The one-time code
  "For a Linux server, or any device with no browser: it joins your account from its command line.": "适用于 Linux 服务器，或任何没有浏览器的设备：它从命令行加入你的账户。",
  "The code expires in {{time}}.": "注册码有效期还剩 {{time}}。",
  "Keep this page open: it is what adds the device once the command runs there.": "请保持这个页面打开：命令在那台设备上运行后，由这个页面完成添加。",
  "This code has expired.": "这个注册码已过期。",
  "No device used it in time. Get a new code, and run the new command on that device.": "没有设备及时用上它。换一个新注册码，再在那台设备上运行新的命令。",
  "new code": "换个新注册码",
};
