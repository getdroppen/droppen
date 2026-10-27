// The zh-TW table of `welcome.ts`, which says what it covers: a file of its
// own, so that it is in the zh-TW chunk alone (`lang-zh-TW.ts`).

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
