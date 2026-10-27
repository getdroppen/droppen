// The ja table of `welcome.ts`, which says what it covers: a file of its
// own, so that it is in the ja chunk alone (`lang-ja.ts`).

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
