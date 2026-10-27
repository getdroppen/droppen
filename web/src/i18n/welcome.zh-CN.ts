// The zh-CN table of `welcome.ts`, which says what it covers: a file of its
// own, so that it is in the zh-CN chunk alone (`lang-zh-CN.ts`).

export const welcomeZhCN: Record<string, string> = {
  // Frame: top bar, step rail, bottom bar
  "Get started with Droppen": "开始使用 Droppen", // shell.setup
  "Finish later": "稍后再说", // shell.later
  "What you've done is kept. Continue from the bar above the session list, or from Settings → General → Setup.": "已经做的都会保留。可以从会话列表上方的提示条，或 设置 → 通用 → 设置引导 继续。", // shell.laterHint
  "Skip this step": "跳过这一步", // shell.skipStep
  "Back": "上一步", // shell.back
  "Continue": "继续", // shell.continue
  "Step {{n}} of 4": "第 {{n}} 步，共 4 步", // shell.stepOf
  "AI tools": "AI 工具", // shell.step.tools
  "Where to use it": "在哪里用", // shell.step.where
  "First session": "第一个会话", // shell.step.first
  "Finishing up": "收尾", // shell.step.wrap
  "skipped": "已跳过", // shell.state.skipped
  "Every step can be skipped and changed later in Settings.": "每一步都能跳过，之后都能在「设置」里改。", // shell.railFoot
  "Offline. Installing, signing in and Droppen need the internet; everything else works.": "现在没有联网：安装、登录和 Droppen 需要联网，其它都能用。", // shell.offline
  "Language": "语言", // shell.language
  // Welcome (A.2)
  "Droppen for Mac": "Droppen · Mac 版", // welcome.kicker
  "Droppen is running on this Mac.": "Droppen 已经在这台 Mac 上运行了。", // welcome.title
  "About three minutes to put it to work: get Claude Code or Codex ready, decide where you'll use it, then start your first session. Skip anything you like; all of it can be changed later in Settings.": "花 3 分钟左右把它用起来：准备好 Claude Code 或 Codex，决定在哪里用，然后开始第一个会话。哪一步都可以跳过，之后都能在「设置」里改。", // welcome.body
  "Find Claude Code and Codex; install and sign in where needed.": "找到 Claude Code 和 Codex，需要时安装、登录。", // welcome.sum.tools
  "Only on this Mac, or on your phone and other devices too.": "只在这台 Mac 上用，还是手机和其它设备也要用。", // welcome.sum.where
  "Pick a folder and send a first message.": "选一个文件夹，发出第一条消息。", // welcome.sum.first
  "Start at login, notifications, your phone.": "登录时启动、通知、手机。", // welcome.sum.wrap
  "Already looking for Claude Code and Codex on this device…": "已经在这台设备上查找 Claude Code 和 Codex…", // welcome.checking
  "Found: {{list}}": "已找到：{{list}}", // welcome.found
  "Get started": "开始", // welcome.start
  "Skip setup and open Droppen": "跳过设置，直接打开", // welcome.skipAll
  "Droppen is running from a temporary copy. Move it to Applications so it can start at login.": "Droppen 正在从一份临时副本运行。移到「应用程序」文件夹后，登录时启动才能用。", // welcome.moved
  "Move to Applications and reopen": "移到「应用程序」并重新打开", // welcome.move
  // Step 01 · AI tools (A.3)
  "01 · AI tools": "01 · AI 工具", // tools.kicker
  "Get Claude Code or Codex ready": "准备好 Claude Code 或 Codex", // tools.title
  "Droppen runs these AI tools on this device. One is enough; you can add the other any time. Droppen only asks each tool for its own status. It never reads your keychain or saved passwords.": "Droppen 在这台设备上运行这些 AI 工具。有一个就够了，另一个随时可以再加。Droppen 只问工具自己的状态，不读钥匙串，也不读保存的密码。", // tools.body
  "by Anthropic": "Anthropic 出品", // tools.by.claude
  "by OpenAI": "OpenAI 出品", // tools.by.codex
  "Needs a Claude Pro, Max, Team or Enterprise plan, or an Anthropic Console account.": "需要 Claude Pro、Max、Team 或 Enterprise 订阅，或 Anthropic Console 账号。", // tools.needs.claude
  "Works with a ChatGPT plan (Plus, Pro, Business, Edu, Enterprise; Free and Go with lower limits) or an OpenAI API key.": "用 ChatGPT 订阅（Plus、Pro、Business、Edu、Enterprise；Free 和 Go 额度较少）或 OpenAI API key。", // tools.needs.codex
  "checking": "检查中", // tools.st.checking
  "not installed": "未安装", // tools.st.notInstalled
  "installing": "安装中", // tools.st.installing
  "not signed in": "未登录", // tools.st.signedOut
  "signing in": "登录中", // tools.st.signingIn
  "ready": "已就绪", // tools.st.ready
  "update needed": "需要更新", // tools.st.tooOld
  "not found": "没找到", // tools.st.notFound
  "plan doesn't include it": "订阅不含", // tools.st.plan
  "Claude Code isn't on this device yet.": "这台设备上还没有 Claude Code。", // tools.notInstalled.claude
  "Codex isn't on this device yet.": "这台设备上还没有 Codex。", // tools.notInstalled.codex
  "Install…": "安装…", // tools.install
  "How to install": "安装方式", // tools.method
  "Official installer": "官方安装程序", // tools.method.native
  "Homebrew": "Homebrew", // tools.method.brew
  "npm": "npm", // tools.method.npm
  "recommended": "推荐", // tools.recommended
  "Droppen will run this command": "Droppen 将运行这条命令", // tools.confirm
  "From Anthropic (claude.ai): the command in Claude Code's own install guide.": "来自 Anthropic（claude.ai），就是 Claude Code 官方安装说明里的命令。", // tools.fact.claude.1
  "Installs into your home folder (~/.local/bin/claude). No administrator password, no Node.js.": "装到你的个人文件夹（~/.local/bin/claude），不需要管理员密码，也不需要 Node.js。", // tools.fact.claude.2
  "Updates itself in the background afterwards. Takes about a minute.": "之后会在后台自动更新。大约需要 1 分钟。", // tools.fact.claude.3
  "From OpenAI (chatgpt.com): the command in Codex's own README.": "来自 OpenAI（chatgpt.com），就是 Codex 官方说明里的命令。", // tools.fact.codex.1
  "Installs into ~/.local/bin/codex and checks every download's SHA-256. No administrator password, no Node.js.": "装到 ~/.local/bin/codex，并核对每个下载文件的 SHA-256。不需要管理员密码，也不需要 Node.js。", // tools.fact.codex.2
  "Adds ~/.local/bin to PATH in ~/.zprofile so your own Terminal finds codex too. That is what the official installer does anyway.": "会在 ~/.zprofile 里把 ~/.local/bin 加进 PATH，让你自己的终端也能找到 codex——官方安装程序本来就这么做。", // tools.fact.codex.3
  "Uses your Homebrew. Homebrew copies don't update themselves; Droppen offers the update when one is out.": "用你已有的 Homebrew 安装。Homebrew 装的不会自己更新，有新版本时 Droppen 会提示。", // tools.fact.brew
  "Uses your Node.js ({{node}}). Only if you already manage tools with npm.": "用你已有的 Node.js（{{node}}）。只建议已经用 npm 管理工具的人选。", // tools.fact.npm
  "Run it here": "在这里运行", // tools.runHere
  "Copy command": "复制命令", // tools.copyCommand
  "Copied": "已复制", // tools.copied
  "Cancel": "取消", // tools.cancel
  "Official guide": "官方说明", // tools.guide
  "Installing Claude Code. It keeps going if you leave this step.": "正在安装 Claude Code，离开这一步也会继续。", // tools.installing.claude
  "Installing Codex. It keeps going if you leave this step.": "正在安装 Codex，离开这一步也会继续。", // tools.installing.codex
  "Stop": "停止", // tools.stop
  "Terminal · on this device": "终端 · 在这台设备上", // tools.terminal
  "F6 moves between the terminal and the buttons": "按 F6 在终端和按钮之间切换", // tools.f6
  "The install didn't finish (exit code 1): couldn't reach claude.ai. Check the connection and try again, or run the command in your own Terminal.": "安装没有完成（退出码 1）：连不上 claude.ai。检查网络后重试，或者在你自己的终端里运行这条命令。", // tools.failed.claude
  "Try again": "重试", // tools.retry
  "Installed. Sign in once and Droppen can start Claude Code sessions. Your browser opens; sign in with your Claude account.": "已经装好。登录一次，Droppen 就能启动 Claude Code 会话。会打开浏览器，用你的 Claude 账号登录。", // tools.signedOut.claude
  "Installed. Sign in once and Droppen can start Codex sessions. Your browser opens; sign in with ChatGPT.": "已经装好。登录一次，Droppen 就能启动 Codex 会话。会打开浏览器，用 ChatGPT 登录。", // tools.signedOut.codex
  "Sign in": "登录", // tools.signIn
  "Use an Anthropic Console account instead": "改用 Anthropic Console 账号", // tools.signIn.console
  "Use an OpenAI API key instead": "改用 OpenAI API key", // tools.signIn.apiKey
  "Finish signing in in your browser, then come back. Droppen notices by itself.": "在浏览器里完成登录，然后回到这里——Droppen 会自己发现。", // tools.signingIn
  "Waiting for sign-in…": "等待登录…", // tools.waiting
  "Browser didn't open?": "浏览器没打开？", // tools.noBrowser
  "Press c in the terminal to copy the sign-in link, open it in any browser, then paste the code it shows back into the terminal.": "在终端里按 c 复制登录链接，用任意浏览器打开，再把页面上显示的代码粘贴回终端。", // tools.noBrowser.claude
  "Use a one-time code instead: Droppen runs codex login --device-auth and shows a code to enter at chatgpt.com.": "改用一次性代码：Droppen 会运行 codex login --device-auth，并显示要在 chatgpt.com 上输入的代码。", // tools.noBrowser.codex
  "Sign in with a code": "用代码登录", // tools.useCode
  "5 hours": "5 小时", // tools.q.5h
  "this week": "本周", // tools.q.week
  "{{n}}% used": "已用 {{n}}%", // tools.q.used
  "resets {{t}}": "{{t}} 重置", // tools.q.resets
  "Quota shows up after the first session.": "第一次会话之后会显示额度。", // tools.q.later
  "Use a different account": "换个账号", // tools.switch
  "Claude Code {{v}} is too old for Droppen; it needs {{min}} or later.": "Claude Code {{v}} 太旧了，Droppen 需要 {{min}} 或更新的版本。", // tools.tooOld
  "Update": "更新", // tools.update
  "Droppen couldn't read your Terminal's settings in time (over 5 s), so a copy installed with nvm, volta or similar may be hiding. Look again, or show Droppen where it is.": "没能及时读到你终端的设置（超过 5 秒），用 nvm、volta 等装的可能没被找到。再找一次，或者告诉 Droppen 它在哪里。", // tools.notFound
  "Look again (wait longer)": "再找一次（多等一会儿）", // tools.lookAgain
  "Show where it is…": "指定位置…", // tools.choosePath
  "Install a fresh copy": "仍然安装一份", // tools.installAnyway
  "Signed in as {{email}}, but the free Claude plan doesn't include Claude Code. Upgrade at claude.ai, or sign in with a Console account.": "已经以 {{email}} 登录，但 Claude 免费版不含 Claude Code。去 claude.ai 升级，或者改用 Console 账号登录。", // tools.plan.claude
  "Plans at claude.ai": "去 claude.ai 看订阅", // tools.upgrade
  "Your Terminal sets ANTHROPIC_API_KEY. Droppen doesn't take keys from your shell settings; to use that key here, add it to Droppen below.": "你的终端里设置了 ANTHROPIC_API_KEY。Droppen 不读 shell 配置里的密钥；想让这里的会话也用它，就在下面把它加进 Droppen。", // tools.envKey
  "Ready through the Droppen account “{{label}}”": "通过 Droppen 账号「{{label}}」就绪", // tools.viaAccount
  "Developer tools (git)": "开发者工具（git）", // tools.git
  "Claude Code and Codex use git in code projects. Without it, macOS interrupts the first session to install Apple's Command Line Tools; you can get that done now.": "Claude Code 和 Codex 在代码项目里会用 git。没装的话，macOS 会在第一个会话里弹窗要你安装 Apple 的命令行工具；可以现在先装好。", // tools.git.body
  "Install (Apple)": "安装（Apple 官方）", // tools.git.install
  "optional": "可选", // tools.optional
  "Add accounts to Droppen (advanced)": "把账号加到 Droppen（高级）", // tools.adv
  "For switching accounts automatically when one runs out of quota, or paying through an API key or a company gateway. Most people don't need this. Keys stay on this device and are never shown again.": "适合额度用完时自动换号，或者用 API key、公司网关计费。大多数人用不到。密钥只存在这台设备上，存好后不再显示。", // tools.adv.body
  "Claude · long-lived sign-in (setup-token)": "Claude · 长期登录令牌（setup-token）", // tools.adv.token
  "Sign in once in the browser; Droppen keeps a one-year token. Needed for automatic account switching.": "在浏览器里登录一次，Droppen 保存一个一年有效的令牌。自动换号需要它。", // tools.adv.token.body
  "Environment variables (API key, gateway)": "环境变量（API key、网关）", // tools.adv.env
  "One KEY=VALUE per line, given only to the sessions this account starts.": "每行一个 KEY=VALUE，只交给用这个账号启动的会话。", // tools.adv.env.body
  "Codex · OpenAI API key": "Codex · OpenAI API key", // tools.adv.codex
  "Name": "名称", // tools.adv.name
  "Add account": "添加账号", // tools.adv.add
  "Start": "开始", // tools.adv.start
  "Rotation and the full list are in Settings → Accounts.": "换号设置和完整列表在 设置 → 账号。", // tools.adv.more
  "Continue without AI tools": "先不装，继续", // tools.none
  "Without Claude Code or Codex, Droppen can show files and open terminals but can't run AI. Come back any time from Settings → General → Setup.": "没有 Claude Code 或 Codex，Droppen 能看文件、开终端，但不能运行 AI。随时可以从 设置 → 通用 → 设置引导 回来。", // tools.none.body
  // Step 02 · Where to use it (A.4)
  "02 · Where to use it": "02 · 在哪里用", // where.kicker
  "Where will you use Droppen?": "你会在哪里用 Droppen？", // where.title
  "Droppen runs on this Mac either way. This only decides what else can reach it, and you can change it any time in Settings.": "不管选哪个，Droppen 都在这台 Mac 上运行；这里只决定还有什么能连到它，随时可以在「设置」里改。", // where.body
  "On my phone and other devices too": "在手机和其它设备上也能用", // where.droppen.title
  "Sign in to Droppen": "登录 Droppen", // where.droppen.action
  "Open droppen.app on your phone or any other device and carry on with this Mac's sessions. Content is end-to-end encrypted; droppen.app can't read it. Your browser opens to sign in or create an account.": "在手机或其它设备上打开 droppen.app，接着用这台 Mac 上的会话。内容端到端加密，droppen.app 看不到。会打开浏览器登录或注册。", // where.droppen.body
  "Recommended": "推荐", // where.recommended
  "Only on this Mac": "只在这台 Mac 上用", // where.local.title
  "Skip": "跳过", // where.local.action
  "Everything works without an account. Nothing leaves this Mac except what Claude Code and Codex send to their own services.": "不需要账号，一切照常。除了 Claude Code 和 Codex 发给它们自己服务的内容，什么都不离开这台 Mac。", // where.local.body
  "Also": "另外", // where.also
  "Let browsers on my local network in too": "让局域网里的浏览器也能访问", // where.lan.title
  "Set a password for this device (advanced)": "设置这台设备的密码（高级）", // where.lan.action
  "An iPad or another device on the same Wi‑Fi opens this Mac's address in a browser and signs in with the password. Doesn't go through droppen.app, and works alongside either choice above.": "同一个 Wi‑Fi 下的 iPad 或其它设备，用浏览器打开这台 Mac 的地址，输入密码进入。不经过 droppen.app，可以和上面任一选项同时用。", // where.lan.body
  "The Droppen app on this Mac never asks for a password. For a browser on this Mac, choose Open in Browser in the app's menu-bar menu: it opens signed in. Once this device has a password, a browser asks for it.": "这台 Mac 上的 Droppen 应用永远不用密码。这台 Mac 上的浏览器，在应用的菜单栏菜单里选「在浏览器中打开」，打开时已经登录。给这台设备设了密码以后，浏览器会要这个密码。", // where.browserNote
  "Sign in in the browser": "在浏览器中登录", // where.go.droppen
  "Set the password": "设置密码", // where.go.lan
  "Change it later in Settings → Access.": "之后在 设置 → 访问 里改。", // where.changeLater
  "droppen.app needs the internet. Choose “Only on this Mac” for now; sign in later from Settings → Access → Droppen.": "登录 droppen.app 需要联网。先选「只在这台 Mac 上用」，之后在 设置 → 访问 → Droppen 里登录。", // where.offline
  "Continue in your browser": "在浏览器里继续", // where.wait.title
  "Sign in to droppen.app, or create an account.": "登录 droppen.app，或者注册一个账户。", // where.wait.1
  "Check that the fingerprint there matches this one, then choose Allow.": "核对那边显示的指纹和这里一致，然后点「同意」。", // where.wait.2
  "Come back here. This Mac joins your account by itself.": "回到这里，这台 Mac 会自己加入你的账户。", // where.wait.3
  "Fingerprint": "指纹", // where.fingerprint
  "Open the browser again": "重新打开浏览器", // where.reopen
  "Copy link": "复制链接", // where.copyLink
  "The link works for 10 minutes ({{left}} left).": "链接 10 分钟内有效（还剩 {{left}}）。", // where.expires
  "Waiting for droppen.app…": "等待 droppen.app…", // where.waiting
  "Keep going; I'll finish in the browser": "先继续，浏览器里登录好了会提示我", // where.keepGoing
  "This Mac is in your Droppen account": "这台 Mac 已加入你的 Droppen 账户", // where.linked.title
  "Open droppen.app on your phone and sign in as {{email}}. “{{device}}” is there.": "在手机上打开 droppen.app，用 {{email}} 登录，就能看到「{{device}}」。", // where.linked.body
  "Device name": "设备名称", // where.deviceName
  "Scan with your phone": "用手机扫一扫", // where.scan
  "Can't reach droppen.app": "连不上 droppen.app", // where.err.title
  "Use Droppen on this Mac for now. When you're online, sign in from Settings → Access → Droppen; nothing done here is lost.": "先只在这台 Mac 上用。联网后在 设置 → 访问 → Droppen 里登录，这里做的都不会丢。", // where.err.body
  "Your plan's devices that run AI are all in use. Remove one at droppen.app or change plan; until then this Mac works on its own.": "你的套餐里可运行 AI 的设备已经用满。到 droppen.app 移除一台或换套餐；在那之前这台 Mac 照常单独使用。", // where.err.limit
  "Only on this Mac for now": "先只在这台 Mac 上用", // where.localNow
  "A password for browsers on your network": "给局域网里的浏览器设一个密码", // where.lan.formTitle
  "Browsers on other devices sign in with this name and password. The Droppen app on this Mac doesn't need it.": "其它设备上的浏览器用这个名字和密码登录。这台 Mac 上的 Droppen 应用不需要它。", // where.lan.formBody
  "Password": "密码", // where.lan.pw
  "Password again": "再输一次", // where.lan.pw2
  "At least 12 characters.": "至少 12 个字符。", // where.lan.rule
  "Open to my local network now": "现在就开放给局域网", // where.lan.open
  "Connections on the local network aren't encrypted (http). Only turn this on at home, at work or on other networks you trust.": "局域网里的连接不加密（http）。只在家里、公司等你信任的网络里打开。", // where.lan.warn
  "macOS may ask whether Droppen can accept incoming connections. Choose Allow.": "macOS 可能会问是否允许 Droppen 接受传入的连接——选「允许」。", // where.lan.firewall
  "Open on your local network": "已开放给局域网", // where.lan.onTitle
  "Address": "地址", // where.lan.addr
  "Open this in a browser on the same network. If the name doesn't work, use the number.": "在同一网络里的浏览器打开这个地址。名字打不开就用数字地址。", // where.lan.addrBody
  "The port stays {{port}} while this is on.": "开放期间端口固定为 {{port}}。", // where.lan.port
  "Copy address": "复制地址", // where.lan.copy
  // Step 03 · First session (A.5)
  "03 · First session": "03 · 第一个会话", // first.kicker
  "Pick a folder for the AI to work in": "选一个文件夹，让 AI 在里面工作", // first.title
  "The AI works in the folder you pick. The suggested first messages only ask it to look, and nothing runs until you press Send on the next screen, where the permission it runs with is shown and can be changed.": "AI 在你选的文件夹里工作。推荐的第一句只让它看看；在下一个画面按下发送之前什么都不会运行，它用什么权限运行也显示在那里，可以改。", // first.body, rewritten: the first session runs with the device's own permission
  "Used recently with Claude Code or Codex": "最近在 Claude Code 或 Codex 里用过", // first.recent
  "Make a practice folder": "新建一个练习文件夹", // first.practice
  "~/Droppen/first-session: empty, safe to experiment in.": "~/Droppen/first-session：空的，随便试。", // first.practice.body
  "Choose another folder…": "选择其他文件夹…", // first.other
  "Folders in Documents, Desktop or Downloads: macOS asks once whether Droppen may open them. Choose Allow.": "选「文稿」「桌面」「下载」里的文件夹时，macOS 会问一次是否允许 Droppen 打开——选「允许」。", // first.tcc
  "{{n}} sessions": "{{n}} 个会话", // first.sessions
  "Run with": "用哪个工具", // first.agent
  "First message": "第一条消息", // first.prompt
  "Explain this project: what it does and how to run it. Don't change any files yet.": "介绍一下这个项目：它是做什么的、怎么运行。先不要修改任何文件。", // first.p.repo1
  "List the three things most worth fixing here first. Just list them; don't change anything.": "列出这个项目里最值得先修的三个问题。只列出来，不要改。", // first.p.repo2
  "Make a one-page website that shows today's date and a short quote, then tell me how to open it.": "做一个单页网页，显示今天的日期和一句短短的名言，然后告诉我怎么打开它。", // first.p.practice
  "Write my own": "自己写", // first.p.own
  "You send it yourself on the next screen; nothing runs until you press Send.": "在下一个画面里由你自己发送——按下发送之前什么都不会运行。", // first.note
  "No AI tool is ready yet, so there is nothing to start. Get one ready in step 1, or skip this step.": "还没有可用的 AI 工具，暂时没法开始。回第 1 步准备一个，或者跳过这一步。", // first.noTool
  // Step 04 · Finishing up (A.6)
  "04 · Finishing up": "04 · 收尾", // wrap.kicker
  "A few last things": "最后几件小事", // wrap.title
  "Start Droppen at login": "登录时启动 Droppen", // wrap.login
  "So sessions, your phone and scheduled tasks can reach this Mac after a restart. Droppen lives in the menu bar; closing the window doesn't stop it.": "这样重启之后，会话、手机和定时任务也能连到这台 Mac。Droppen 待在菜单栏里，关掉窗口不会停止它。", // wrap.login.body
  "Move Droppen to Applications first.": "先把 Droppen 移到「应用程序」。", // wrap.login.move
  "Notifications": "通知", // wrap.notif
  "A macOS notification when a session finishes or asks you something.": "会话做完或在问你时，发一条 macOS 通知。", // wrap.notif.body
  "Allow notifications": "允许通知", // wrap.notif.allow
  "Turned off in System Settings → Notifications → Droppen.": "已在 系统设置 → 通知 → Droppen 里关闭。", // wrap.notif.denied
  "Open System Settings": "打开系统设置", // wrap.notif.open
  "On your phone": "在手机上", // wrap.phone
  "Open droppen.app and sign in as {{email}}.": "打开 droppen.app，用 {{email}} 登录。", // wrap.phone.linked
  "Want it on your phone too? Sign in to Droppen any time in Settings → Access → Droppen.": "也想在手机上用？随时在 设置 → 访问 → Droppen 里登录。", // wrap.phone.not
  "What's set": "设置结果", // wrap.summary
  "Droppen · {{email}}": "Droppen · {{email}}", // wrap.sum.linked
  "Local network · {{addr}}": "局域网 · {{addr}}", // wrap.sum.lan
  "Finish and open my first session": "完成，打开第一个会话", // wrap.finish
  "Finish": "完成", // wrap.finishPlain
  "Run this again from Settings → General → Setup.": "可以从 设置 → 通用 → 设置引导 再运行一次。", // wrap.rerun
  // After the guide: landing, resume bar, Settings (A.7)
  "Press Send when you're ready. Model, effort and permission are in the row below.": "准备好就按发送。模型、思考强度和权限在下面那一行。", // after.landed
  "Setup: {{n}} steps left": "设置还差 {{n}} 步", // after.resume
  "Setup: 1 step left": "设置还差 1 步", // after.resume, one step (new)
  "Don't remind me": "不再提醒", // after.resume.off
  "No AI tool is ready on this device yet.": "这台设备上还没有可用的 AI 工具。", // after.noTool
  "Get one ready": "去准备", // after.noTool.go
  "Setup": "设置引导", // after.settings.title
  "Check Claude Code and Codex, change where you use Droppen, pick a first folder again.": "检查 Claude Code 和 Codex，改在哪里用，重新选第一个文件夹。", // after.settings.body
  "Run setup again": "重新运行", // after.settings.rerun
  "Claude Code is signed in. You can go back to Droppen.": "Claude Code 已登录，可以回到 Droppen 了。", // after.signedInToast
  // Step 02 · Where to use it (A.4)
  "This Mac stays in your Droppen account. To take it out, use Settings → Access → Droppen.": "这台 Mac 仍在你的 Droppen 账户里。要把它移出，请到 设置 → 访问 → Droppen。", // where.local.stays (as built)
  "This window cannot sign this device in. See Settings → Access → Droppen.": "这个窗口不能让这台设备登录。请看 设置 → 访问 → Droppen。", // where.unavailable (as built)
  // Step 04 · Finishing up (A.6)
  "Turn it on or off any time from the Droppen icon in the menu bar: Start at Login.": "随时可以在菜单栏的 Droppen 图标里打开或关闭「登录时启动」。", // wrap.login.menuBar
  // Step 01 · AI tools, as built: the states, forms and failures the copy table leaves to the implementation
  "Close": "关闭",
  "macOS opened Apple's installer. Finish it there, then check again.": "macOS 已打开 Apple 的安装程序。在那里装完，再回来检查一次。",
  "Check again": "再检查一次",
  "Added to Droppen as “{{label}}”.": "已作为「{{label}}」加到 Droppen。",
  "These run from the Droppen app on this Mac. Copy the command and run it in your own Terminal.": "这些操作只能在这台 Mac 上的 Droppen 应用里运行。复制命令，在你自己的终端里运行。",
  "OpenAI API key": "OpenAI API key",
  "Droppen hands the key to codex login --with-api-key once, through its input. Codex keeps it; Droppen doesn't.": "Droppen 只通过输入把 key 交给 codex login --with-api-key 一次。由 Codex 保存，Droppen 不保存。",
  "Where is it?": "它在哪里？",
  "In your own Terminal, `which {{name}}` prints it.": "在你自己的终端里运行 `which {{name}}` 就能看到。",
  "Use this one": "用这个",
  "Droppen can't install it on this device. The official guide lists the other ways.": "Droppen 没法在这台设备上安装它。官方说明里有其他安装方法。",
  "Add it to Droppen": "加到 Droppen",
  "Want claude in your own Terminal too? The installer leaves that to you; this line adds ~/.local/bin to your PATH:": "想在你自己的终端里也用 claude？安装程序不会替你改；这一行会把 ~/.local/bin 加进 PATH：",
  "Signing in with the API key…": "正在用 API key 登录…",
  "Signing in didn't finish (exit code {{code}}). Try again.": "登录没有完成（退出码 {{code}}）。请重试。",
  "Droppen account": "Droppen 账号",
  "Signed in": "已登录",
  "Looking… (up to 20 s)": "正在找…（最多 20 秒）",
  "Found {{name}} at {{path}}, outside the places Droppen's sessions look. Use this copy?": "在 {{path}} 找到了 {{name}}，但它不在 Droppen 的会话查找的位置。用这一份吗？",
  "Found it, use it": "找到了，用它",
  "The free Claude plan doesn't include Claude Code. Upgrade at claude.ai, or sign in with a Console account.": "Claude 免费版不含 Claude Code。去 claude.ai 升级，或者改用 Console 账号登录。",
  "More": "更多",
  "Sign in with single sign-on (SSO)": "用单点登录（SSO）登录",
  "updating": "更新中",
  "found elsewhere": "在别处找到",
  "Updating Claude Code. It keeps going if you leave this step.": "正在更新 Claude Code，离开这一步也会继续。",
  "Updating Codex. It keeps going if you leave this step.": "正在更新 Codex，离开这一步也会继续。",
  "Codex {{v}} is too old for Droppen; it needs {{min}} or later.": "Codex {{v}} 太旧了，Droppen 需要 {{min}} 或更新的版本。",
  "Your Terminal sets OPENAI_API_KEY. Droppen doesn't take keys from your shell settings; to use that key here, add it to Droppen below.": "你的终端里设置了 OPENAI_API_KEY。Droppen 不读 shell 配置里的密钥；想让这里的会话也用它，就在下面把它加进 Droppen。",
  "Codex is signed in. You can go back to Droppen.": "Codex 已登录，可以回到 Droppen 了。",
  "The install didn't finish (exit code {{code}}).": "安装没有完成（退出码 {{code}}）。",
  "The update didn't finish (exit code {{code}}).": "更新没有完成（退出码 {{code}}）。",
  "The installer finished, but the tool still isn't on this device.": "安装程序结束了，但这台设备上还是找不到这个工具。",
  "The update finished, but the version is still too old.": "更新结束了，但版本还是太旧。",
  "Couldn't reach the download server. Check the connection and try again, or run the command in your own Terminal.": "连不上下载服务器。检查网络后重试，或者在你自己的终端里运行这条命令。",
  "Permission denied. Homebrew on this Mac may belong to another user; the official installer installs into your home folder instead.": "没有权限。这台 Mac 上的 Homebrew 可能属于另一个用户；官方安装程序会装到你的个人文件夹，不受影响。",
  "Permission denied: it couldn't write where it installs. Try the official installer, which installs into your home folder, or run the command in your own Terminal.": "没有权限：没法写入安装位置。试试官方安装程序（装到你的个人文件夹），或者在你自己的终端里运行这条命令。",
  "The disk is full. Free some space, then try again.": "磁盘满了。清出一些空间后重试。",
  "The last lines are below. Try again, or run the command in your own Terminal.": "下面是最后几行输出。重试，或者在你自己的终端里运行这条命令。",
  "The official installer needs macOS 13 or later.": "官方安装程序需要 macOS 13 或更新的版本。",
  "Homebrew isn't on this Mac.": "这台 Mac 上没有 Homebrew。",
  "npm needs Node.js, which isn't on this Mac.": "npm 需要 Node.js，这台 Mac 上没有。",
  "Not available on this device.": "这台设备上用不了。",
  "Something is already running for this tool. Wait for it, or stop it.": "这个工具已经有任务在运行。等它结束，或者先停止它。",
  "That isn't a copy Droppen can use. Pick the program itself, named claude or codex.": "这不是 Droppen 能用的程序。请选程序本身，名字是 claude 或 codex。",
  "That didn't work. Try again.": "没有成功，请重试。",
  "Claude · environment": "Claude · 环境变量",
  "Codex · API key": "Codex · API key",
  // Step 03 · First session, as built
  "Back to step 1": "回到第 1 步",
};
