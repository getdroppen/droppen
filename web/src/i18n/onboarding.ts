/*
 * Adding a device and seeing devices side by side, without anyone to walk
 * you through it: droppen.app's 「添加可运行 AI 的设备」 guide
 * (`features/cloud/add-computer.tsx`), the hints while the account has one
 * device that runs AI and the one-time side-by-side tip
 * (`features/cloud/onboarding-hints.tsx`), 「我的设备」's first step after
 * signing up, and the local page's first-run card (`features/link/first-run.tsx`).
 * Kept apart from the main tables and merged under them in `index.ts`; keys
 * are the English source strings, as everywhere. The interface says 「设备」 /
 * 裝置 / デバイス, never 「机器」 or 「电脑」 (`test/ui-wording.test.ts`); the
 * buttons it names are named as the app names them (「登录 Droppen」,
 * 「加入这台设备」, 「添加浏览器或手机」).
 */

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
  "— or add it from its command line with a one-time code:": "也可以用一次性注册码从命令行添加：",
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

  // 「我的设备」 after signing up
  "Next: install Droppen on the device your AI runs on, then add it here.": "下一步：把 Droppen 装到跑 AI 的那台设备上，再在这里添加它。",

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
};

export const onboardingZhTW: Record<string, string> = {
  // The App first, the server way second
  "01 · install the Droppen App on that device": "01 · 在那台裝置上安裝 Droppen App",
  "On the device your AI runs on (a Mac, for now), download the Droppen App and open it: Droppen starts there by itself, with no login page.": "在執行 AI 的那台裝置上（目前支援 Mac）下載並打開 Droppen App：Droppen 會自己在那台裝置上啟動，不需要登入頁。",
  "Download Droppen App (macOS)": "下載 Droppen App（macOS）",
  "The Droppen App for Mac is coming soon; until then, Droppen runs in the background.": "Mac 版 Droppen App 即將推出，在那之前先讓 Droppen 在背景執行。",
  "The Droppen App cannot sign in to Droppen yet (that is coming in an update): to add a Mac now, run Droppen in the background.":
    "Droppen App 暫時還不能登入 Droppen（會在後續更新中推出）。現在要新增 Mac，請先讓 Droppen 在背景執行。",
  "02 · in the App, click Sign in to Droppen": "02 · 在 App 裡點「登入 Droppen」",
  "In the Droppen App on that device, click Sign in to Droppen (it is also under Settings → Access → Droppen).": "在那台裝置的 Droppen App 裡點「登入 Droppen」（也在「設定 → 存取 → Droppen」裡）。",
  "Server or running in the background?": "伺服器或背景執行？",
  "Install it with one line:": "一行指令安裝：",
  "Mac in the background": "Mac 背景執行",
  "Linux server": "Linux 伺服器",
  "— or add it from its command line with a one-time code:": "也可以用一次性註冊碼從命令列新增：",
  "01 · install Droppen on that device": "01 · 在那台裝置上安裝 Droppen",
  "On the device your AI runs on (a Mac, for now), open Terminal and run:": "在執行 AI 的那台裝置上（目前支援 Mac），打開「終端機」執行：",
  "download page": "下載頁",
  "It starts Droppen there, makes a local login for it, and opens its page.": "它會在那台裝置上啟動 Droppen，建立本地登入，並打開它的頁面。",
  "02 · on its page, sign in to Droppen": "02 · 在它的頁面上登入 Droppen",
  "On the page that opens on that device, sign in with its local login and click Sign in to Droppen (it is also under Settings → Access → Droppen).":
    "在那台裝置打開的頁面上，用本地登入進去，點「登入 Droppen」（也在「設定 → 存取 → Droppen」裡）。",
  "A {{app}} window opens there: sign in to this account, check that the fingerprint matches the one its page shows, and click add this device.":
    "那邊會彈出 {{app}} 視窗：登入這個帳戶，核對指紋和它頁面上顯示的一致，然後點「加入這台裝置」。",
  "This account is on {{hub}}, not droppen.app: on that page, open advanced first and set the hub address to {{hub}}.":
    "這個帳戶在 {{hub}} 上，不在 droppen.app：先在那個頁面上打開「進階選項」，把 Hub 位址設成 {{hub}}。",
  "Is that window's browser new to your account? It shows a code first: add it from here.": "那個視窗所在的瀏覽器還沒加入你的帳戶？它會先顯示一個配對碼，在這裡新增它：",
  "03 · it comes online here": "03 · 在這裡等它上線",
  "{{name}} joined the account; waiting for it to come online…": "{{name}} 已加入帳戶，正在等它上線…",
  "waiting for the new device to come online…": "正在等新裝置上線…",
  "Keep this page open: it notices the new device by itself.": "這個頁面開著就好，新裝置上線會自動提示。",
  "{{name}} is online": "{{name}} 已上線",
  "{{name}} is in your account": "{{name}} 已加入你的帳戶",
  "Now you can put the two devices on one screen.": "現在可以把兩台裝置放進同一個畫面",
  "{{a}} and {{b}}, side by side: each pane opens on its own device's latest session, or a terminal on it, and you can change what each one shows.":
    "{{a}} 和 {{b}} 並排：每個格子打開各自裝置上最近的工作階段（沒有的話就開一個終端機），每個格子裡放什麼都可以再換。",
  "Open it to start a session there. Add another device the same way to see both on one screen.": "打開它，就能在上面開工作階段。用同樣的方法再加一台裝置，就能在一個畫面裡同時看。",
  "put them on one screen": "放進同一個畫面",
  "open {{name}}": "打開 {{name}}",
  "{{name}} keeps your screens and cannot be reached right now. Try again once it is online.": "畫面保存在 {{name}} 上，它現在連不上。等它上線後再試。",
  "The screen could not be made: {{message}}": "畫面沒能建立：{{message}}",

  "Next: install Droppen on the device your AI runs on, then add it here.": "下一步：把 Droppen 裝到執行 AI 的那台裝置上，再在這裡新增它。",

  "Add another device to see both on one screen.": "再加一台裝置，就能在一個畫面裡同時看",
  "add": "新增",
  "Each pane can show a different device's content.": "每個格子都可以放不同裝置的內容",
  "Make a screen with the rail's +, then pick a device in each pane.": "用左側欄的「+」建立一個畫面，再替每個格子選裝置。",

  "Sign in to Droppen to see your other devices here, and to use this one from your phone.": "登入 Droppen，就能在這裡看到你的其他裝置，也能在手機上用這一台。",
  "not now": "以後再說",

  "Droppen is not on that device yet?": "那台裝置上還沒裝 Droppen？",
};

export const onboardingJa: Record<string, string> = {
  // The App first, the server way second
  "01 · install the Droppen App on that device": "01 · そのデバイスに Droppen アプリをインストール",
  "On the device your AI runs on (a Mac, for now), download the Droppen App and open it: Droppen starts there by itself, with no login page.": "AI を動かすデバイス（現在は Mac）で Droppen アプリをダウンロードして開きます。Droppen は自動で起動し、ログイン画面はありません。",
  "Download Droppen App (macOS)": "Droppen アプリをダウンロード（macOS）",
  "The Droppen App for Mac is coming soon; until then, Droppen runs in the background.": "Mac 版 Droppen アプリは近日公開です。それまではバックグラウンドで動かします。",
  "The Droppen App cannot sign in to Droppen yet (that is coming in an update): to add a Mac now, run Droppen in the background.":
    "Droppen アプリからの Droppen へのサインインは、今後のアップデートで対応します。今 Mac を追加するには、Droppen をバックグラウンドで動かします。",
  "02 · in the App, click Sign in to Droppen": "02 · アプリで「Droppen にサインイン」を押す",
  "In the Droppen App on that device, click Sign in to Droppen (it is also under Settings → Access → Droppen).": "そのデバイスの Droppen アプリで「Droppen にサインイン」を押します（「設定 → アクセス → Droppen」にもあります）。",
  "Server or running in the background?": "サーバーやバックグラウンドで動かすなら",
  "Install it with one line:": "コマンド 1 行でインストール：",
  "Mac in the background": "Mac のバックグラウンド",
  "Linux server": "Linux サーバー",
  "— or add it from its command line with a one-time code:": "またはワンタイムコードでコマンドラインから追加：",
  "01 · install Droppen on that device": "01 · そのデバイスに Droppen をインストール",
  "On the device your AI runs on (a Mac, for now), open Terminal and run:": "AI を動かすデバイス（現在は Mac）で「ターミナル」を開き、次を実行します：",
  "download page": "ダウンロードページ",
  "It starts Droppen there, makes a local login for it, and opens its page.": "そのデバイスで Droppen を起動し、ローカルログインを作って、ページを開きます。",
  "02 · on its page, sign in to Droppen": "02 · そのページで Droppen にサインイン",
  "On the page that opens on that device, sign in with its local login and click Sign in to Droppen (it is also under Settings → Access → Droppen).":
    "そのデバイスで開いたページにローカルログインで入り、「Droppen にサインイン」を押します（「設定 → アクセス → Droppen」にもあります）。",
  "A {{app}} window opens there: sign in to this account, check that the fingerprint matches the one its page shows, and click add this device.":
    "{{app}} のウィンドウが開くので、このアカウントでサインインし、フィンガープリントがページの表示と一致することを確かめて「このデバイスを追加」を押します。",
  "This account is on {{hub}}, not droppen.app: on that page, open advanced first and set the hub address to {{hub}}.":
    "このアカウントは droppen.app ではなく {{hub}} にあります。先にそのページで「詳細設定」を開き、Hub のアドレスを {{hub}} にしてください。",
  "Is that window's browser new to your account? It shows a code first: add it from here.": "そのウィンドウのブラウザがまだアカウントにない場合は、先にコードが表示されます。ここから追加してください：",
  "03 · it comes online here": "03 · ここでオンラインになるのを待つ",
  "{{name}} joined the account; waiting for it to come online…": "{{name}} がアカウントに加わりました。オンラインになるのを待っています…",
  "waiting for the new device to come online…": "新しいデバイスがオンラインになるのを待っています…",
  "Keep this page open: it notices the new device by itself.": "このページを開いたままにしておけば、新しいデバイスに自動で気づきます。",
  "{{name}} is online": "{{name}} がオンラインになりました",
  "{{name}} is in your account": "{{name}} がアカウントに加わりました",
  "Now you can put the two devices on one screen.": "2 台のデバイスをひとつの画面に並べられるようになりました",
  "{{a}} and {{b}}, side by side: each pane opens on its own device's latest session, or a terminal on it, and you can change what each one shows.":
    "{{a}} と {{b}} を並べて表示します。各ペインはそれぞれのデバイスの最新のセッション（なければターミナル）を開き、表示する内容はあとから変えられます。",
  "Open it to start a session there. Add another device the same way to see both on one screen.": "開けば、そこでセッションを始められます。同じ手順でもう 1 台追加すれば、ひとつの画面で同時に見られます。",
  "put them on one screen": "ひとつの画面に並べる",
  "open {{name}}": "{{name}} を開く",
  "{{name}} keeps your screens and cannot be reached right now. Try again once it is online.": "画面は {{name}} に保存されますが、今は接続できません。オンラインになってからもう一度お試しください。",
  "The screen could not be made: {{message}}": "画面を作れませんでした：{{message}}",

  "Next: install Droppen on the device your AI runs on, then add it here.": "次に、AI を動かすデバイスに Droppen をインストールして、ここで追加します。",

  "Add another device to see both on one screen.": "もう 1 台デバイスを追加すると、ひとつの画面で同時に見られます",
  "add": "追加",
  "Each pane can show a different device's content.": "どのペインにも、別のデバイスの内容を置けます",
  "Make a screen with the rail's +, then pick a device in each pane.": "左のレールの「+」で画面を作り、ペインごとにデバイスを選びます。",

  "Sign in to Droppen to see your other devices here, and to use this one from your phone.": "Droppen にサインインすると、ほかのデバイスをここで見られ、このデバイスをスマホから使えます。",
  "not now": "あとで",

  "Droppen is not on that device yet?": "そのデバイスにまだ Droppen が入っていない場合は",
};
