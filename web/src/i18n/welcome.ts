/*
 * The first-run guide of the Droppen app (`features/setup/`, `/welcome`):
 * every string of the guide as first designed, so the
 * steps built later (01 AI tools, 03 first session) only add what they invent.
 * Kept apart from the main tables and merged under them in `index.ts` (a key
 * the main tables already translate keeps that translation); keys are the
 * English source strings, as everywhere.
 *
 * Generated once from a table of the guide's copy
 * (one row per string: screen, key, en, zh-CN, zh-TW, ja): the key is the
 * English text with `{name}` written as `{{name}}`; rows with no English are
 * left out; the trailing comment is the row's key in that table
 * (`where.title`). Changes from that table: the header reads "Get
 * started with Droppen" (「设置 Droppen」 read as Settings); the sign-in card's
 * "coming soon" line, step 04's menu-bar line and the resume bar's "1 step"
 * are new; nothing here changes a translation the app already has
 * (`test/setup-i18n.test.ts`), so "now" and "on" (a time, online) are left
 * out, "done" is 完成 as in the main tables and two ja strings follow the
 * app's (サインイン, 件のセッション). Where two rows share their English the
 * first one's words are kept: "Name" is 名称 (step 01's account name; step
 * 02's network sign-in name needs a key of its own). Edit by hand from here
 * on.
 */

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
  "done": "完成", // shell.state.done
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
  "failed": "失败", // tools.st.failed
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
  "installed": "已安装", // tools.git.ready
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
  "Signing in from the app is coming soon": "很快就能在应用里登录", // where.droppen.soon
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

export const welcomeZhTW: Record<string, string> = {
  // Frame: top bar, step rail, bottom bar
  "Get started with Droppen": "開始使用 Droppen", // shell.setup
  "Finish later": "稍後再說", // shell.later
  "What you've done is kept. Continue from the bar above the session list, or from Settings → General → Setup.": "已經做的都會保留。可以從工作階段列表上方的提示列，或 設定 → 一般 → 設定引導 繼續。", // shell.laterHint
  "Skip this step": "略過這一步", // shell.skipStep
  "Back": "上一步", // shell.back
  "Continue": "繼續", // shell.continue
  "Step {{n}} of 4": "第 {{n}} 步，共 4 步", // shell.stepOf
  "AI tools": "AI 工具", // shell.step.tools
  "Where to use it": "在哪裡用", // shell.step.where
  "First session": "第一個工作階段", // shell.step.first
  "Finishing up": "收尾", // shell.step.wrap
  "done": "完成", // shell.state.done
  "skipped": "已略過", // shell.state.skipped
  "Every step can be skipped and changed later in Settings.": "每一步都能略過，之後都能在「設定」裡修改。", // shell.railFoot
  "Offline. Installing, signing in and Droppen need the internet; everything else works.": "目前沒有連網：安裝、登入和 Droppen 需要網路，其他都能用。", // shell.offline
  "Language": "語言", // shell.language
  // Welcome (A.2)
  "Droppen for Mac": "Droppen · Mac 版", // welcome.kicker
  "Droppen is running on this Mac.": "Droppen 已經在這台 Mac 上執行了。", // welcome.title
  "About three minutes to put it to work: get Claude Code or Codex ready, decide where you'll use it, then start your first session. Skip anything you like; all of it can be changed later in Settings.": "花 3 分鐘左右把它用起來：準備好 Claude Code 或 Codex，決定在哪裡用，然後開始第一個工作階段。哪一步都可以略過，之後都能在「設定」裡修改。", // welcome.body
  "Find Claude Code and Codex; install and sign in where needed.": "找到 Claude Code 和 Codex，需要時安裝、登入。", // welcome.sum.tools
  "Only on this Mac, or on your phone and other devices too.": "只在這台 Mac 上用，還是手機和其他裝置也要用。", // welcome.sum.where
  "Pick a folder and send a first message.": "選一個資料夾，送出第一則訊息。", // welcome.sum.first
  "Start at login, notifications, your phone.": "登入時啟動、通知、手機。", // welcome.sum.wrap
  "Already looking for Claude Code and Codex on this device…": "已經在這台裝置上尋找 Claude Code 和 Codex…", // welcome.checking
  "Found: {{list}}": "已找到：{{list}}", // welcome.found
  "Get started": "開始", // welcome.start
  "Skip setup and open Droppen": "略過設定，直接打開", // welcome.skipAll
  "Droppen is running from a temporary copy. Move it to Applications so it can start at login.": "Droppen 正在從一份暫時副本執行。移到「應用程式」檔案夾後，登入時啟動才能使用。", // welcome.moved
  "Move to Applications and reopen": "移到「應用程式」並重新打開", // welcome.move
  // Step 01 · AI tools (A.3)
  "01 · AI tools": "01 · AI 工具", // tools.kicker
  "Get Claude Code or Codex ready": "準備好 Claude Code 或 Codex", // tools.title
  "Droppen runs these AI tools on this device. One is enough; you can add the other any time. Droppen only asks each tool for its own status. It never reads your keychain or saved passwords.": "Droppen 在這台裝置上執行這些 AI 工具。有一個就夠了，另一個隨時可以再加。Droppen 只詢問工具本身的狀態，不讀鑰匙圈，也不讀儲存的密碼。", // tools.body
  "by Anthropic": "Anthropic 出品", // tools.by.claude
  "by OpenAI": "OpenAI 出品", // tools.by.codex
  "Needs a Claude Pro, Max, Team or Enterprise plan, or an Anthropic Console account.": "需要 Claude Pro、Max、Team 或 Enterprise 訂閱，或 Anthropic Console 帳號。", // tools.needs.claude
  "Works with a ChatGPT plan (Plus, Pro, Business, Edu, Enterprise; Free and Go with lower limits) or an OpenAI API key.": "使用 ChatGPT 訂閱（Plus、Pro、Business、Edu、Enterprise；Free 與 Go 額度較少）或 OpenAI API key。", // tools.needs.codex
  "checking": "檢查中", // tools.st.checking
  "not installed": "未安裝", // tools.st.notInstalled
  "installing": "安裝中", // tools.st.installing
  "failed": "失敗", // tools.st.failed
  "not signed in": "未登入", // tools.st.signedOut
  "signing in": "登入中", // tools.st.signingIn
  "ready": "已就緒", // tools.st.ready
  "update needed": "需要更新", // tools.st.tooOld
  "not found": "沒找到", // tools.st.notFound
  "plan doesn't include it": "訂閱不含", // tools.st.plan
  "Claude Code isn't on this device yet.": "這台裝置上還沒有 Claude Code。", // tools.notInstalled.claude
  "Codex isn't on this device yet.": "這台裝置上還沒有 Codex。", // tools.notInstalled.codex
  "Install…": "安裝…", // tools.install
  "How to install": "安裝方式", // tools.method
  "Official installer": "官方安裝程式", // tools.method.native
  "Homebrew": "Homebrew", // tools.method.brew
  "npm": "npm", // tools.method.npm
  "recommended": "推薦", // tools.recommended
  "Droppen will run this command": "Droppen 將執行這個指令", // tools.confirm
  "From Anthropic (claude.ai): the command in Claude Code's own install guide.": "來自 Anthropic（claude.ai），就是 Claude Code 官方安裝說明裡的指令。", // tools.fact.claude.1
  "Installs into your home folder (~/.local/bin/claude). No administrator password, no Node.js.": "裝到你的個人資料夾（~/.local/bin/claude），不需要管理員密碼，也不需要 Node.js。", // tools.fact.claude.2
  "Updates itself in the background afterwards. Takes about a minute.": "之後會在背景自動更新。大約需要 1 分鐘。", // tools.fact.claude.3
  "From OpenAI (chatgpt.com): the command in Codex's own README.": "來自 OpenAI（chatgpt.com），就是 Codex 官方說明裡的指令。", // tools.fact.codex.1
  "Installs into ~/.local/bin/codex and checks every download's SHA-256. No administrator password, no Node.js.": "裝到 ~/.local/bin/codex，並核對每個下載檔案的 SHA-256。不需要管理員密碼，也不需要 Node.js。", // tools.fact.codex.2
  "Adds ~/.local/bin to PATH in ~/.zprofile so your own Terminal finds codex too. That is what the official installer does anyway.": "會在 ~/.zprofile 裡把 ~/.local/bin 加進 PATH，讓你自己的終端機也能找到 codex——官方安裝程式本來就這麼做。", // tools.fact.codex.3
  "Uses your Homebrew. Homebrew copies don't update themselves; Droppen offers the update when one is out.": "用你已有的 Homebrew 安裝。Homebrew 裝的不會自己更新，有新版本時 Droppen 會提示。", // tools.fact.brew
  "Uses your Node.js ({{node}}). Only if you already manage tools with npm.": "用你已有的 Node.js（{{node}}）。只建議已經用 npm 管理工具的人選。", // tools.fact.npm
  "Run it here": "在這裡執行", // tools.runHere
  "Copy command": "複製指令", // tools.copyCommand
  "Copied": "已複製", // tools.copied
  "Cancel": "取消", // tools.cancel
  "Official guide": "官方說明", // tools.guide
  "Installing Claude Code. It keeps going if you leave this step.": "正在安裝 Claude Code，離開這一步也會繼續。", // tools.installing.claude
  "Installing Codex. It keeps going if you leave this step.": "正在安裝 Codex，離開這一步也會繼續。", // tools.installing.codex
  "Stop": "停止", // tools.stop
  "Terminal · on this device": "終端機 · 在這台裝置上", // tools.terminal
  "F6 moves between the terminal and the buttons": "按 F6 在終端機和按鈕之間切換", // tools.f6
  "The install didn't finish (exit code 1): couldn't reach claude.ai. Check the connection and try again, or run the command in your own Terminal.": "安裝沒有完成（結束代碼 1）：連不上 claude.ai。檢查網路後重試，或在你自己的終端機裡執行這個指令。", // tools.failed.claude
  "Try again": "重試", // tools.retry
  "Installed. Sign in once and Droppen can start Claude Code sessions. Your browser opens; sign in with your Claude account.": "已經裝好。登入一次，Droppen 就能啟動 Claude Code 工作階段。會打開瀏覽器，用你的 Claude 帳號登入。", // tools.signedOut.claude
  "Installed. Sign in once and Droppen can start Codex sessions. Your browser opens; sign in with ChatGPT.": "已經裝好。登入一次，Droppen 就能啟動 Codex 工作階段。會打開瀏覽器，用 ChatGPT 登入。", // tools.signedOut.codex
  "Sign in": "登入", // tools.signIn
  "Use an Anthropic Console account instead": "改用 Anthropic Console 帳號", // tools.signIn.console
  "Use an OpenAI API key instead": "改用 OpenAI API key", // tools.signIn.apiKey
  "Finish signing in in your browser, then come back. Droppen notices by itself.": "在瀏覽器裡完成登入，然後回到這裡——Droppen 會自己發現。", // tools.signingIn
  "Waiting for sign-in…": "等待登入…", // tools.waiting
  "Browser didn't open?": "瀏覽器沒打開？", // tools.noBrowser
  "Press c in the terminal to copy the sign-in link, open it in any browser, then paste the code it shows back into the terminal.": "在終端機裡按 c 複製登入連結，用任意瀏覽器打開，再把頁面上顯示的代碼貼回終端機。", // tools.noBrowser.claude
  "Use a one-time code instead: Droppen runs codex login --device-auth and shows a code to enter at chatgpt.com.": "改用一次性代碼：Droppen 會執行 codex login --device-auth，並顯示要在 chatgpt.com 上輸入的代碼。", // tools.noBrowser.codex
  "Sign in with a code": "用代碼登入", // tools.useCode
  "5 hours": "5 小時", // tools.q.5h
  "this week": "本週", // tools.q.week
  "{{n}}% used": "已用 {{n}}%", // tools.q.used
  "resets {{t}}": "{{t}} 重置", // tools.q.resets
  "Quota shows up after the first session.": "第一個工作階段之後會顯示額度。", // tools.q.later
  "Use a different account": "換個帳號", // tools.switch
  "Claude Code {{v}} is too old for Droppen; it needs {{min}} or later.": "Claude Code {{v}} 太舊了，Droppen 需要 {{min}} 或更新的版本。", // tools.tooOld
  "Update": "更新", // tools.update
  "Droppen couldn't read your Terminal's settings in time (over 5 s), so a copy installed with nvm, volta or similar may be hiding. Look again, or show Droppen where it is.": "沒能及時讀到你終端機的設定（超過 5 秒），用 nvm、volta 等安裝的可能沒被找到。再找一次，或告訴 Droppen 它在哪裡。", // tools.notFound
  "Look again (wait longer)": "再找一次（多等一會兒）", // tools.lookAgain
  "Show where it is…": "指定位置…", // tools.choosePath
  "Install a fresh copy": "仍然安裝一份", // tools.installAnyway
  "Signed in as {{email}}, but the free Claude plan doesn't include Claude Code. Upgrade at claude.ai, or sign in with a Console account.": "已經以 {{email}} 登入，但 Claude 免費版不含 Claude Code。到 claude.ai 升級，或改用 Console 帳號登入。", // tools.plan.claude
  "Plans at claude.ai": "到 claude.ai 看訂閱", // tools.upgrade
  "Your Terminal sets ANTHROPIC_API_KEY. Droppen doesn't take keys from your shell settings; to use that key here, add it to Droppen below.": "你的終端機設定了 ANTHROPIC_API_KEY。Droppen 不讀 shell 設定裡的金鑰；想讓這裡的工作階段也用它，就在下面把它加進 Droppen。", // tools.envKey
  "Ready through the Droppen account “{{label}}”": "透過 Droppen 帳號「{{label}}」就緒", // tools.viaAccount
  "Developer tools (git)": "開發者工具（git）", // tools.git
  "Claude Code and Codex use git in code projects. Without it, macOS interrupts the first session to install Apple's Command Line Tools; you can get that done now.": "Claude Code 和 Codex 在程式專案裡會用 git。沒裝的話，macOS 會在第一個工作階段裡跳出視窗要你安裝 Apple 的命令列工具；可以現在先裝好。", // tools.git.body
  "Install (Apple)": "安裝（Apple 官方）", // tools.git.install
  "installed": "已安裝", // tools.git.ready
  "optional": "選用", // tools.optional
  "Add accounts to Droppen (advanced)": "把帳號加到 Droppen（進階）", // tools.adv
  "For switching accounts automatically when one runs out of quota, or paying through an API key or a company gateway. Most people don't need this. Keys stay on this device and are never shown again.": "適合額度用完時自動換帳號，或用 API key、公司閘道計費。大多數人用不到。金鑰只存在這台裝置上，存好後不再顯示。", // tools.adv.body
  "Claude · long-lived sign-in (setup-token)": "Claude · 長期登入權杖（setup-token）", // tools.adv.token
  "Sign in once in the browser; Droppen keeps a one-year token. Needed for automatic account switching.": "在瀏覽器裡登入一次，Droppen 保存一個一年有效的權杖。自動換帳號需要它。", // tools.adv.token.body
  "Environment variables (API key, gateway)": "環境變數（API key、閘道）", // tools.adv.env
  "One KEY=VALUE per line, given only to the sessions this account starts.": "每行一個 KEY=VALUE，只交給用這個帳號啟動的工作階段。", // tools.adv.env.body
  "Codex · OpenAI API key": "Codex · OpenAI API key", // tools.adv.codex
  "Name": "名稱", // tools.adv.name
  "Add account": "新增帳號", // tools.adv.add
  "Start": "開始", // tools.adv.start
  "Rotation and the full list are in Settings → Accounts.": "換帳號設定和完整列表在 設定 → 帳號。", // tools.adv.more
  "Continue without AI tools": "先不裝，繼續", // tools.none
  "Without Claude Code or Codex, Droppen can show files and open terminals but can't run AI. Come back any time from Settings → General → Setup.": "沒有 Claude Code 或 Codex，Droppen 能看檔案、開終端機，但不能執行 AI。隨時可以從 設定 → 一般 → 設定引導 回來。", // tools.none.body
  // Step 02 · Where to use it (A.4)
  "02 · Where to use it": "02 · 在哪裡用", // where.kicker
  "Where will you use Droppen?": "你會在哪裡用 Droppen？", // where.title
  "Droppen runs on this Mac either way. This only decides what else can reach it, and you can change it any time in Settings.": "不管選哪個，Droppen 都在這台 Mac 上執行；這裡只決定還有什麼能連到它，隨時可以在「設定」裡修改。", // where.body
  "On my phone and other devices too": "在手機和其他裝置上也能用", // where.droppen.title
  "Sign in to Droppen": "登入 Droppen", // where.droppen.action
  "Open droppen.app on your phone or any other device and carry on with this Mac's sessions. Content is end-to-end encrypted; droppen.app can't read it. Your browser opens to sign in or create an account.": "在手機或其他裝置上打開 droppen.app，接著用這台 Mac 上的工作階段。內容端對端加密，droppen.app 看不到。會打開瀏覽器登入或註冊。", // where.droppen.body
  "Recommended": "推薦", // where.recommended
  "Only on this Mac": "只在這台 Mac 上用", // where.local.title
  "Skip": "略過", // where.local.action
  "Everything works without an account. Nothing leaves this Mac except what Claude Code and Codex send to their own services.": "不需要帳號，一切照常。除了 Claude Code 和 Codex 傳給它們自己服務的內容，什麼都不離開這台 Mac。", // where.local.body
  "Also": "另外", // where.also
  "Let browsers on my local network in too": "讓區域網路裡的瀏覽器也能存取", // where.lan.title
  "Set a password for this device (advanced)": "設定這台裝置的密碼（進階）", // where.lan.action
  "An iPad or another device on the same Wi‑Fi opens this Mac's address in a browser and signs in with the password. Doesn't go through droppen.app, and works alongside either choice above.": "同一個 Wi‑Fi 下的 iPad 或其他裝置，用瀏覽器打開這台 Mac 的位址，輸入密碼進入。不經過 droppen.app，可以和上面任一選項同時使用。", // where.lan.body
  "The Droppen app on this Mac never asks for a password. For a browser on this Mac, choose Open in Browser in the app's menu-bar menu: it opens signed in. Once this device has a password, a browser asks for it.": "這台 Mac 上的 Droppen 應用程式永遠不用密碼。這台 Mac 上的瀏覽器，在應用程式的選單列選單中選擇「在瀏覽器中打開」，打開時已經登入。替這台裝置設定密碼以後，瀏覽器會要這個密碼。", // where.browserNote
  "Sign in in the browser": "在瀏覽器中登入", // where.go.droppen
  "Set the password": "設定密碼", // where.go.lan
  "Change it later in Settings → Access.": "之後在 設定 → 存取 裡修改。", // where.changeLater
  "droppen.app needs the internet. Choose “Only on this Mac” for now; sign in later from Settings → Access → Droppen.": "登入 droppen.app 需要網路。先選「只在這台 Mac 上用」，之後在 設定 → 存取 → Droppen 裡登入。", // where.offline
  "Continue in your browser": "在瀏覽器裡繼續", // where.wait.title
  "Sign in to droppen.app, or create an account.": "登入 droppen.app，或註冊一個帳戶。", // where.wait.1
  "Check that the fingerprint there matches this one, then choose Allow.": "核對那邊顯示的指紋和這裡一致，然後按「同意」。", // where.wait.2
  "Come back here. This Mac joins your account by itself.": "回到這裡，這台 Mac 會自己加入你的帳戶。", // where.wait.3
  "Fingerprint": "指紋", // where.fingerprint
  "Open the browser again": "重新打開瀏覽器", // where.reopen
  "Copy link": "複製連結", // where.copyLink
  "The link works for 10 minutes ({{left}} left).": "連結 10 分鐘內有效（還剩 {{left}}）。", // where.expires
  "Waiting for droppen.app…": "等待 droppen.app…", // where.waiting
  "Keep going; I'll finish in the browser": "先繼續，瀏覽器裡登入好了會提示我", // where.keepGoing
  "This Mac is in your Droppen account": "這台 Mac 已加入你的 Droppen 帳戶", // where.linked.title
  "Open droppen.app on your phone and sign in as {{email}}. “{{device}}” is there.": "在手機上打開 droppen.app，用 {{email}} 登入，就能看到「{{device}}」。", // where.linked.body
  "Device name": "裝置名稱", // where.deviceName
  "Scan with your phone": "用手機掃描", // where.scan
  "Can't reach droppen.app": "連不上 droppen.app", // where.err.title
  "Use Droppen on this Mac for now. When you're online, sign in from Settings → Access → Droppen; nothing done here is lost.": "先只在這台 Mac 上用。連網後在 設定 → 存取 → Droppen 裡登入，這裡做的都不會遺失。", // where.err.body
  "Your plan's devices that run AI are all in use. Remove one at droppen.app or change plan; until then this Mac works on its own.": "你的方案中可執行 AI 的裝置已經用滿。到 droppen.app 移除一台或更換方案；在那之前這台 Mac 照常單獨使用。", // where.err.limit
  "Only on this Mac for now": "先只在這台 Mac 上用", // where.localNow
  "A password for browsers on your network": "給區域網路裡的瀏覽器設一個密碼", // where.lan.formTitle
  "Browsers on other devices sign in with this name and password. The Droppen app on this Mac doesn't need it.": "其他裝置上的瀏覽器用這個名稱和密碼登入。這台 Mac 上的 Droppen 應用程式不需要它。", // where.lan.formBody
  "Password": "密碼", // where.lan.pw
  "Password again": "再輸入一次", // where.lan.pw2
  "At least 12 characters.": "至少 12 個字元。", // where.lan.rule
  "Open to my local network now": "現在就開放給區域網路", // where.lan.open
  "Connections on the local network aren't encrypted (http). Only turn this on at home, at work or on other networks you trust.": "區域網路裡的連線不加密（http）。只在家裡、公司等你信任的網路裡打開。", // where.lan.warn
  "macOS may ask whether Droppen can accept incoming connections. Choose Allow.": "macOS 可能會詢問是否允許 Droppen 接受傳入的連線——選「允許」。", // where.lan.firewall
  "Open on your local network": "已開放給區域網路", // where.lan.onTitle
  "Address": "位址", // where.lan.addr
  "Open this in a browser on the same network. If the name doesn't work, use the number.": "在同一網路裡的瀏覽器打開這個位址。名稱打不開就用數字位址。", // where.lan.addrBody
  "The port stays {{port}} while this is on.": "開放期間連接埠固定為 {{port}}。", // where.lan.port
  "Copy address": "複製位址", // where.lan.copy
  // Step 03 · First session (A.5)
  "03 · First session": "03 · 第一個工作階段", // first.kicker
  "Pick a folder for the AI to work in": "選一個資料夾，讓 AI 在裡面工作", // first.title
  "The AI works in the folder you pick. The suggested first messages only ask it to look, and nothing runs until you press Send on the next screen, where the permission it runs with is shown and can be changed.": "AI 在你選的資料夾裡工作。推薦的第一句只讓它看看；在下一個畫面按下送出之前什麼都不會執行，它用什麼權限執行也顯示在那裡，可以修改。", // first.body, rewritten: the first session runs with the device's own permission
  "Used recently with Claude Code or Codex": "最近在 Claude Code 或 Codex 裡用過", // first.recent
  "Make a practice folder": "新增一個練習資料夾", // first.practice
  "~/Droppen/first-session: empty, safe to experiment in.": "~/Droppen/first-session：空的，隨便試。", // first.practice.body
  "Choose another folder…": "選擇其他資料夾…", // first.other
  "Folders in Documents, Desktop or Downloads: macOS asks once whether Droppen may open them. Choose Allow.": "選「文件」「桌面」「下載項目」裡的資料夾時，macOS 會詢問一次是否允許 Droppen 打開——選「允許」。", // first.tcc
  "{{n}} sessions": "{{n}} 個工作階段", // first.sessions
  "Run with": "用哪個工具", // first.agent
  "First message": "第一則訊息", // first.prompt
  "Explain this project: what it does and how to run it. Don't change any files yet.": "介紹一下這個專案：它是做什麼的、怎麼執行。先不要修改任何檔案。", // first.p.repo1
  "List the three things most worth fixing here first. Just list them; don't change anything.": "列出這個專案裡最值得先修的三個問題。只列出來，不要改。", // first.p.repo2
  "Make a one-page website that shows today's date and a short quote, then tell me how to open it.": "做一個單頁網頁，顯示今天的日期和一句短短的名言，然後告訴我怎麼打開它。", // first.p.practice
  "Write my own": "自己寫", // first.p.own
  "You send it yourself on the next screen; nothing runs until you press Send.": "在下一個畫面裡由你自己送出——按下送出之前什麼都不會執行。", // first.note
  "No AI tool is ready yet, so there is nothing to start. Get one ready in step 1, or skip this step.": "還沒有可用的 AI 工具，暫時沒辦法開始。回第 1 步準備一個，或略過這一步。", // first.noTool
  // Step 04 · Finishing up (A.6)
  "04 · Finishing up": "04 · 收尾", // wrap.kicker
  "A few last things": "最後幾件小事", // wrap.title
  "Start Droppen at login": "登入時啟動 Droppen", // wrap.login
  "So sessions, your phone and scheduled tasks can reach this Mac after a restart. Droppen lives in the menu bar; closing the window doesn't stop it.": "這樣重新開機之後，工作階段、手機和排程任務也能連到這台 Mac。Droppen 待在選單列裡，關掉視窗不會停止它。", // wrap.login.body
  "Move Droppen to Applications first.": "先把 Droppen 移到「應用程式」。", // wrap.login.move
  "Notifications": "通知", // wrap.notif
  "A macOS notification when a session finishes or asks you something.": "工作階段完成或在問你時，發一則 macOS 通知。", // wrap.notif.body
  "Allow notifications": "允許通知", // wrap.notif.allow
  "Turned off in System Settings → Notifications → Droppen.": "已在 系統設定 → 通知 → Droppen 裡關閉。", // wrap.notif.denied
  "Open System Settings": "打開系統設定", // wrap.notif.open
  "On your phone": "在手機上", // wrap.phone
  "Open droppen.app and sign in as {{email}}.": "打開 droppen.app，用 {{email}} 登入。", // wrap.phone.linked
  "Want it on your phone too? Sign in to Droppen any time in Settings → Access → Droppen.": "也想在手機上用？隨時在 設定 → 存取 → Droppen 裡登入。", // wrap.phone.not
  "What's set": "設定結果", // wrap.summary
  "Droppen · {{email}}": "Droppen · {{email}}", // wrap.sum.linked
  "Local network · {{addr}}": "區域網路 · {{addr}}", // wrap.sum.lan
  "Finish and open my first session": "完成，打開第一個工作階段", // wrap.finish
  "Finish": "完成", // wrap.finishPlain
  "Run this again from Settings → General → Setup.": "可以從 設定 → 一般 → 設定引導 再執行一次。", // wrap.rerun
  // After the guide: landing, resume bar, Settings (A.7)
  "Press Send when you're ready. Model, effort and permission are in the row below.": "準備好就按送出。模型、思考強度和權限在下面那一行。", // after.landed
  "Setup: {{n}} steps left": "設定還差 {{n}} 步", // after.resume
  "Setup: 1 step left": "設定還差 1 步", // after.resume, one step (new)
  "Don't remind me": "不再提醒", // after.resume.off
  "No AI tool is ready on this device yet.": "這台裝置上還沒有可用的 AI 工具。", // after.noTool
  "Get one ready": "去準備", // after.noTool.go
  "Setup": "設定引導", // after.settings.title
  "Check Claude Code and Codex, change where you use Droppen, pick a first folder again.": "檢查 Claude Code 和 Codex，修改在哪裡用，重新選第一個資料夾。", // after.settings.body
  "Run setup again": "重新執行", // after.settings.rerun
  "Claude Code is signed in. You can go back to Droppen.": "Claude Code 已登入，可以回到 Droppen 了。", // after.signedInToast
  // Step 02 · Where to use it (A.4)
  "Signing in from the app is coming soon": "很快就能在應用程式裡登入", // where.droppen.soon
  "This Mac stays in your Droppen account. To take it out, use Settings → Access → Droppen.": "這台 Mac 仍在你的 Droppen 帳戶裡。要把它移出，請到 設定 → 存取 → Droppen。", // where.local.stays (as built)
  "This window cannot sign this device in. See Settings → Access → Droppen.": "這個視窗不能讓這台裝置登入。請看 設定 → 存取 → Droppen。", // where.unavailable (as built)
  // Step 04 · Finishing up (A.6)
  "Turn it on or off any time from the Droppen icon in the menu bar: Start at Login.": "隨時可以在選單列的 Droppen 圖示裡開啟或關閉「登入時啟動」。", // wrap.login.menuBar
  // Step 01 · AI tools, as built: the states, forms and failures the copy table leaves to the implementation
  "Close": "關閉",
  "macOS opened Apple's installer. Finish it there, then check again.": "macOS 已打開 Apple 的安裝程式。在那裡裝完，再回來檢查一次。",
  "Check again": "再檢查一次",
  "Added to Droppen as “{{label}}”.": "已作為「{{label}}」加到 Droppen。",
  "These run from the Droppen app on this Mac. Copy the command and run it in your own Terminal.": "這些操作只能在這台 Mac 上的 Droppen 應用程式裡執行。複製指令，在你自己的終端機裡執行。",
  "OpenAI API key": "OpenAI API key",
  "Droppen hands the key to codex login --with-api-key once, through its input. Codex keeps it; Droppen doesn't.": "Droppen 只透過輸入把 key 交給 codex login --with-api-key 一次。由 Codex 保存，Droppen 不保存。",
  "Where is it?": "它在哪裡？",
  "In your own Terminal, `which {{name}}` prints it.": "在你自己的終端機裡執行 `which {{name}}` 就能看到。",
  "Use this one": "用這個",
  "Droppen can't install it on this device. The official guide lists the other ways.": "Droppen 沒辦法在這台裝置上安裝它。官方說明裡有其他安裝方法。",
  "Add it to Droppen": "加到 Droppen",
  "Want claude in your own Terminal too? The installer leaves that to you; this line adds ~/.local/bin to your PATH:": "想在你自己的終端機裡也用 claude？安裝程式不會替你改；這一行會把 ~/.local/bin 加進 PATH：",
  "Signing in with the API key…": "正在用 API key 登入…",
  "Signing in didn't finish (exit code {{code}}). Try again.": "登入沒有完成（結束代碼 {{code}}）。請重試。",
  "Droppen account": "Droppen 帳號",
  "Signed in": "已登入",
  "Looking… (up to 20 s)": "正在找…（最多 20 秒）",
  "Found {{name}} at {{path}}, outside the places Droppen's sessions look. Use this copy?": "在 {{path}} 找到了 {{name}}，但它不在 Droppen 的工作階段尋找的位置。用這一份嗎？",
  "Found it, use it": "找到了，用它",
  "The free Claude plan doesn't include Claude Code. Upgrade at claude.ai, or sign in with a Console account.": "Claude 免費版不含 Claude Code。到 claude.ai 升級，或改用 Console 帳號登入。",
  "More": "更多",
  "Sign in with single sign-on (SSO)": "用單一登入（SSO）登入",
  "updating": "更新中",
  "found elsewhere": "在別處找到",
  "Updating Claude Code. It keeps going if you leave this step.": "正在更新 Claude Code，離開這一步也會繼續。",
  "Updating Codex. It keeps going if you leave this step.": "正在更新 Codex，離開這一步也會繼續。",
  "Codex {{v}} is too old for Droppen; it needs {{min}} or later.": "Codex {{v}} 太舊了，Droppen 需要 {{min}} 或更新的版本。",
  "Your Terminal sets OPENAI_API_KEY. Droppen doesn't take keys from your shell settings; to use that key here, add it to Droppen below.": "你的終端機設定了 OPENAI_API_KEY。Droppen 不讀 shell 設定裡的金鑰；想讓這裡的工作階段也用它，就在下面把它加進 Droppen。",
  "Codex is signed in. You can go back to Droppen.": "Codex 已登入，可以回到 Droppen 了。",
  "The install didn't finish (exit code {{code}}).": "安裝沒有完成（結束代碼 {{code}}）。",
  "The update didn't finish (exit code {{code}}).": "更新沒有完成（結束代碼 {{code}}）。",
  "The installer finished, but the tool still isn't on this device.": "安裝程式結束了，但這台裝置上還是找不到這個工具。",
  "The update finished, but the version is still too old.": "更新結束了，但版本還是太舊。",
  "Couldn't reach the download server. Check the connection and try again, or run the command in your own Terminal.": "連不上下載伺服器。檢查網路後重試，或在你自己的終端機裡執行這個指令。",
  "Permission denied. Homebrew on this Mac may belong to another user; the official installer installs into your home folder instead.": "沒有權限。這台 Mac 上的 Homebrew 可能屬於另一個使用者；官方安裝程式會裝到你的個人資料夾，不受影響。",
  "Permission denied: it couldn't write where it installs. Try the official installer, which installs into your home folder, or run the command in your own Terminal.": "沒有權限：沒辦法寫入安裝位置。試試官方安裝程式（裝到你的個人資料夾），或在你自己的終端機裡執行這個指令。",
  "The disk is full. Free some space, then try again.": "磁碟已滿。清出一些空間後重試。",
  "The last lines are below. Try again, or run the command in your own Terminal.": "下面是最後幾行輸出。重試，或在你自己的終端機裡執行這個指令。",
  "The official installer needs macOS 13 or later.": "官方安裝程式需要 macOS 13 或更新的版本。",
  "Homebrew isn't on this Mac.": "這台 Mac 上沒有 Homebrew。",
  "npm needs Node.js, which isn't on this Mac.": "npm 需要 Node.js，這台 Mac 上沒有。",
  "Not available on this device.": "這台裝置上用不了。",
  "Something is already running for this tool. Wait for it, or stop it.": "這個工具已經有任務在執行。等它結束，或先停止它。",
  "That isn't a copy Droppen can use. Pick the program itself, named claude or codex.": "這不是 Droppen 能用的程式。請選程式本身，名稱是 claude 或 codex。",
  "That didn't work. Try again.": "沒有成功，請重試。",
  "Claude · environment": "Claude · 環境變數",
  "Codex · API key": "Codex · API key",
  // Step 03 · First session, as built
  "Back to step 1": "回到第 1 步",
};

export const welcomeJa: Record<string, string> = {
  // Frame: top bar, step rail, bottom bar
  "Get started with Droppen": "Droppen を使い始める", // shell.setup
  "Finish later": "あとで続ける", // shell.later
  "What you've done is kept. Continue from the bar above the session list, or from Settings → General → Setup.": "ここまでの内容は保存されます。セッション一覧の上のバー、または 設定 → 一般 → セットアップ から続けられます。", // shell.laterHint
  "Skip this step": "このステップをスキップ", // shell.skipStep
  "Back": "戻る", // shell.back
  "Continue": "続ける", // shell.continue
  "Step {{n}} of 4": "ステップ {{n}}/4", // shell.stepOf
  "AI tools": "AI ツール", // shell.step.tools
  "Where to use it": "使う場所", // shell.step.where
  "First session": "最初のセッション", // shell.step.first
  "Finishing up": "仕上げ", // shell.step.wrap
  "done": "完了", // shell.state.done
  "skipped": "スキップ", // shell.state.skipped
  "Every step can be skipped and changed later in Settings.": "どのステップもスキップでき、あとから「設定」で変更できます。", // shell.railFoot
  "Offline. Installing, signing in and Droppen need the internet; everything else works.": "オフラインです。インストール・ログイン・Droppen にはインターネットが必要ですが、それ以外は使えます。", // shell.offline
  "Language": "言語", // shell.language
  // Welcome (A.2)
  "Droppen for Mac": "Droppen for Mac", // welcome.kicker
  "Droppen is running on this Mac.": "Droppen はこの Mac で動いています。", // welcome.title
  "About three minutes to put it to work: get Claude Code or Codex ready, decide where you'll use it, then start your first session. Skip anything you like; all of it can be changed later in Settings.": "3 分ほどで使い始められます。Claude Code か Codex を準備し、どこで使うかを決めて、最初のセッションを始めましょう。どのステップもスキップでき、あとから「設定」で変更できます。", // welcome.body
  "Find Claude Code and Codex; install and sign in where needed.": "Claude Code と Codex を探し、必要ならインストールしてログインします。", // welcome.sum.tools
  "Only on this Mac, or on your phone and other devices too.": "この Mac だけで使うか、スマートフォンやほかのデバイスでも使うか。", // welcome.sum.where
  "Pick a folder and send a first message.": "フォルダを選んで、最初のメッセージを送ります。", // welcome.sum.first
  "Start at login, notifications, your phone.": "ログイン時の起動、通知、スマートフォン。", // welcome.sum.wrap
  "Already looking for Claude Code and Codex on this device…": "このデバイスで Claude Code と Codex を探しています…", // welcome.checking
  "Found: {{list}}": "見つかりました：{{list}}", // welcome.found
  "Get started": "始める", // welcome.start
  "Skip setup and open Droppen": "セットアップをスキップして開く", // welcome.skipAll
  "Droppen is running from a temporary copy. Move it to Applications so it can start at login.": "Droppen は一時的なコピーから実行されています。ログイン時に起動するには「アプリケーション」フォルダに移動してください。", // welcome.moved
  "Move to Applications and reopen": "「アプリケーション」に移動して開き直す", // welcome.move
  // Step 01 · AI tools (A.3)
  "01 · AI tools": "01 · AI ツール", // tools.kicker
  "Get Claude Code or Codex ready": "Claude Code か Codex を準備する", // tools.title
  "Droppen runs these AI tools on this device. One is enough; you can add the other any time. Droppen only asks each tool for its own status. It never reads your keychain or saved passwords.": "Droppen はこのデバイスでこれらの AI ツールを動かします。どちらか一つあれば十分で、もう一つはいつでも追加できます。Droppen は各ツール自身に状態を尋ねるだけで、キーチェーンや保存されたパスワードは読みません。", // tools.body
  "by Anthropic": "Anthropic 製", // tools.by.claude
  "by OpenAI": "OpenAI 製", // tools.by.codex
  "Needs a Claude Pro, Max, Team or Enterprise plan, or an Anthropic Console account.": "Claude の Pro・Max・Team・Enterprise プラン、または Anthropic Console アカウントが必要です。", // tools.needs.claude
  "Works with a ChatGPT plan (Plus, Pro, Business, Edu, Enterprise; Free and Go with lower limits) or an OpenAI API key.": "ChatGPT のプラン（Plus・Pro・Business・Edu・Enterprise。Free と Go は上限が低め）か OpenAI API キーで使えます。", // tools.needs.codex
  "checking": "確認中", // tools.st.checking
  "not installed": "未インストール", // tools.st.notInstalled
  "installing": "インストール中", // tools.st.installing
  "failed": "失敗", // tools.st.failed
  "not signed in": "未ログイン", // tools.st.signedOut
  "signing in": "ログイン中", // tools.st.signingIn
  "ready": "準備完了", // tools.st.ready
  "update needed": "要アップデート", // tools.st.tooOld
  "not found": "見つかりません", // tools.st.notFound
  "plan doesn't include it": "プラン対象外", // tools.st.plan
  "Claude Code isn't on this device yet.": "このデバイスにはまだ Claude Code がありません。", // tools.notInstalled.claude
  "Codex isn't on this device yet.": "このデバイスにはまだ Codex がありません。", // tools.notInstalled.codex
  "Install…": "インストール…", // tools.install
  "How to install": "インストール方法", // tools.method
  "Official installer": "公式インストーラー", // tools.method.native
  "Homebrew": "Homebrew", // tools.method.brew
  "npm": "npm", // tools.method.npm
  "recommended": "おすすめ", // tools.recommended
  "Droppen will run this command": "Droppen はこのコマンドを実行します", // tools.confirm
  "From Anthropic (claude.ai): the command in Claude Code's own install guide.": "Anthropic（claude.ai）の公式インストール手順にあるコマンドです。", // tools.fact.claude.1
  "Installs into your home folder (~/.local/bin/claude). No administrator password, no Node.js.": "ホームフォルダ（~/.local/bin/claude）にインストールします。管理者パスワードも Node.js も不要です。", // tools.fact.claude.2
  "Updates itself in the background afterwards. Takes about a minute.": "以降はバックグラウンドで自動更新されます。1 分ほどかかります。", // tools.fact.claude.3
  "From OpenAI (chatgpt.com): the command in Codex's own README.": "OpenAI（chatgpt.com）の公式 README にあるコマンドです。", // tools.fact.codex.1
  "Installs into ~/.local/bin/codex and checks every download's SHA-256. No administrator password, no Node.js.": "~/.local/bin/codex にインストールし、ダウンロードごとに SHA-256 を検証します。管理者パスワードも Node.js も不要です。", // tools.fact.codex.2
  "Adds ~/.local/bin to PATH in ~/.zprofile so your own Terminal finds codex too. That is what the official installer does anyway.": "自分のターミナルでも codex が見つかるよう、~/.zprofile で ~/.local/bin を PATH に追加します。公式インストーラーの通常の動作です。", // tools.fact.codex.3
  "Uses your Homebrew. Homebrew copies don't update themselves; Droppen offers the update when one is out.": "お使いの Homebrew でインストールします。Homebrew 版は自動更新されないため、新しい版が出たら Droppen がお知らせします。", // tools.fact.brew
  "Uses your Node.js ({{node}}). Only if you already manage tools with npm.": "お使いの Node.js（{{node}}）を使います。npm でツールを管理している方向けです。", // tools.fact.npm
  "Run it here": "ここで実行", // tools.runHere
  "Copy command": "コマンドをコピー", // tools.copyCommand
  "Copied": "コピーしました", // tools.copied
  "Cancel": "キャンセル", // tools.cancel
  "Official guide": "公式ガイド", // tools.guide
  "Installing Claude Code. It keeps going if you leave this step.": "Claude Code をインストールしています。このステップを離れても続きます。", // tools.installing.claude
  "Installing Codex. It keeps going if you leave this step.": "Codex をインストールしています。このステップを離れても続きます。", // tools.installing.codex
  "Stop": "停止", // tools.stop
  "Terminal · on this device": "ターミナル · このデバイス", // tools.terminal
  "F6 moves between the terminal and the buttons": "F6 でターミナルとボタンを切り替え", // tools.f6
  "The install didn't finish (exit code 1): couldn't reach claude.ai. Check the connection and try again, or run the command in your own Terminal.": "インストールが完了しませんでした（終了コード 1）：claude.ai に接続できません。接続を確認して再試行するか、ご自分のターミナルでコマンドを実行してください。", // tools.failed.claude
  "Try again": "再試行", // tools.retry
  "Installed. Sign in once and Droppen can start Claude Code sessions. Your browser opens; sign in with your Claude account.": "インストール済みです。一度ログインすれば Droppen から Claude Code のセッションを始められます。ブラウザが開くので、Claude アカウントでログインしてください。", // tools.signedOut.claude
  "Installed. Sign in once and Droppen can start Codex sessions. Your browser opens; sign in with ChatGPT.": "インストール済みです。一度ログインすれば Droppen から Codex のセッションを始められます。ブラウザが開くので、ChatGPT でログインしてください。", // tools.signedOut.codex
  "Sign in": "ログイン", // tools.signIn
  "Use an Anthropic Console account instead": "Anthropic Console アカウントを使う", // tools.signIn.console
  "Use an OpenAI API key instead": "OpenAI API キーを使う", // tools.signIn.apiKey
  "Finish signing in in your browser, then come back. Droppen notices by itself.": "ブラウザでログインを済ませて戻ってきてください。Droppen が自動で検知します。", // tools.signingIn
  "Waiting for sign-in…": "ログインを待っています…", // tools.waiting
  "Browser didn't open?": "ブラウザが開きませんか？", // tools.noBrowser
  "Press c in the terminal to copy the sign-in link, open it in any browser, then paste the code it shows back into the terminal.": "ターミナルで c を押してログインリンクをコピーし、任意のブラウザで開いて、表示されたコードをターミナルに貼り付けてください。", // tools.noBrowser.claude
  "Use a one-time code instead: Droppen runs codex login --device-auth and shows a code to enter at chatgpt.com.": "ワンタイムコードを使います。Droppen が codex login --device-auth を実行し、chatgpt.com で入力するコードを表示します。", // tools.noBrowser.codex
  "Sign in with a code": "コードでログイン", // tools.useCode
  "5 hours": "5 時間", // tools.q.5h
  "this week": "今週", // tools.q.week
  "{{n}}% used": "{{n}}% 使用", // tools.q.used
  "resets {{t}}": "{{t}} にリセット", // tools.q.resets
  "Quota shows up after the first session.": "最初のセッションのあとに使用量が表示されます。", // tools.q.later
  "Use a different account": "別のアカウントにする", // tools.switch
  "Claude Code {{v}} is too old for Droppen; it needs {{min}} or later.": "Claude Code {{v}} は古すぎます。Droppen には {{min}} 以降が必要です。", // tools.tooOld
  "Update": "アップデート", // tools.update
  "Droppen couldn't read your Terminal's settings in time (over 5 s), so a copy installed with nvm, volta or similar may be hiding. Look again, or show Droppen where it is.": "ターミナルの設定を時間内（5 秒）に読み取れなかったため、nvm や volta などで入れたものが見つかっていない可能性があります。もう一度探すか、場所を指定してください。", // tools.notFound
  "Look again (wait longer)": "もう一度探す（長めに待つ）", // tools.lookAgain
  "Show where it is…": "場所を指定…", // tools.choosePath
  "Install a fresh copy": "新しくインストールする", // tools.installAnyway
  "Signed in as {{email}}, but the free Claude plan doesn't include Claude Code. Upgrade at claude.ai, or sign in with a Console account.": "{{email}} でログイン済みですが、Claude の無料プランには Claude Code が含まれていません。claude.ai でアップグレードするか、Console アカウントでログインしてください。", // tools.plan.claude
  "Plans at claude.ai": "claude.ai でプランを見る", // tools.upgrade
  "Your Terminal sets ANTHROPIC_API_KEY. Droppen doesn't take keys from your shell settings; to use that key here, add it to Droppen below.": "ターミナルで ANTHROPIC_API_KEY が設定されています。Droppen はシェル設定のキーを使いません。ここでも使うには、下で Droppen に追加してください。", // tools.envKey
  "Ready through the Droppen account “{{label}}”": "Droppen のアカウント「{{label}}」で準備完了", // tools.viaAccount
  "Developer tools (git)": "デベロッパツール（git）", // tools.git
  "Claude Code and Codex use git in code projects. Without it, macOS interrupts the first session to install Apple's Command Line Tools; you can get that done now.": "Claude Code と Codex はコードのプロジェクトで git を使います。入っていないと、最初のセッション中に macOS が Apple のコマンドラインツールのインストールを求めてきます。今のうちに入れておけます。", // tools.git.body
  "Install (Apple)": "インストール（Apple 公式）", // tools.git.install
  "installed": "インストール済み", // tools.git.ready
  "optional": "任意", // tools.optional
  "Add accounts to Droppen (advanced)": "Droppen にアカウントを追加（上級）", // tools.adv
  "For switching accounts automatically when one runs out of quota, or paying through an API key or a company gateway. Most people don't need this. Keys stay on this device and are never shown again.": "使用量が尽きたときの自動切り替えや、API キー・社内ゲートウェイでの課金向けです。ほとんどの方には不要です。キーはこのデバイスにだけ保存され、保存後は表示されません。", // tools.adv.body
  "Claude · long-lived sign-in (setup-token)": "Claude · 長期ログイントークン（setup-token）", // tools.adv.token
  "Sign in once in the browser; Droppen keeps a one-year token. Needed for automatic account switching.": "ブラウザで一度ログインすると、Droppen が 1 年有効なトークンを保存します。自動切り替えに必要です。", // tools.adv.token.body
  "Environment variables (API key, gateway)": "環境変数（API キー、ゲートウェイ）", // tools.adv.env
  "One KEY=VALUE per line, given only to the sessions this account starts.": "1 行に 1 つ KEY=VALUE。このアカウントで始めたセッションにだけ渡します。", // tools.adv.env.body
  "Codex · OpenAI API key": "Codex · OpenAI API キー", // tools.adv.codex
  "Name": "名前", // tools.adv.name
  "Add account": "アカウントを追加", // tools.adv.add
  "Start": "開始", // tools.adv.start
  "Rotation and the full list are in Settings → Accounts.": "切り替えの設定と一覧は 設定 → アカウント にあります。", // tools.adv.more
  "Continue without AI tools": "インストールせずに続ける", // tools.none
  "Without Claude Code or Codex, Droppen can show files and open terminals but can't run AI. Come back any time from Settings → General → Setup.": "Claude Code も Codex もない場合、Droppen でファイルやターミナルは使えますが AI は動かせません。設定 → 一般 → セットアップ からいつでも戻れます。", // tools.none.body
  // Step 02 · Where to use it (A.4)
  "02 · Where to use it": "02 · 使う場所", // where.kicker
  "Where will you use Droppen?": "どこで Droppen を使いますか？", // where.title
  "Droppen runs on this Mac either way. This only decides what else can reach it, and you can change it any time in Settings.": "どれを選んでも Droppen はこの Mac で動きます。ここで決めるのは、ほかに何から接続できるかだけで、いつでも「設定」で変更できます。", // where.body
  "On my phone and other devices too": "スマートフォンやほかのデバイスでも使う", // where.droppen.title
  "Sign in to Droppen": "Droppen にサインイン", // where.droppen.action
  "Open droppen.app on your phone or any other device and carry on with this Mac's sessions. Content is end-to-end encrypted; droppen.app can't read it. Your browser opens to sign in or create an account.": "スマートフォンやほかのデバイスで droppen.app を開き、この Mac のセッションの続きができます。内容はエンドツーエンドで暗号化され、droppen.app からは読めません。ブラウザが開くので、ログインまたは登録してください。", // where.droppen.body
  "Recommended": "おすすめ", // where.recommended
  "Only on this Mac": "この Mac だけで使う", // where.local.title
  "Skip": "スキップ", // where.local.action
  "Everything works without an account. Nothing leaves this Mac except what Claude Code and Codex send to their own services.": "アカウントなしですべて使えます。Claude Code と Codex がそれぞれのサービスに送る内容以外、この Mac から何も出ていきません。", // where.local.body
  "Also": "追加で", // where.also
  "Let browsers on my local network in too": "同じネットワークのブラウザからも使う", // where.lan.title
  "Set a password for this device (advanced)": "このデバイスのパスワードを設定（上級）", // where.lan.action
  "An iPad or another device on the same Wi‑Fi opens this Mac's address in a browser and signs in with the password. Doesn't go through droppen.app, and works alongside either choice above.": "同じ Wi‑Fi の iPad やほかのデバイスのブラウザでこの Mac のアドレスを開き、パスワードで入ります。droppen.app は経由せず、上のどちらとも併用できます。", // where.lan.body
  "The Droppen app on this Mac never asks for a password. For a browser on this Mac, choose Open in Browser in the app's menu-bar menu: it opens signed in. Once this device has a password, a browser asks for it.": "この Mac の Droppen アプリはパスワードを求めません。この Mac のブラウザでは、アプリのメニューバーのメニューで「ブラウザで開く」を選ぶと、ログインした状態で開きます。このデバイスにパスワードを設定したあとは、ブラウザでそのパスワードが必要になります。", // where.browserNote
  "Sign in in the browser": "ブラウザでログイン", // where.go.droppen
  "Set the password": "パスワードを設定", // where.go.lan
  "Change it later in Settings → Access.": "あとから 設定 → アクセス で変更できます。", // where.changeLater
  "droppen.app needs the internet. Choose “Only on this Mac” for now; sign in later from Settings → Access → Droppen.": "droppen.app にはインターネットが必要です。いまは「この Mac だけで使う」を選び、あとで 設定 → アクセス → Droppen からログインしてください。", // where.offline
  "Continue in your browser": "ブラウザで続けてください", // where.wait.title
  "Sign in to droppen.app, or create an account.": "droppen.app にサインインするか、アカウントを作成します。", // where.wait.1: the app's own wording
  "Check that the fingerprint there matches this one, then choose Allow.": "表示されたフィンガープリントがこれと一致することを確認して「許可」を選びます。", // where.wait.2
  "Come back here. This Mac joins your account by itself.": "ここに戻ると、この Mac が自動でアカウントに加わります。", // where.wait.3
  "Fingerprint": "フィンガープリント", // where.fingerprint
  "Open the browser again": "ブラウザをもう一度開く", // where.reopen
  "Copy link": "リンクをコピー", // where.copyLink
  "The link works for 10 minutes ({{left}} left).": "リンクの有効期限は 10 分です（残り {{left}}）。", // where.expires
  "Waiting for droppen.app…": "droppen.app を待っています…", // where.waiting
  "Keep going; I'll finish in the browser": "先に進む（ブラウザで続ける）", // where.keepGoing
  "This Mac is in your Droppen account": "この Mac は Droppen アカウントに追加されました", // where.linked.title
  "Open droppen.app on your phone and sign in as {{email}}. “{{device}}” is there.": "スマートフォンで droppen.app を開き、{{email}} でログインすると「{{device}}」があります。", // where.linked.body
  "Device name": "デバイス名", // where.deviceName
  "Scan with your phone": "スマートフォンで読み取る", // where.scan
  "Can't reach droppen.app": "droppen.app に接続できません", // where.err.title
  "Use Droppen on this Mac for now. When you're online, sign in from Settings → Access → Droppen; nothing done here is lost.": "いまはこの Mac だけで使ってください。オンラインになったら 設定 → アクセス → Droppen からログインできます。ここでの設定は失われません。", // where.err.body
  "Your plan's devices that run AI are all in use. Remove one at droppen.app or change plan; until then this Mac works on its own.": "プランで AI を動かせるデバイスが上限に達しています。droppen.app で 1 台外すかプランを変更してください。それまではこの Mac 単体で使えます。", // where.err.limit
  "Only on this Mac for now": "今はこの Mac だけで使う", // where.localNow
  "A password for browsers on your network": "ネットワーク上のブラウザ用のパスワード", // where.lan.formTitle
  "Browsers on other devices sign in with this name and password. The Droppen app on this Mac doesn't need it.": "ほかのデバイスのブラウザはこの名前とパスワードでログインします。この Mac の Droppen アプリには不要です。", // where.lan.formBody
  "Password": "パスワード", // where.lan.pw
  "Password again": "パスワード（確認）", // where.lan.pw2
  "At least 12 characters.": "12 文字以上。", // where.lan.rule
  "Open to my local network now": "今すぐネットワークに公開する", // where.lan.open
  "Connections on the local network aren't encrypted (http). Only turn this on at home, at work or on other networks you trust.": "ローカルネットワーク内の通信は暗号化されません（http）。自宅や職場など信頼できるネットワークでだけオンにしてください。", // where.lan.warn
  "macOS may ask whether Droppen can accept incoming connections. Choose Allow.": "macOS が Droppen への接続を許可するか尋ねたら「許可」を選んでください。", // where.lan.firewall
  "Open on your local network": "ネットワークに公開中", // where.lan.onTitle
  "Address": "アドレス", // where.lan.addr
  "Open this in a browser on the same network. If the name doesn't work, use the number.": "同じネットワークのブラウザでこのアドレスを開きます。名前で開けない場合は数字のアドレスを使ってください。", // where.lan.addrBody
  "The port stays {{port}} while this is on.": "公開中はポート {{port}} のままです。", // where.lan.port
  "Copy address": "アドレスをコピー", // where.lan.copy
  // Step 03 · First session (A.5)
  "03 · First session": "03 · 最初のセッション", // first.kicker
  "Pick a folder for the AI to work in": "AI が作業するフォルダを選ぶ", // first.title
  "The AI works in the folder you pick. The suggested first messages only ask it to look, and nothing runs until you press Send on the next screen, where the permission it runs with is shown and can be changed.": "AI は選んだフォルダの中で作業します。おすすめの最初のメッセージは見てもらうだけの内容で、次の画面で送信を押すまで何も実行されません。どの権限で動くかもその画面に表示され、変更できます。", // first.body, rewritten: the first session runs with the device's own permission
  "Used recently with Claude Code or Codex": "最近 Claude Code か Codex で使ったフォルダ", // first.recent
  "Make a practice folder": "練習用のフォルダを作る", // first.practice
  "~/Droppen/first-session: empty, safe to experiment in.": "~/Droppen/first-session：空のフォルダで、自由に試せます。", // first.practice.body
  "Choose another folder…": "ほかのフォルダを選ぶ…", // first.other
  "Folders in Documents, Desktop or Downloads: macOS asks once whether Droppen may open them. Choose Allow.": "「書類」「デスクトップ」「ダウンロード」内のフォルダを選ぶと、macOS が一度だけアクセスの許可を求めます。「許可」を選んでください。", // first.tcc
  "{{n}} sessions": "{{n}} 件のセッション", // first.sessions
  "Run with": "使うツール", // first.agent
  "First message": "最初のメッセージ", // first.prompt
  "Explain this project: what it does and how to run it. Don't change any files yet.": "このプロジェクトを説明してください。何をするもので、どう動かすのか。まだファイルは変更しないでください。", // first.p.repo1
  "List the three things most worth fixing here first. Just list them; don't change anything.": "このプロジェクトでまず直すべき点を 3 つ挙げてください。挙げるだけで、変更はしないでください。", // first.p.repo2
  "Make a one-page website that shows today's date and a short quote, then tell me how to open it.": "今日の日付と短い名言を表示する 1 ページのウェブサイトを作って、開き方を教えてください。", // first.p.practice
  "Write my own": "自分で書く", // first.p.own
  "You send it yourself on the next screen; nothing runs until you press Send.": "送信は次の画面でご自身で行います。送信を押すまで何も実行されません。", // first.note
  "No AI tool is ready yet, so there is nothing to start. Get one ready in step 1, or skip this step.": "使える AI ツールがまだないため、始められません。ステップ 1 で準備するか、このステップをスキップしてください。", // first.noTool
  // Step 04 · Finishing up (A.6)
  "04 · Finishing up": "04 · 仕上げ", // wrap.kicker
  "A few last things": "最後にいくつか", // wrap.title
  "Start Droppen at login": "ログイン時に Droppen を起動", // wrap.login
  "So sessions, your phone and scheduled tasks can reach this Mac after a restart. Droppen lives in the menu bar; closing the window doesn't stop it.": "再起動後も、セッション・スマートフォン・定期タスクがこの Mac に接続できます。Droppen はメニューバーに常駐し、ウインドウを閉じても止まりません。", // wrap.login.body
  "Move Droppen to Applications first.": "先に Droppen を「アプリケーション」に移動してください。", // wrap.login.move
  "Notifications": "通知", // wrap.notif
  "A macOS notification when a session finishes or asks you something.": "セッションが終わったときや質問があるときに macOS の通知でお知らせします。", // wrap.notif.body
  "Allow notifications": "通知を許可", // wrap.notif.allow
  "Turned off in System Settings → Notifications → Droppen.": "システム設定 → 通知 → Droppen でオフになっています。", // wrap.notif.denied
  "Open System Settings": "システム設定を開く", // wrap.notif.open
  "On your phone": "スマートフォンで", // wrap.phone
  "Open droppen.app and sign in as {{email}}.": "droppen.app を開いて {{email}} でログインします。", // wrap.phone.linked
  "Want it on your phone too? Sign in to Droppen any time in Settings → Access → Droppen.": "スマートフォンでも使いたいときは、いつでも 設定 → アクセス → Droppen からログインできます。", // wrap.phone.not
  "What's set": "設定内容", // wrap.summary
  "Droppen · {{email}}": "Droppen · {{email}}", // wrap.sum.linked
  "Local network · {{addr}}": "ローカルネットワーク · {{addr}}", // wrap.sum.lan
  "Finish and open my first session": "完了して最初のセッションを開く", // wrap.finish
  "Finish": "完了", // wrap.finishPlain
  "Run this again from Settings → General → Setup.": "設定 → 一般 → セットアップ からもう一度実行できます。", // wrap.rerun
  // After the guide: landing, resume bar, Settings (A.7)
  "Press Send when you're ready. Model, effort and permission are in the row below.": "準備ができたら送信してください。モデル・思考の深さ・権限は下の行で変えられます。", // after.landed
  "Setup: {{n}} steps left": "セットアップ：残り {{n}} ステップ", // after.resume
  "Setup: 1 step left": "セットアップ：残り 1 ステップ", // after.resume, one step (new)
  "Don't remind me": "今後表示しない", // after.resume.off
  "No AI tool is ready on this device yet.": "このデバイスにはまだ使える AI ツールがありません。", // after.noTool
  "Get one ready": "準備する", // after.noTool.go
  "Setup": "セットアップ", // after.settings.title
  "Check Claude Code and Codex, change where you use Droppen, pick a first folder again.": "Claude Code と Codex の確認、使う場所の変更、最初のフォルダの選び直し。", // after.settings.body
  "Run setup again": "もう一度実行", // after.settings.rerun
  "Claude Code is signed in. You can go back to Droppen.": "Claude Code にログインしました。Droppen に戻れます。", // after.signedInToast
  // Step 02 · Where to use it (A.4)
  "Signing in from the app is coming soon": "アプリからのログインはまもなく対応します", // where.droppen.soon
  "This Mac stays in your Droppen account. To take it out, use Settings → Access → Droppen.": "この Mac は Droppen アカウントに残ります。外すには 設定 → アクセス → Droppen から。", // where.local.stays (as built)
  "This window cannot sign this device in. See Settings → Access → Droppen.": "このウィンドウからはこのデバイスをサインインできません。設定 → アクセス → Droppen を見てください。", // where.unavailable (as built)
  // Step 04 · Finishing up (A.6)
  "Turn it on or off any time from the Droppen icon in the menu bar: Start at Login.": "メニューバーの Droppen アイコンの「ログイン時に起動」で、いつでもオン・オフを切り替えられます。", // wrap.login.menuBar
  // Step 01 · AI tools, as built: the states, forms and failures the copy table leaves to the implementation
  "Close": "閉じる",
  "macOS opened Apple's installer. Finish it there, then check again.": "macOS で Apple のインストーラーが開きました。そちらで完了してから、もう一度確認してください。",
  "Check again": "もう一度確認",
  "Added to Droppen as “{{label}}”.": "「{{label}}」として Droppen に追加しました。",
  "These run from the Droppen app on this Mac. Copy the command and run it in your own Terminal.": "これらはこの Mac の Droppen アプリからのみ実行できます。コマンドをコピーして、ご自分のターミナルで実行してください。",
  "OpenAI API key": "OpenAI API キー",
  "Droppen hands the key to codex login --with-api-key once, through its input. Codex keeps it; Droppen doesn't.": "Droppen はキーを標準入力から codex login --with-api-key に一度だけ渡します。保存するのは Codex で、Droppen は保存しません。",
  "Where is it?": "どこにありますか？",
  "In your own Terminal, `which {{name}}` prints it.": "ご自分のターミナルで `which {{name}}` を実行すると表示されます。",
  "Use this one": "これを使う",
  "Droppen can't install it on this device. The official guide lists the other ways.": "Droppen はこのデバイスにインストールできません。ほかの方法は公式ガイドにあります。",
  "Add it to Droppen": "Droppen に追加",
  "Want claude in your own Terminal too? The installer leaves that to you; this line adds ~/.local/bin to your PATH:": "ご自分のターミナルでも claude を使いますか？インストーラーは設定しないので、次の 1 行で ~/.local/bin を PATH に追加します：",
  "Signing in with the API key…": "API キーでログインしています…",
  "Signing in didn't finish (exit code {{code}}). Try again.": "ログインが完了しませんでした（終了コード {{code}}）。もう一度お試しください。",
  "Droppen account": "Droppen アカウント",
  "Signed in": "ログイン済み",
  "Looking… (up to 20 s)": "探しています…（最大 20 秒）",
  "Found {{name}} at {{path}}, outside the places Droppen's sessions look. Use this copy?": "{{path}} に {{name}} が見つかりましたが、Droppen のセッションが探す場所の外です。これを使いますか？",
  "Found it, use it": "見つかったものを使う",
  "The free Claude plan doesn't include Claude Code. Upgrade at claude.ai, or sign in with a Console account.": "Claude の無料プランには Claude Code が含まれていません。claude.ai でアップグレードするか、Console アカウントでログインしてください。",
  "More": "その他",
  "Sign in with single sign-on (SSO)": "シングルサインオン（SSO）でログイン",
  "updating": "アップデート中",
  "found elsewhere": "別の場所で検出",
  "Updating Claude Code. It keeps going if you leave this step.": "Claude Code をアップデートしています。このステップを離れても続きます。",
  "Updating Codex. It keeps going if you leave this step.": "Codex をアップデートしています。このステップを離れても続きます。",
  "Codex {{v}} is too old for Droppen; it needs {{min}} or later.": "Codex {{v}} は古すぎます。Droppen には {{min}} 以降が必要です。",
  "Your Terminal sets OPENAI_API_KEY. Droppen doesn't take keys from your shell settings; to use that key here, add it to Droppen below.": "ターミナルで OPENAI_API_KEY が設定されています。Droppen はシェル設定のキーを使いません。ここでも使うには、下で Droppen に追加してください。",
  "Codex is signed in. You can go back to Droppen.": "Codex にログインしました。Droppen に戻れます。",
  "The install didn't finish (exit code {{code}}).": "インストールが完了しませんでした（終了コード {{code}}）。",
  "The update didn't finish (exit code {{code}}).": "アップデートが完了しませんでした（終了コード {{code}}）。",
  "The installer finished, but the tool still isn't on this device.": "インストーラーは終了しましたが、このデバイスにはまだツールが見つかりません。",
  "The update finished, but the version is still too old.": "アップデートは終了しましたが、バージョンはまだ古いままです。",
  "Couldn't reach the download server. Check the connection and try again, or run the command in your own Terminal.": "ダウンロードサーバーに接続できません。接続を確認して再試行するか、ご自分のターミナルでコマンドを実行してください。",
  "Permission denied. Homebrew on this Mac may belong to another user; the official installer installs into your home folder instead.": "権限がありません。この Mac の Homebrew は別のユーザーのものかもしれません。公式インストーラーならホームフォルダにインストールします。",
  "Permission denied: it couldn't write where it installs. Try the official installer, which installs into your home folder, or run the command in your own Terminal.": "権限がありません：インストール先に書き込めませんでした。ホームフォルダにインストールする公式インストーラーを試すか、ご自分のターミナルでコマンドを実行してください。",
  "The disk is full. Free some space, then try again.": "ディスクがいっぱいです。空きを作ってから再試行してください。",
  "The last lines are below. Try again, or run the command in your own Terminal.": "最後の数行を下に表示しています。再試行するか、ご自分のターミナルでコマンドを実行してください。",
  "The official installer needs macOS 13 or later.": "公式インストーラーには macOS 13 以降が必要です。",
  "Homebrew isn't on this Mac.": "この Mac には Homebrew がありません。",
  "npm needs Node.js, which isn't on this Mac.": "npm には Node.js が必要ですが、この Mac にはありません。",
  "Not available on this device.": "このデバイスでは使えません。",
  "Something is already running for this tool. Wait for it, or stop it.": "このツールではすでに処理が実行中です。終わるのを待つか、停止してください。",
  "That isn't a copy Droppen can use. Pick the program itself, named claude or codex.": "Droppen が使えるプログラムではありません。claude または codex という名前のプログラム本体を指定してください。",
  "That didn't work. Try again.": "うまくいきませんでした。もう一度お試しください。",
  "Claude · environment": "Claude · 環境変数",
  "Codex · API key": "Codex · API キー",
  // Step 03 · First session, as built
  "Back to step 1": "ステップ 1 に戻る",
};
