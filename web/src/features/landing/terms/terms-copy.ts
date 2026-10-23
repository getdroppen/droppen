/*
 * The words of the terms of service (`terms.tsx`), droppen.app/terms. As
 * everywhere (`src/i18n`), the keys are the English text, so English needs no
 * table; zh-CN is written as native copy, zh-TW and ja say the same things,
 * and `test/terms-page.test.tsx` holds the three to the page's keys. The
 * lines the page shares with the other public pages ("Terms of Service",
 * "Last updated {{date}}", "Your account", "Contact") are the landing
 * tables' (`pricing-copy.ts`).
 *
 * Plain words, no legalese: the voice of the privacy notice. Product words as
 * on the other public pages (`test/ui-wording.test.ts`): a device (设备 /
 * 裝置 / デバイス), a device that runs AI (可运行 AI 的设备 / 可執行 AI 的裝置
 * / AI を実行できるデバイス), "our servers", an account (账号 / 帳號 /
 * アカウント) — never a machine or a computer, and no relay. The plans'
 * numbers and the minimum age are interpolated, never written into a line.
 *
 * Its own chunk: loaded with the page only, never with the landing page.
 */

type Bundle = Record<string, string>;

/** Each line once: the English (the key), then zh-CN, zh-TW and ja, in the order the page says them. */
const LINES: readonly (readonly [en: string, zhCN: string, zhTW: string, ja: string])[] = [
  ["Terms of Service — Droppen", "服务条款 — Droppen", "服務條款 — Droppen", "利用規約 — Droppen"],
  ["What you can expect from us, and what we ask of you.", "我们承诺什么，也请你遵守什么。", "我們承諾什麼，也請你遵守什麼。", "私たちがお約束すること、お願いすること。"],
  [
    "These terms are the agreement between you and Droppen (“we”), which operates droppen.app. Creating an account, or using the service, means you accept them.",
    "本条款是你与 Droppen（下称「我们」，droppen.app 的运营方）之间的协议。创建账号或使用这项服务，即表示你接受本条款。",
    "本條款是你與 Droppen（下稱「我們」，droppen.app 的營運方）之間的協議。建立帳號或使用這項服務，即表示你接受本條款。",
    "この規約は、あなたと、droppen.app を運営する Droppen（以下「私たち」）との間の取り決めです。アカウントを作成するか、サービスを利用した時点で、この規約に同意したことになります。",
  ],

  // The service
  ["The service", "服务", "服務", "サービス"],
  ["What these terms cover", "本条款适用于什么", "本條款適用於什麼", "この規約の対象"],
  ["What Droppen is", "Droppen 是什么", "Droppen 是什麼", "Droppen とは"],
  [
    "Droppen lets you work with the AI coding tools on your own devices from anywhere: from your phone, or from a browser on another device.",
    "Droppen 让你在任何地方使用自己设备上的 AI 编程工具：用手机，或用另一台设备上的浏览器。",
    "Droppen 讓你在任何地方使用自己裝置上的 AI 程式設計工具：用手機，或用另一台裝置上的瀏覽器。",
    "Droppen を使うと、自分のデバイスにある AI コーディングツールを、スマホや別のデバイスのブラウザから、どこからでも使えます。",
  ],
  [
    "Your sessions and terminals are end-to-end encrypted on your devices, and only your devices hold the keys. We cannot read them, so we cannot check, restore or hand over what is in them.",
    "你的会话和终端在你的设备上端到端加密，钥匙只在你的设备里。我们读不了它们，所以也无法检查、恢复或交出其中的内容。",
    "你的工作階段和終端機在你的裝置上端對端加密，金鑰只在你的裝置裡。我們讀不了它們，所以也無法檢查、復原或交出其中的內容。",
    "セッションとターミナルはあなたのデバイス上でエンドツーエンド暗号化され、鍵を持つのはあなたのデバイスだけです。私たちには読めないため、その中身を確認することも、復元することも、誰かに渡すこともできません。",
  ],
  ["What they cover, and what they don't", "适用于什么，不适用于什么", "適用於什麼，不適用於什麼", "対象になるもの、ならないもの"],
  [
    "These terms cover only the hosted service at droppen.app: your account, and our servers that connect your devices and keep their encrypted copies.",
    "本条款只适用于 droppen.app 上的托管服务：你的账号，以及连接你各台设备、保管其加密副本的我们的服务器。",
    "本條款只適用於 droppen.app 上的託管服務：你的帳號，以及連接你各台裝置、保管其加密副本的我們的伺服器。",
    "この規約が対象とするのは、droppen.app のホスト型サービスだけです。つまり、あなたのアカウントと、デバイス同士をつなぎ、その暗号化されたコピーを保管する私たちのサーバーです。",
  ],
  [
    "The Droppen app and server are free software, licensed under the AGPL-3.0 (some of their libraries under MIT or Apache-2.0), with their source code at github.com/getdroppen/droppen. Those licences, not these terms, govern the code.",
    "Droppen 应用和服务端是自由软件，以 AGPL-3.0 授权（部分库以 MIT 或 Apache-2.0 授权），源代码在 github.com/getdroppen/droppen。代码适用的是这些许可证，而不是本条款。",
    "Droppen 應用程式和伺服器是自由軟體，以 AGPL-3.0 授權（部分函式庫以 MIT 或 Apache-2.0 授權），原始碼在 github.com/getdroppen/droppen。程式碼適用的是這些授權條款，而不是本條款。",
    "Droppen のアプリとサーバーは AGPL-3.0 のもとで提供されるフリーソフトウェアで（一部のライブラリは MIT または Apache-2.0）、ソースコードは github.com/getdroppen/droppen にあります。コードに適用されるのはこれらのライセンスで、この規約ではありません。",
  ],
  [
    "Local only, on your own devices with no account, does not use our service at all.",
    "「仅本地」用法（只在自己的设备上用，不登录账号）完全不经过我们的服务。",
    "「僅本地」用法（只在自己的裝置上用，不登入帳號）完全不經過我們的服務。",
    "アカウントなしで自分のデバイスだけで使う「ローカルのみ」は、私たちのサービスを一切使いません。",
  ],

  // Your account
  ["One person, one account", "一人一个账号", "一人一個帳號", "1 人に 1 アカウント"],
  ["Signing up", "注册", "註冊", "登録について"],
  [
    "You must be at least {{age}} years old to use droppen.app.",
    "年满 {{age}} 岁才能使用 droppen.app。",
    "年滿 {{age}} 歲才能使用 droppen.app。",
    "droppen.app を利用できるのは {{age}} 歳以上の方です。",
  ],
  [
    "An account is for one person: don't share it with others.",
    "一个账号只给一个人用，不要与他人共用。",
    "一個帳號只給一個人用，不要與他人共用。",
    "アカウントは 1 人で使うものです。他の人と共有しないでください。",
  ],
  [
    "One account per person and per e-mail inbox: an alias or a +tag of the same inbox counts as the same one. Don't make more to get around a plan's limits.",
    "每人、每个邮箱只能有一个账号：同一个邮箱的别名或 +tag 地址也算同一个。不要为了绕过方案的限制多开账号。",
    "每人、每個信箱只能有一個帳號：同一個信箱的別名或 +tag 地址也算同一個。不要為了繞過方案的限制多開帳號。",
    "アカウントは 1 人につき 1 つ、メールの受信箱 1 つにつき 1 つまでです。同じ受信箱のエイリアスや +タグ付きのアドレスも同じものとみなします。プランの上限を回避するために、複数作らないでください。",
  ],
  [
    "Use an e-mail address you read, and verify it when we ask. An account still unverified after 14 days cannot add devices until it is verified.",
    "请使用你常看的邮箱，并在我们要求时完成验证。注册 14 天后仍未验证的账号，在验证之前无法添加设备。",
    "請使用你常看的信箱，並在我們要求時完成驗證。註冊 14 天後仍未驗證的帳號，在驗證之前無法新增裝置。",
    "普段読んでいるメールアドレスを使い、確認を求められたら確認してください。14 日たっても確認されていないアカウントは、確認が済むまでデバイスを追加できません。",
  ],
  ["Keeping it safe", "保护好你的账号", "保護好你的帳號", "安全に使うために"],
  [
    "Keep your password, passkeys, recovery words and devices safe: anyone who can use one of your devices can act as you through it, on your devices that run AI too.",
    "保管好你的密码、通行密钥、恢复词和设备：谁能用你的任何一台设备，谁就能借它以你的身份操作，包括操作你那些可运行 AI 的设备。",
    "保管好你的密碼、通行金鑰、復原詞和裝置：誰能用你的任何一台裝置，誰就能藉它以你的身分操作，包括操作你那些可執行 AI 的裝置。",
    "パスワード、パスキー、リカバリーワード、デバイスを安全に保管してください。あなたのデバイスを使える人は、それを通じてあなたとして操作できます。AI を実行できるデバイスに対してもです。",
  ],
  [
    "Only your devices and your recovery words can decrypt your data. If you lose them all, nobody can decrypt it again, not even us.",
    "只有你的设备和恢复词能解密你的数据。全部丢失后，谁都无法再解密，包括我们。",
    "只有你的裝置和復原詞能解密你的資料。全部遺失後，誰都無法再解密，包括我們。",
    "データを復号できるのは、あなたのデバイスとリカバリーワードだけです。すべて失うと、私たちを含め誰も復号できなくなります。",
  ],

  // Your devices
  ["Your devices", "你的设备", "你的裝置", "あなたのデバイス"],
  ["Your devices, your AI tools", "你的设备，你的 AI 工具", "你的裝置，你的 AI 工具", "あなたのデバイス、あなたの AI ツール"],
  ["You are responsible for", "由你负责的", "由你負責的", "あなたの責任となるもの"],
  [
    "The devices you connect: connect only devices you own or are allowed to use, and keep them secure.",
    "你连接的设备：只连接你拥有或有权使用的设备，并保证它们的安全。",
    "你連接的裝置：只連接你擁有或有權使用的裝置，並確保它們的安全。",
    "接続するデバイス。自分が所有しているか、使う権限のあるデバイスだけを接続し、安全に保ってください。",
  ],
  [
    "The AI tools you run through Droppen, the accounts you use them with, and keeping to their providers' terms.",
    "你通过 Droppen 运行的 AI 工具、你用于这些工具的账号，以及遵守其提供方的条款。",
    "你透過 Droppen 執行的 AI 工具、你用於這些工具的帳號，以及遵守其提供者的條款。",
    "Droppen 経由で動かす AI ツールと、そのツールで使うアカウント。そして、その提供元の規約を守ること。",
  ],
  [
    "What those tools do on your devices: the code they write and the commands they run. Check their work as you would anyone's.",
    "这些工具在你设备上做的事：它们写的代码、运行的命令。像检查任何人的工作一样，检查它们的产出。",
    "這些工具在你裝置上做的事：它們寫的程式碼、執行的指令。像檢查任何人的工作一樣，檢查它們的產出。",
    "それらのツールがデバイス上で行うこと。書いたコードや実行したコマンドです。誰の作業でもそうするように、内容を確認してください。",
  ],
  ["What we don't do", "我们不做的", "我們不做的", "私たちが行わないこと"],
  [
    "We don't provide AI tools, models, or accounts with their providers: Droppen connects you to the ones on your devices.",
    "我们不提供 AI 工具、模型，也不提供这些服务商的账号：Droppen 只是把你连到你设备上已有的那些。",
    "我們不提供 AI 工具、模型，也不提供這些服務商的帳號：Droppen 只是把你連到你裝置上已有的那些。",
    "AI ツールやモデル、その提供元のアカウントは提供しません。Droppen は、あなたのデバイスにあるものにつなぐだけです。",
  ],
  [
    "We don't back up your work. The encrypted copy in the cloud lets you read recent sessions while a device is off; a device off for longer than your plan allows has that copy deleted. Keep your own copies of what matters.",
    "我们不替你备份。云端的加密副本只是让你在设备关机时能看最近的会话；设备关机超过你的方案允许的天数，这份副本就会被删除。重要的东西请自己留好备份。",
    "我們不替你備份。雲端的加密副本只是讓你在裝置關機時能看最近的工作階段；裝置關機超過你的方案允許的天數，這份副本就會被刪除。重要的東西請自己留好備份。",
    "作業内容のバックアップは行いません。クラウドの暗号化コピーは、デバイスの電源が切れている間に最近のセッションを読むためのもので、プランで決まった日数より長く電源が切れたデバイスは、そのコピーが削除されます。大切なものは自分でコピーを残してください。",
  ],

  // Acceptable use
  ["Acceptable use", "使用规范", "使用規範", "利用ルール"],
  ["Use it fairly", "合理使用", "合理使用", "節度を持って使う"],
  ["Don't", "请不要", "請不要", "禁止事項"],
  [
    "Use Droppen for anything illegal, or to harm other people or their systems.",
    "用 Droppen 做任何违法的事，或伤害他人及其系统。",
    "用 Droppen 做任何違法的事，或傷害他人及其系統。",
    "違法な目的や、他人やそのシステムに害を与える目的で Droppen を使うこと。",
  ],
  [
    "Attack the service: get into accounts that are not yours, read or change other people's data, or overload our servers. No load or denial-of-service testing.",
    "攻击这项服务：进入不属于你的账号，读取或篡改他人的数据，或让我们的服务器过载。也不要做压力测试或拒绝服务测试。",
    "攻擊這項服務：進入不屬於你的帳號，讀取或竄改他人的資料，或讓我們的伺服器超載。也不要做壓力測試或阻斷服務測試。",
    "サービスを攻撃すること。自分のものではないアカウントに入る、他人のデータを読む・変える、私たちのサーバーに過負荷をかける、などです。負荷テストやサービス妨害（DoS）のテストもお断りします。",
  ],
  [
    "Use our servers as a general-purpose tunnel, proxy or VPN, or as file storage: they are there for your Droppen sessions.",
    "把我们的服务器当作通用隧道、代理、VPN 或网盘：它们是为你的 Droppen 会话准备的。",
    "把我們的伺服器當作通用隧道、代理、VPN 或雲端硬碟：它們是為你的 Droppen 工作階段準備的。",
    "私たちのサーバーを、汎用のトンネル、プロキシ、VPN やファイル置き場として使うこと。サーバーは Droppen のセッションのためのものです。",
  ],
  [
    "Get around a plan's limits, for example with extra accounts or automated sign-ups.",
    "绕过方案的限制，比如多开账号或自动批量注册。",
    "繞過方案的限制，例如多開帳號或自動批量註冊。",
    "複数のアカウントや自動登録などで、プランの上限を回避すること。",
  ],
  ["Security research", "安全研究", "安全研究", "セキュリティ研究"],
  [
    "Found a vulnerability? Report it privately: to {{email}}, or through GitHub's private vulnerability reporting at github.com/getdroppen/droppen.",
    "发现了漏洞？请私下报告：写信到 {{email}}，或通过 github.com/getdroppen/droppen 的 GitHub 私密漏洞报告。",
    "發現了漏洞？請私下回報：寫信到 {{email}}，或透過 github.com/getdroppen/droppen 的 GitHub 私密漏洞回報。",
    "脆弱性を見つけたら、非公開で報告してください。{{email}} へのメール、または github.com/getdroppen/droppen の GitHub の非公開の脆弱性報告からお願いします。",
  ],
  [
    "When you test against droppen.app, use only accounts you own, don't access or change anyone else's data, and don't slow the service down for others.",
    "在 droppen.app 上测试时，只用你自己的账号，不要访问或改动他人的数据，也不要拖慢其他人的服务。",
    "在 droppen.app 上測試時，只用你自己的帳號，不要存取或改動他人的資料，也不要拖慢其他人的服務。",
    "droppen.app に対してテストするときは、自分のアカウントだけを使い、他人のデータにアクセスしたり変更したりせず、他の人のサービスを遅くしないでください。",
  ],

  // Plans
  ["Plans", "方案", "方案", "プラン"],
  ["Plans, and how they may change", "方案，以及它们可能怎么变", "方案，以及它們可能怎麼變", "プランと、その変更"],
  ["Today", "现在的方案", "現在的方案", "現在のプラン"],
  [
    "The Free plan includes {{hosts}} devices that run AI, {{devices}} devices in all, the last {{days}} days readable while a device is off, and {{gb}} GB a month through our servers, then slower.",
    "免费版包含：{{hosts}} 台可运行 AI 的设备，设备总共 {{devices}} 台，设备关机时能看最近 {{days}} 天的会话，每月经我们服务器转发 {{gb}} GB，超出后限速。",
    "免費版包含：{{hosts}} 台可執行 AI 的裝置，裝置總共 {{devices}} 台，裝置關機時能看最近 {{days}} 天的工作階段，每月經我們伺服器轉送 {{gb}} GB，超出後限速。",
    "無料プランに含まれるのは、AI を実行できるデバイス {{hosts}} 台、デバイスは全部で {{devices}} 台、デバイスの電源が切れていても直近 {{days}} 日分のセッションを読めること、私たちのサーバー経由の転送は月 {{gb}} GB まで（超えると低速に）です。",
  ],
  [
    "Pro is not on sale yet. Nothing is charged unless you choose to pay, and you see the price and how billing works before you do.",
    "Pro 还没开卖。除非你自己选择付费，否则不会产生任何费用；付费之前，你会先看到价格和计费方式。",
    "Pro 還沒開賣。除非你自己選擇付費，否則不會產生任何費用；付費之前，你會先看到價格和計費方式。",
    "Pro はまだ販売していません。あなたが支払うと決めない限り料金は発生せず、支払う前に価格と請求の仕組みを確認できます。",
  ],
  ["Changes", "调整", "調整", "変更について"],
  [
    "We may change plans, their limits and their prices. The pricing page always shows the current ones.",
    "我们可能调整方案、方案的限制和价格。定价页上始终是当前的方案。",
    "我們可能調整方案、方案的限制和價格。價格頁上始終是目前的方案。",
    "プラン、その上限、価格は変更することがあります。最新の内容は、いつでも料金ページで確認できます。",
  ],
  [
    "If a change takes away something your plan includes, we tell you by e-mail before it applies to you.",
    "如果调整会拿走你的方案里已有的东西，我们会在它对你生效之前发邮件告诉你。",
    "如果調整會拿走你的方案裡已有的東西，我們會在它對你生效之前寄信告訴你。",
    "あなたのプランに含まれるものが減る変更は、あなたに適用される前にメールでお知らせします。",
  ],
  [
    "A change of plan never deletes your account or its keys. On a smaller plan the cloud keeps fewer days of copies, and what goes beyond its limits is refused.",
    "换方案永远不会删除你的账号或它的密钥。换到更小的方案后，云端保留副本的天数会变少，超出限制的部分会被拒绝。",
    "換方案永遠不會刪除你的帳號或它的金鑰。換到更小的方案後，雲端保留副本的天數會變少，超出限制的部分會被拒絕。",
    "プランが変わっても、アカウントやその鍵が削除されることはありません。小さいプランでは、クラウドがコピーを保持する日数が短くなり、上限を超える分は受け付けられません。",
  ],
  ["See the plans", "查看各个方案", "查看各個方案", "プランを見る"],

  // Availability
  ["Availability", "可用性", "可用性", "提供状況"],
  ["In beta, provided as is", "测试版，按现状提供", "測試版，依現狀提供", "ベータ版、現状のままでの提供"],
  ["What to expect", "请你知悉", "請你知悉", "ご理解いただきたいこと"],
  [
    "Droppen is in beta. We work to keep it running well, but it is provided “as is” and “as available”: there is no service-level agreement, and no promise that it is always up, fast or free of bugs.",
    "Droppen 目前是测试版。我们会尽力让它稳定运行，但它按「现状」和「可用状态」提供：没有服务等级协议（SLA），也不保证它始终可用、速度快或没有缺陷。",
    "Droppen 目前是測試版。我們會盡力讓它穩定運作，但它依「現狀」及「可用狀態」提供：沒有服務等級協議（SLA），也不保證它始終可用、速度快或沒有缺陷。",
    "Droppen はベータ版です。安定して動くよう努めていますが、「現状のまま」「提供可能な範囲で」の提供となります。サービスレベル契約（SLA）はなく、常に使えること、速いこと、不具合がないことは保証しません。",
  ],
  [
    "Features marked as in testing, such as renewing the identity key of a device that runs AI, may change or be withdrawn.",
    "标为「测试中」的功能，比如更换可运行 AI 的设备的身份密钥，可能会改变或被撤下。",
    "標為「測試中」的功能，例如更換可執行 AI 的裝置的身分金鑰，可能會改變或被撤下。",
    "「テスト中」と表示された機能（AI を実行できるデバイスの ID 鍵の更新など）は、変更されたり提供が中止されたりすることがあります。",
  ],
  [
    "We may change or stop parts of the service. If we ever shut it down, we tell you by e-mail first, and Droppen on your own devices keeps working locally without us.",
    "我们可能调整或停掉服务的某些部分。如果哪天要关闭整个服务，我们会先发邮件通知你；你设备上的 Droppen 不依赖我们，照样能在本地使用。",
    "我們可能調整或停掉服務的某些部分。如果哪天要關閉整個服務，我們會先寄信通知你；你裝置上的 Droppen 不依賴我們，照樣能在本地使用。",
    "サービスの一部を変更・停止することがあります。サービス全体を終了する場合は、事前にメールでお知らせします。あなたのデバイス上の Droppen は、私たちがいなくてもローカルで使い続けられます。",
  ],
  ["Liability", "责任限制", "責任限制", "責任の制限"],
  [
    "As far as the law allows, we are not liable for indirect or consequential losses, lost profits or lost data, or for what your devices and AI tools do.",
    "在法律允许的范围内，我们不对间接或附带损失、利润损失、数据丢失负责，也不对你的设备和 AI 工具所做的事负责。",
    "在法律允許的範圍內，我們不對間接或附帶損失、利潤損失、資料遺失負責，也不對你的裝置和 AI 工具所做的事負責。",
    "法律で認められる範囲で、私たちは間接損害・派生的損害、逸失利益、データの消失について、またあなたのデバイスや AI ツールが行ったことについて、責任を負いません。",
  ],
  [
    "As far as the law allows, our total liability to you is limited to what you paid us for the service in the 12 months before the claim.",
    "在法律允许的范围内，我们对你承担的全部责任，以你在索赔前 12 个月内为这项服务向我们支付的金额为限。",
    "在法律允許的範圍內，我們對你承擔的全部責任，以你在求償前 12 個月內為這項服務向我們支付的金額為限。",
    "法律で認められる範囲で、私たちがあなたに負う責任の総額は、請求の前 12 か月間にあなたがこのサービスに支払った金額を上限とします。",
  ],
  [
    "Nothing in these terms limits a liability the law does not let us limit, or takes away your rights as a consumer.",
    "本条款不会限制法律不允许限制的责任，也不会剥夺你作为消费者的权利。",
    "本條款不會限制法律不允許限制的責任，也不會剝奪你作為消費者的權利。",
    "この規約は、法律上制限できない責任を制限するものではなく、消費者としてのあなたの権利を奪うものでもありません。",
  ],

  // Ending
  ["Ending", "终止", "終止", "利用の終了"],
  ["Leaving, and deleting your account", "离开，以及删除账号", "離開，以及刪除帳號", "退会とアカウントの削除"],
  ["If you leave", "如果你要离开", "如果你要離開", "退会するとき"],
  [
    "You can stop using droppen.app at any time, and delete your account yourself, in its settings: it is gone at once, with its devices and what our servers keep for it (the privacy notice says what stays for a short while).",
    "你随时可以停止使用 droppen.app，也可以在账号设置里自己删除账号：账号会立即删除，连同它的设备和我们的服务器为它保存的内容（短期保留的少量记录见隐私说明）。",
    "你隨時可以停止使用 droppen.app，也可以在帳號設定裡自己刪除帳號：帳號會立即刪除，連同它的裝置和我們的伺服器為它保存的內容（短期保留的少量紀錄見隱私權聲明）。",
    "droppen.app はいつでも利用をやめられ、アカウントは設定から自分で削除できます。アカウントはすぐに削除され、そのデバイスや、私たちのサーバーがそのアカウントのために保管しているものも消えます（短期間だけ残る記録は「プライバシーについて」に記載しています）。",
  ],
  [
    "Deleting the account does not touch your own devices: what is on them stays until you remove it.",
    "删除账号不会动你自己的设备：设备上的东西会一直留着，直到你自己删除。",
    "刪除帳號不會動你自己的裝置：裝置上的東西會一直留著，直到你自己刪除。",
    "アカウントを削除しても、あなたのデバイスには影響しません。デバイス上のものは、あなたが消すまで残ります。",
  ],
  ["If we suspend it", "如果我们暂停你的账号", "如果我們暫停你的帳號", "アカウントの停止"],
  [
    "We may suspend or close an account that breaks these terms or puts the service or other people at risk. Where we can, we tell you first, say why, and give you a chance to put it right.",
    "违反本条款、或给服务及他人带来风险的账号，我们可能暂停或关闭。只要条件允许，我们会先通知你、说明原因，并给你改正的机会。",
    "違反本條款、或給服務及他人帶來風險的帳號，我們可能暫停或關閉。只要條件允許，我們會先通知你、說明原因，並給你改正的機會。",
    "この規約に違反したアカウントや、サービスや他の人を危険にさらすアカウントは、停止または閉鎖することがあります。可能な限り、事前にお知らせして理由を説明し、改める機会を設けます。",
  ],
  [
    "We can't read your sessions, so we judge by what our servers can see: how much an account sends, when, and how.",
    "我们读不了你的会话，所以只能根据服务器看得到的东西来判断：一个账号传了多少、什么时候传、怎么传。",
    "我們讀不了你的工作階段，所以只能根據伺服器看得到的東西來判斷：一個帳號傳了多少、什麼時候傳、怎麼傳。",
    "私たちはセッションを読めないため、サーバーから見えるもの、つまりアカウントがどれだけ、いつ、どのように送っているかで判断します。",
  ],

  // Legal
  ["Legal", "法律事项", "法律事項", "法的事項"],
  ["Changes, the law, and who we are", "条款变更、适用法律，以及我们是谁", "條款變更、準據法，以及我們是誰", "規約の変更、準拠法、運営者"],
  ["Changes to these terms", "条款变更", "條款變更", "規約の変更"],
  [
    "We may update these terms. When a change matters, we tell you by e-mail before it takes effect, and the date at the top changes.",
    "我们可能更新本条款。重要的变更会在生效前发邮件告诉你，页首的日期也会随之更新。",
    "我們可能更新本條款。重要的變更會在生效前寄信告訴你，頁首的日期也會隨之更新。",
    "この規約は更新することがあります。重要な変更は、発効する前にメールでお知らせし、ページ上部の日付も更新します。",
  ],
  [
    "If you keep using droppen.app after that, the new terms apply. If you don't accept them, you can delete your account.",
    "之后继续使用 droppen.app，即适用新条款。如果不接受，你可以删除账号。",
    "之後繼續使用 droppen.app，即適用新條款。如果不接受，你可以刪除帳號。",
    "その後も droppen.app を使い続けると、新しい規約が適用されます。同意できない場合は、アカウントを削除できます。",
  ],
  ["Who we are", "我们是谁", "我們是誰", "運営者"],
  [
    "Droppen operates droppen.app and the service on it.",
    "droppen.app 及其上的服务由 Droppen 运营。",
    "droppen.app 及其上的服務由 Droppen 營運。",
    "droppen.app とそのサービスは Droppen が運営しています。",
  ],
  [
    "These terms are governed by the laws of Singapore, and the courts of Singapore have jurisdiction over any dispute about them. If you are a consumer, you keep the protection of the law where you live.",
    "本条款受新加坡法律管辖，与本条款有关的任何争议由新加坡法院管辖。如果你是消费者，你所在地法律给你的保护不受影响。",
    "本條款以新加坡法律為準據法，與本條款有關的任何爭議由新加坡法院管轄。如果你是消費者，你所在地法律給你的保護不受影響。",
    "この規約はシンガポールの法律に準拠し、規約に関するすべての紛争はシンガポールの裁判所が管轄します。消費者の方は、お住まいの地域の法律による保護を引き続き受けられます。",
  ],

  // Contact
  [
    "Questions about these terms, or a request about your account:",
    "对本条款有疑问，或有关于账号的请求，请写信到：",
    "對本條款有疑問，或有關於帳號的請求，請寫信到：",
    "この規約についてのご質問や、アカウントに関するご依頼は：",
  ],
  ["Security issues:", "安全问题：", "安全問題：", "セキュリティの問題は："],
  [
    "What we keep, and what we can't see: the privacy notice",
    "我们保存什么，又看不到什么：隐私说明",
    "我們保存什麼，又看不到什麼：隱私權聲明",
    "私たちが保管するもの、見えないもの：プライバシーについて",
  ],
];

const column = (i: 1 | 2 | 3): Bundle => Object.fromEntries(LINES.map((line) => [line[0], line[i]]));

export const termsZhCN: Bundle = column(1);
export const termsZhTW: Bundle = column(2);
export const termsJa: Bundle = column(3);

/** The page's tables, by language. */
export const termsBundles: Record<"zh-CN" | "zh-TW" | "ja", Bundle> = {
  "zh-CN": termsZhCN,
  "zh-TW": termsZhTW,
  ja: termsJa,
};
