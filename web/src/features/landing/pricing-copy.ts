/*
 * The words of the pricing page (`pricing.tsx`), its waitlist form
 * (`waitlist.tsx`), the privacy notice (`privacy.tsx`) and the public pages'
 * links to them (`page-kit.tsx`) — merged into the landing page's tables
 * (`copy.ts`), so the landing test holds them to the same rules: keys are the
 * English text, zh-CN is written as native copy first, zh-TW and ja say the
 * same things.
 *
 * The voice is a product manager's: what the reader gets first (their AI on
 * their phone, more devices, longer offline reading), then the terms. These
 * pages are the product's own words, so `test/ui-wording.test.ts` holds them
 * like the download page: a device is a 设备 / 裝置 / デバイス, the one the
 * page runs on 「这台设备」 / 「這台裝置」 / 「このデバイス」, never a computer
 * or a machine; no "relay", "host" or "entitlement" — "our servers", "devices
 * that run AI". Every number is interpolated from `pricing-plans.ts`, which
 * a test holds to the hub's plans; none is written into a sentence here.
 */

type Bundle = Record<string, string>;

export const pricingZhCN: Bundle = {
  // The public pages' links
  "Pricing": "定价",
  "Privacy notice": "隐私说明",
  "How the encryption works, in detail": "加密是怎么做的：详细说明",
  "No code yet? Join the waitlist": "还没有邀请码？加入等候名单",

  // Page
  "Droppen pricing — start free, Pro for ${{price}} a month": "Droppen 定价 — 免费起步，Pro 每月 ${{price}}",
  "Use Droppen on one device with no account, free with an invite, or Pro for more devices and longer offline reading. Every plan is end-to-end encrypted.":
    "一台设备不用账号就能用；凭邀请码免费用；想要更多设备、更长的离线可读时间就选 Pro。每个方案都端到端加密。",
  "Start free. Go Pro when you need more.": "先免费用起来，不够再升 Pro。",
  "Every plan is end-to-end encrypted: we can't see your code on any of them. A plan only decides how many devices you connect, how far back your phone can read while a device is off, and how much our servers carry for you each month.":
    "每个方案都端到端加密，哪个方案我们都看不到你的代码。方案只决定三件事：能连几台设备，设备关机时手机能往回看多久，以及每月我们的服务器替你转发多少流量。",
  "Prices in US dollars.": "价格以美元计。",

  // The plans
  "End-to-end encrypted: we can't see your code": "端到端加密：我们看不到你的代码",
  "Local only": "仅本地",
  "forever": "永久免费",
  "No account, nothing to sign up for": "无需账号，也不用注册",
  "Droppen on the device your AI runs on, and nothing in the cloud.": "Droppen 只装在跑 AI 的那台设备上，云端什么都不放。",
  "1 device that runs AI: this one": "1 台可运行 AI 的设备：就是这台",
  "Use it from any browser on the same network": "同一网络里的任何浏览器都能用",
  "Nothing goes through our servers": "不经过我们的服务器",
  "Free": "免费版",
  "USD / month": "美元 / 月",
  "Sign-up is by invite for now": "目前凭邀请码注册",
  "Join the waitlist": "加入等候名单",
  "You have an account already.": "你已经有账号了。",
  "Have an invite code? Sign up": "有邀请码？去注册",
  "Your AI on your phone, wherever you are.": "不管人在哪，手机上都能接着用 AI。",
  "{{n}} devices that run AI": "{{n}} 台可运行 AI 的设备",
  "{{n}} devices in all, phones and browsers included": "设备总共 {{n}} 台，手机、浏览器都算在内",
  "Read the last {{n}} days on your phone while a device is off": "设备关机时，手机上能看最近 {{n}} 天的会话",
  "{{n}} GB a month through our servers, then slower": "每月经我们服务器转发 {{n}} GB，超出后限速",
  "Waitlist": "等候名单",
  "or ${{price}} a year: save {{saving}}%": "或每年 ${{price}}，省 {{saving}}%",
  "Payments are not open yet: we'll e-mail you when they are.": "目前还不能付款，开放时我们会发邮件告诉你。",
  "Everything in Free, plus:": "免费版的全部，再加上：",
  "{{n}} GB a month through our servers, then billed by use, at full speed": "每月经我们服务器转发 {{n}} GB，超出部分按量计费，不限速",

  // Compare
  "Compare": "对比",
  "What each plan includes": "每个方案包含什么",
  "included": "包含",
  "not included": "不包含",
  "Price": "价格",
  "Free forever, no account": "永久免费，无需账号",
  "Free; sign-up by invite for now": "免费；目前凭邀请码注册",
  "${{monthly}} a month, or ${{yearly}} a year": "每月 ${{monthly}}，或每年 ${{yearly}}",
  "Devices that run AI": "可运行 AI 的设备",
  "1: this device": "1 台：这台设备",
  "All your devices, phones and browsers included": "设备总数（含手机、浏览器）",
  "This device, and browsers on the same network": "这台设备，以及同一网络里的浏览器",
  "How far back your phone can read while a device is off": "设备关机时，手机能往回看多久",
  "{{n}} days": "最近 {{n}} 天",
  "A month's transfer through our servers": "每月经我们服务器转发的流量",
  "{{n}} GB, then slower": "{{n}} GB，超出后限速",
  "{{n}} GB, then billed by use": "{{n}} GB，超出部分按量计费",
  "A device off for longer than that has its copy in the cloud deleted: what is on the device stays, and its last {{free}} days ({{pro}} on Pro) come back once it is online.":
    "设备关机超过这个天数，它在云端的副本会被删除：设备上的记录不受影响，重新上线后会自动补回最近 {{free}} 天（Pro 为 {{pro}} 天）。",

  // Waitlist
  "Leave your address, and we'll write when it's your turn.": "留下邮箱，轮到你时我们写信给你。",
  "Sign-up is invite-only while Droppen is in beta, and Pro is not on sale yet. Pick what you are waiting for: we'll e-mail you an invite, or tell you when Pro opens. Submitting the form is all it takes.":
    "Droppen 还是邀请制测试版，注册需要邀请码，Pro 也还没开卖。选好你在等哪一个：我们会把邀请发到你的邮箱，或在 Pro 开放时通知你。提交表单就算加入，不用再确认。",
  "Plan": "方案",
  "${{price}} / month": "${{price}} / 月",
  "E-mail address": "邮箱地址",
  "No confirmation mail and no newsletter: we use your address only to send what you asked for.": "没有确认邮件，也没有新闻简报：你的邮箱只用来发你要的那一封。",
  "That does not look like an e-mail address. Check it and try again.": "这看起来不像邮箱地址，检查一下再试。",
  "Use an address you keep: throwaway mail providers are not accepted.": "请用你长期使用的邮箱：不接受一次性邮箱。",
  "Too many tries from your network. Try again in {{n}} min.": "你所在的网络尝试次数太多，请 {{n}} 分钟后再试。",
  "Could not reach Droppen. Check your connection and try again.": "连不上 Droppen，检查网络后再试。",
  "You're on the list: we'll e-mail you an invite when Pro opens.": "你已在名单上，Pro 开放时我们会把邀请发到你的邮箱。",
  "You're on the list: we'll e-mail you an invite.": "你已在名单上，我们会把邀请发到你的邮箱。",
  "We'll send nothing else. To be taken off the list, write to {{email}}.": "除此之外我们不会发任何邮件。想退出名单，请写信到 {{email}}。",
  "Add another address": "再添加一个邮箱",

  // Questions
  "Questions": "常见问题",
  "Before you choose": "选之前，你可能想知道",
  "What happens when I use up a month's transfer?": "每月的流量用完了会怎样？",
  "Only what goes through our servers counts: what goes straight between your devices does not. On Free, past {{free}} GB that traffic slows to {{kb}} KB/s until the next month starts; terminals and reading still work, just more slowly. On Pro nothing slows down: what you use past {{pro}} GB is billed by use, at a price we will publish before payments open.":
    "只有经我们服务器转发的流量才计入，设备之间直连的不算。免费版超过 {{free}} GB 后，这部分流量会限速到 {{kb}} KB/s，直到下个月开始；终端和阅读照常能用，只是慢一些。Pro 不限速：超过 {{pro}} GB 的部分按量计费，单价会在开放付款前公布。",
  "What does “local only” mean?": "「仅本地」是什么意思？",
  "Droppen runs on your device with no account. You use it in a browser on that device, or on another device on the same network, and nothing goes through our servers. Sign in to Droppen when you want your AI on your phone from anywhere, and all your devices on one screen.":
    "Droppen 跑在你的设备上，不需要账号。在这台设备或同一网络里的其他设备上用浏览器打开就能用，什么都不经过我们的服务器。想在外面用手机接着用 AI、把所有设备放进一个屏幕时，再登录 Droppen。",
  "Can I host it myself?": "可以自己部署吗？",
  "The Droppen app and server are licensed under the AGPL-3.0, and local only needs nothing from us. Their source code is at github.com/getdroppen/droppen. Droppen Hub, the service at droppen.app that connects your devices, is not open source yet, so it cannot be self-hosted for now.":
    "Droppen 应用和服务端以 AGPL-3.0 授权，「仅本地」用法完全不依赖我们。源代码在 github.com/getdroppen/droppen。连接你各台设备的 Droppen Hub（也就是 droppen.app 背后的服务）暂未开源，所以目前还不能自己部署。",
  "When can I pay for Pro?": "什么时候能付费开通 Pro？",
  "Not yet: payments are on their way. Until then Pro is by waitlist: join it and we'll e-mail you when Pro opens. Nothing is charged before you choose to pay, and prices are in US dollars.":
    "还不行：付款功能正在准备中。在那之前，Pro 只能先加入等候名单，开放时我们会发邮件通知你。在你自己决定付款之前，不会产生任何费用；价格以美元计。",
  "What if a device stays off for longer?": "设备关机更久会怎样？",
  "On Free your phone can read a device's last {{free}} days while it is off, on Pro its last {{pro}}. A device off for longer than that has its encrypted copy in the cloud deleted: what is on the device itself is not affected, and its recent days are uploaded again once it is back online.":
    "设备关机时，免费版的手机上能看它最近 {{free}} 天的会话，Pro 是 {{pro}} 天。关机超过这个天数，它在云端的加密副本会被删除：设备上的记录不受影响，重新上线后会自动再上传最近的会话。",
  "Is my code safer on Pro?": "用 Pro，代码会更安全吗？",
  "It is just as safe on every plan: all of them are end-to-end encrypted the same way, and we can't read your sessions, your code or your terminals on any. A plan only changes how much we keep and carry for you, never who can read it.":
    "每个方案都一样安全：全部以同样的方式端到端加密，哪个方案我们都读不到你的会话、代码和终端。方案只改变我们替你保存和转发多少，从不改变谁能读。",

  // Privacy notice
  "Privacy notice — Droppen": "隐私说明 — Droppen",
  "What we keep, and what we can't see.": "我们保存什么，又看不到什么。",
  "Droppen is end-to-end encrypted, so there is little to tell: here is all of it.": "Droppen 是端到端加密的，所以要说明的不多，全在这里了。",
  "Last updated {{date}}": "最后更新：{{date}}",
  "If you joined the waitlist": "如果你加入了等候名单",
  "What we keep, and why": "我们保存什么、为什么",
  "Your e-mail address, the plan you chose (Free or Pro), the language of the page and when you joined.": "你的邮箱地址、你选的方案（免费版或 Pro）、页面语言，以及加入的时间。",
  "We use them only to e-mail you an invite, or to tell you when Pro opens. Nothing else is sent, and your address is never sold or used for marketing.":
    "只用来给你发邀请，或在 Pro 开放时通知你。不会发别的邮件，你的邮箱也绝不出售、不用于营销。",
  "Not your IP address, nor anything else about your browser. To stop abuse, our servers count requests per network in memory, for a day at most.":
    "不保存你的 IP 地址，也不保存浏览器的任何其他信息。为防止滥用，服务器会在内存里按网络计数请求，最多保留一天。",
  "How long, and how to be taken off": "保存多久、怎么退出",
  "Until you ask us to remove it, and never more than 12 months after you last joined: older entries are deleted automatically.":
    "保存到你要求删除为止，最长不超过你最后一次加入后的 12 个月：超过的记录会自动删除。",
  "To be taken off, write to {{email}} from that address and we will delete it.": "想退出，用那个邮箱写信到 {{email}}，我们会删除它。",
  "Your account": "你的账号",
  "If you use Droppen": "如果你在用 Droppen",
  "Your sessions and terminals are encrypted on your devices, and only your devices hold the keys. Our servers pass and keep what they cannot read.":
    "你的会话和终端都在你的设备上加密，钥匙只在你的设备里。我们的服务器转发、保存的，都是它读不了的内容。",
  "What our servers can see": "我们的服务器看得到",
  "Your e-mail address, and when you sign in": "你的邮箱地址，以及登录时间",
  "Which of your devices are online, when they connect and from which IP address": "你的哪些设备在线、何时连接、从哪个 IP 地址",
  "What kind of device each one is and which browser, as a short name such as “iPhone · Chrome” made from its User-Agent; it does not keep the User-Agent itself":
    "每台设备的类型和浏览器，只记一个简称，比如「iPhone · Chrome」，由它的 User-Agent 得出；User-Agent 本身从不保存",
  "How much data moves and when. What we store is padded to fixed sizes; live connections are not, so while our servers carry one they see its timing, its volume and the rhythm of typing in a terminal: roughly when you are working": "传了多少数据、什么时候传的。我们保存的数据会填充到固定大小；实时连接不做填充，所以经我们的服务器转发时，它的时序、流量和终端里的打字节奏都看得到：也就是你大概什么时候在工作",
  "Your plan, and how much of it you use": "你的方案，以及用了多少",
  "What they cannot see": "我们的服务器看不到",
  "What is said in a session, or typed and shown in a terminal": "会话里说了什么，终端里输入和显示了什么",
  "Session titles, folders, repositories and branches": "会话标题、文件夹、仓库和分支",
  "Model names, which AI tool you use, your AI accounts and their quotas": "模型名、你用哪个 AI 工具、你的 AI 账号和额度",
  "The names of your devices, your files and their names": "设备名、你的文件以及文件名",
  "Contact": "联系我们",
  "Questions, or a request about your data:": "有疑问，或想处理你的数据，请写信到：",
  // Deleting an account (privacy notice)
  "Deleting your account":
    "删除账号",
  "If you delete your account":
    "如果你删除了账号",
  "What is deleted":
    "删除什么",
  "You can delete your account yourself, in its settings. It is gone at once and for good: nobody, us included, can bring it back.":
    "你可以在账号设置里自己删除账号。删除立即生效、无法撤销：谁都无法恢复，包括我们。",
  "Deleted with it: your devices, the devices that run AI you added, the encrypted copies of your sessions and terminals, your keys, your plan, your password and passkeys, and the records of verification mail sent to your address.":
    "一并删除的有：你的设备、你添加的可运行 AI 的设备、会话和终端的加密副本、你的钥匙、你的方案、密码和通行密钥，以及发往你邮箱的验证邮件记录。",
  "Your address also leaves the waitlist, if it was on it.":
    "如果你的邮箱在等候名单上，也会一并移出。",
  "What we keep, and for how long":
    "我们保留什么、保留多久",
  "A keyed hash of your e-mail address, never the address itself, for {{days}} days, so the same inbox cannot sign up again straight away. How long is a setting of Droppen Hub: {{days}} days on droppen.app.":
    "你邮箱地址的带密钥哈希（不是地址本身），保留 {{days}} 天，防止同一个邮箱马上重新注册。保留多久由 Droppen Hub 的设置决定：droppen.app 上是 {{days}} 天。",
  "For the same {{days}} days, your account's and your devices' random ids and your devices' public keys, which open nothing: our servers use them to turn the account's connections away, and to tell a device that comes back that the account is gone, so it forgets its keys.":
    "同样保留 {{days}} 天：你账号和各个设备的随机 ID，以及设备的公钥（打不开任何东西）。我们的服务器用它们拒绝这个账号的连接，并告诉再次连上的设备账号已删除，让它忘掉自己的钥匙。",
  "Deleting removes all this from our live systems at once. Copies in our backups and logs expire after {{period}}.":
    "删除会立即把这些从我们的在线系统中移除。备份和日志中的副本会在 {{period}} 后过期。",
};

export const pricingZhTW: Bundle = {
  "Pricing": "價格",
  "Privacy notice": "隱私權聲明",
  "How the encryption works, in detail": "加密是怎麼做的：詳細說明",
  "No code yet? Join the waitlist": "還沒有邀請碼？加入等候名單",

  "Droppen pricing — start free, Pro for ${{price}} a month": "Droppen 價格 — 免費起步，Pro 每月 ${{price}}",
  "Use Droppen on one device with no account, free with an invite, or Pro for more devices and longer offline reading. Every plan is end-to-end encrypted.":
    "一台裝置不用帳號就能用；憑邀請碼免費用；想要更多裝置、更長的離線可讀時間就選 Pro。每個方案都端對端加密。",
  "Start free. Go Pro when you need more.": "先免費用起來，不夠再升 Pro。",
  "Every plan is end-to-end encrypted: we can't see your code on any of them. A plan only decides how many devices you connect, how far back your phone can read while a device is off, and how much our servers carry for you each month.":
    "每個方案都端對端加密，哪個方案我們都看不到你的程式碼。方案只決定三件事：能連幾台裝置，裝置關機時手機能往回看多久，以及每月我們的伺服器替你轉送多少流量。",
  "Prices in US dollars.": "價格以美元計。",

  "End-to-end encrypted: we can't see your code": "端對端加密：我們看不到你的程式碼",
  "Local only": "僅本地",
  "forever": "永久免費",
  "No account, nothing to sign up for": "不需要帳號，也不用註冊",
  "Droppen on the device your AI runs on, and nothing in the cloud.": "Droppen 只裝在跑 AI 的那台裝置上，雲端什麼都不放。",
  "1 device that runs AI: this one": "1 台可執行 AI 的裝置：就是這台",
  "Use it from any browser on the same network": "同一網路裡的任何瀏覽器都能用",
  "Nothing goes through our servers": "不經過我們的伺服器",
  "Free": "免費版",
  "USD / month": "美元 / 月",
  "Sign-up is by invite for now": "目前憑邀請碼註冊",
  "Join the waitlist": "加入等候名單",
  "You have an account already.": "你已經有帳號了。",
  "Have an invite code? Sign up": "有邀請碼？去註冊",
  "Your AI on your phone, wherever you are.": "不管人在哪，手機上都能接著用 AI。",
  "{{n}} devices that run AI": "{{n}} 台可執行 AI 的裝置",
  "{{n}} devices in all, phones and browsers included": "裝置總共 {{n}} 台，手機、瀏覽器都算在內",
  "Read the last {{n}} days on your phone while a device is off": "裝置關機時，手機上能看最近 {{n}} 天的工作階段",
  "{{n}} GB a month through our servers, then slower": "每月經我們伺服器轉送 {{n}} GB，超出後限速",
  "Waitlist": "等候名單",
  "or ${{price}} a year: save {{saving}}%": "或每年 ${{price}}，省 {{saving}}%",
  "Payments are not open yet: we'll e-mail you when they are.": "目前還不能付款，開放時我們會寄信通知你。",
  "Everything in Free, plus:": "免費版的全部，再加上：",
  "{{n}} GB a month through our servers, then billed by use, at full speed": "每月經我們伺服器轉送 {{n}} GB，超出部分按用量計費，不限速",

  "Compare": "比較",
  "What each plan includes": "每個方案包含什麼",
  "included": "包含",
  "not included": "不包含",
  "Price": "價格",
  "Free forever, no account": "永久免費，不需要帳號",
  "Free; sign-up by invite for now": "免費；目前憑邀請碼註冊",
  "${{monthly}} a month, or ${{yearly}} a year": "每月 ${{monthly}}，或每年 ${{yearly}}",
  "Devices that run AI": "可執行 AI 的裝置",
  "1: this device": "1 台：這台裝置",
  "All your devices, phones and browsers included": "裝置總數（含手機、瀏覽器）",
  "This device, and browsers on the same network": "這台裝置，以及同一網路裡的瀏覽器",
  "How far back your phone can read while a device is off": "裝置關機時，手機能往回看多久",
  "{{n}} days": "最近 {{n}} 天",
  "A month's transfer through our servers": "每月經我們伺服器轉送的流量",
  "{{n}} GB, then slower": "{{n}} GB，超出後限速",
  "{{n}} GB, then billed by use": "{{n}} GB，超出部分按用量計費",
  "A device off for longer than that has its copy in the cloud deleted: what is on the device stays, and its last {{free}} days ({{pro}} on Pro) come back once it is online.":
    "裝置關機超過這個天數，它在雲端的副本會被刪除：裝置上的紀錄不受影響，重新上線後會自動補回最近 {{free}} 天（Pro 為 {{pro}} 天）。",

  "Leave your address, and we'll write when it's your turn.": "留下電子郵件，輪到你時我們寫信給你。",
  "Sign-up is invite-only while Droppen is in beta, and Pro is not on sale yet. Pick what you are waiting for: we'll e-mail you an invite, or tell you when Pro opens. Submitting the form is all it takes.":
    "Droppen 還是邀請制測試版，註冊需要邀請碼，Pro 也還沒開賣。選好你在等哪一個：我們會把邀請寄到你的信箱，或在 Pro 開放時通知你。送出表單就算加入，不用再確認。",
  "Plan": "方案",
  "${{price}} / month": "${{price}} / 月",
  "E-mail address": "電子郵件地址",
  "No confirmation mail and no newsletter: we use your address only to send what you asked for.": "沒有確認信，也沒有電子報：你的信箱只用來寄你要的那一封。",
  "That does not look like an e-mail address. Check it and try again.": "這看起來不像電子郵件地址，檢查一下再試。",
  "Use an address you keep: throwaway mail providers are not accepted.": "請用你長期使用的信箱：不接受拋棄式信箱。",
  "Too many tries from your network. Try again in {{n}} min.": "你所在的網路嘗試次數太多，請 {{n}} 分鐘後再試。",
  "Could not reach Droppen. Check your connection and try again.": "連不上 Droppen，檢查網路後再試。",
  "You're on the list: we'll e-mail you an invite when Pro opens.": "你已在名單上，Pro 開放時我們會把邀請寄到你的信箱。",
  "You're on the list: we'll e-mail you an invite.": "你已在名單上，我們會把邀請寄到你的信箱。",
  "We'll send nothing else. To be taken off the list, write to {{email}}.": "除此之外我們不會寄任何信。想退出名單，請寫信到 {{email}}。",
  "Add another address": "再新增一個信箱",

  "Questions": "常見問題",
  "Before you choose": "選之前，你可能想知道",
  "What happens when I use up a month's transfer?": "每月的流量用完了會怎樣？",
  "Only what goes through our servers counts: what goes straight between your devices does not. On Free, past {{free}} GB that traffic slows to {{kb}} KB/s until the next month starts; terminals and reading still work, just more slowly. On Pro nothing slows down: what you use past {{pro}} GB is billed by use, at a price we will publish before payments open.":
    "只有經我們伺服器轉送的流量才計入，裝置之間直連的不算。免費版超過 {{free}} GB 後，這部分流量會限速到 {{kb}} KB/s，直到下個月開始；終端機和閱讀照常能用，只是慢一些。Pro 不限速：超過 {{pro}} GB 的部分按用量計費，單價會在開放付款前公布。",
  "What does “local only” mean?": "「僅本地」是什麼意思？",
  "Droppen runs on your device with no account. You use it in a browser on that device, or on another device on the same network, and nothing goes through our servers. Sign in to Droppen when you want your AI on your phone from anywhere, and all your devices on one screen.":
    "Droppen 跑在你的裝置上，不需要帳號。在這台裝置或同一網路裡的其他裝置上用瀏覽器打開就能用，什麼都不經過我們的伺服器。想在外面用手機接著用 AI、把所有裝置放進一個畫面時，再登入 Droppen。",
  "Can I host it myself?": "可以自己架設嗎？",
  "The Droppen app and server are licensed under the AGPL-3.0, and local only needs nothing from us. Their source code is at github.com/getdroppen/droppen. Droppen Hub, the service at droppen.app that connects your devices, is not open source yet, so it cannot be self-hosted for now.":
    "Droppen 應用程式和伺服器以 AGPL-3.0 授權，「僅本地」用法完全不依賴我們。原始碼在 github.com/getdroppen/droppen。連接你各台裝置的 Droppen Hub（也就是 droppen.app 背後的服務）尚未開源，所以目前還不能自己架設。",
  "When can I pay for Pro?": "什麼時候能付費開通 Pro？",
  "Not yet: payments are on their way. Until then Pro is by waitlist: join it and we'll e-mail you when Pro opens. Nothing is charged before you choose to pay, and prices are in US dollars.":
    "還不行：付款功能正在準備中。在那之前，Pro 只能先加入等候名單，開放時我們會寄信通知你。在你自己決定付款之前，不會產生任何費用；價格以美元計。",
  "What if a device stays off for longer?": "裝置關機更久會怎樣？",
  "On Free your phone can read a device's last {{free}} days while it is off, on Pro its last {{pro}}. A device off for longer than that has its encrypted copy in the cloud deleted: what is on the device itself is not affected, and its recent days are uploaded again once it is back online.":
    "裝置關機時，免費版的手機上能看它最近 {{free}} 天的工作階段，Pro 是 {{pro}} 天。關機超過這個天數，它在雲端的加密副本會被刪除：裝置上的紀錄不受影響，重新上線後會自動再上傳最近的工作階段。",
  "Is my code safer on Pro?": "用 Pro，程式碼會更安全嗎？",
  "It is just as safe on every plan: all of them are end-to-end encrypted the same way, and we can't read your sessions, your code or your terminals on any. A plan only changes how much we keep and carry for you, never who can read it.":
    "每個方案都一樣安全：全部以同樣的方式端對端加密，哪個方案我們都讀不到你的工作階段、程式碼和終端機。方案只改變我們替你保存和轉送多少，從不改變誰能讀。",

  "Privacy notice — Droppen": "隱私權聲明 — Droppen",
  "What we keep, and what we can't see.": "我們保存什麼，又看不到什麼。",
  "Droppen is end-to-end encrypted, so there is little to tell: here is all of it.": "Droppen 是端對端加密的，所以要說明的不多，全在這裡了。",
  "Last updated {{date}}": "最後更新：{{date}}",
  "If you joined the waitlist": "如果你加入了等候名單",
  "What we keep, and why": "我們保存什麼、為什麼",
  "Your e-mail address, the plan you chose (Free or Pro), the language of the page and when you joined.": "你的電子郵件地址、你選的方案（免費版或 Pro）、頁面語言，以及加入的時間。",
  "We use them only to e-mail you an invite, or to tell you when Pro opens. Nothing else is sent, and your address is never sold or used for marketing.":
    "只用來寄邀請給你，或在 Pro 開放時通知你。不會寄別的信，你的信箱也絕不出售、不用於行銷。",
  "Not your IP address, nor anything else about your browser. To stop abuse, our servers count requests per network in memory, for a day at most.":
    "不保存你的 IP 位址，也不保存瀏覽器的任何其他資訊。為防止濫用，伺服器會在記憶體裡按網路計數請求，最多保留一天。",
  "How long, and how to be taken off": "保存多久、怎麼退出",
  "Until you ask us to remove it, and never more than 12 months after you last joined: older entries are deleted automatically.":
    "保存到你要求刪除為止，最長不超過你最後一次加入後的 12 個月：超過的紀錄會自動刪除。",
  "To be taken off, write to {{email}} from that address and we will delete it.": "想退出，用那個信箱寫信到 {{email}}，我們會刪除它。",
  "Your account": "你的帳號",
  "If you use Droppen": "如果你在用 Droppen",
  "Your sessions and terminals are encrypted on your devices, and only your devices hold the keys. Our servers pass and keep what they cannot read.":
    "你的工作階段和終端機都在你的裝置上加密，金鑰只在你的裝置裡。我們的伺服器轉送、保存的，都是它讀不了的內容。",
  "What our servers can see": "我們的伺服器看得到",
  "Your e-mail address, and when you sign in": "你的電子郵件地址，以及登入時間",
  "Which of your devices are online, when they connect and from which IP address": "你的哪些裝置在線、何時連線、從哪個 IP 位址",
  "What kind of device each one is and which browser, as a short name such as “iPhone · Chrome” made from its User-Agent; it does not keep the User-Agent itself":
    "每台裝置的類型和瀏覽器，只記一個簡稱，例如「iPhone · Chrome」，由它的 User-Agent 得出；User-Agent 本身從不保存",
  "How much data moves and when. What we store is padded to fixed sizes; live connections are not, so while our servers carry one they see its timing, its volume and the rhythm of typing in a terminal: roughly when you are working": "傳了多少資料、什麼時候傳的。我們保存的資料會填充到固定大小；即時連線不做填充，所以經我們的伺服器轉送時，它的時序、流量和終端機裡的打字節奏都看得到：也就是你大概什麼時候在工作",
  "Your plan, and how much of it you use": "你的方案，以及用了多少",
  "What they cannot see": "我們的伺服器看不到",
  "What is said in a session, or typed and shown in a terminal": "工作階段裡說了什麼，終端機裡輸入和顯示了什麼",
  "Session titles, folders, repositories and branches": "工作階段標題、資料夾、儲存庫和分支",
  "Model names, which AI tool you use, your AI accounts and their quotas": "模型名稱、你用哪個 AI 工具、你的 AI 帳號和額度",
  "The names of your devices, your files and their names": "裝置名稱、你的檔案以及檔名",
  "Contact": "聯絡我們",
  "Questions, or a request about your data:": "有疑問，或想處理你的資料，請寫信到：",
  // Deleting an account (privacy notice)
  "Deleting your account":
    "刪除帳號",
  "If you delete your account":
    "如果你刪除了帳號",
  "What is deleted":
    "刪除什麼",
  "You can delete your account yourself, in its settings. It is gone at once and for good: nobody, us included, can bring it back.":
    "你可以在帳號設定裡自己刪除帳號。刪除立即生效、無法撤銷：誰都無法復原，包括我們。",
  "Deleted with it: your devices, the devices that run AI you added, the encrypted copies of your sessions and terminals, your keys, your plan, your password and passkeys, and the records of verification mail sent to your address.":
    "一併刪除的有：你的裝置、你新增的可執行 AI 的裝置、工作階段和終端機的加密副本、你的金鑰、你的方案、密碼和通行金鑰，以及寄往你信箱的驗證郵件紀錄。",
  "Your address also leaves the waitlist, if it was on it.":
    "如果你的信箱在等候名單上，也會一併移除。",
  "What we keep, and for how long":
    "我們保留什麼、保留多久",
  "A keyed hash of your e-mail address, never the address itself, for {{days}} days, so the same inbox cannot sign up again straight away. How long is a setting of Droppen Hub: {{days}} days on droppen.app.":
    "你電子郵件地址的帶金鑰雜湊（不是地址本身），保留 {{days}} 天，防止同一個信箱馬上重新註冊。保留多久由 Droppen Hub 的設定決定：droppen.app 上是 {{days}} 天。",
  "For the same {{days}} days, your account's and your devices' random ids and your devices' public keys, which open nothing: our servers use them to turn the account's connections away, and to tell a device that comes back that the account is gone, so it forgets its keys.":
    "同樣保留 {{days}} 天：你帳號和各個裝置的隨機 ID，以及裝置的公鑰（打不開任何東西）。我們的伺服器用它們拒絕這個帳號的連線，並告訴再次連上的裝置帳號已刪除，讓它忘掉自己的金鑰。",
  "Deleting removes all this from our live systems at once. Copies in our backups and logs expire after {{period}}.":
    "刪除會立即把這些從我們的線上系統中移除。備份和日誌中的副本會在 {{period}} 後過期。",
};

export const pricingJa: Bundle = {
  "Pricing": "料金",
  "Privacy notice": "プライバシーについて",
  "How the encryption works, in detail": "暗号化のしくみ（詳しく）",
  "No code yet? Join the waitlist": "招待コードがない方はウェイトリストへ",

  "Droppen pricing — start free, Pro for ${{price}} a month": "Droppen の料金 — 無料から、Pro は月 ${{price}}",
  "Use Droppen on one device with no account, free with an invite, or Pro for more devices and longer offline reading. Every plan is end-to-end encrypted.":
    "1 台ならアカウントなしで、招待があれば無料で、もっと多くのデバイスと長いオフライン閲覧なら Pro で。どのプランもエンドツーエンド暗号化です。",
  "Start free. Go Pro when you need more.": "まずは無料で。足りなくなったら Pro へ。",
  "Every plan is end-to-end encrypted: we can't see your code on any of them. A plan only decides how many devices you connect, how far back your phone can read while a device is off, and how much our servers carry for you each month.":
    "どのプランもエンドツーエンド暗号化で、私たちにはあなたのコードが見えません。プランで決まるのは 3 つだけ。つなげるデバイスの数、デバイスの電源が切れているときにスマホで読める範囲、そして私たちのサーバーが毎月転送する量です。",
  "Prices in US dollars.": "価格は米ドル建てです。",

  "End-to-end encrypted: we can't see your code": "エンドツーエンド暗号化：あなたのコードは私たちにも見えません",
  "Local only": "ローカルのみ",
  "forever": "ずっと無料",
  "No account, nothing to sign up for": "アカウント不要、登録もいりません",
  "Droppen on the device your AI runs on, and nothing in the cloud.": "Droppen は AI が動くデバイスの中だけ。クラウドには何も置きません。",
  "1 device that runs AI: this one": "AI を実行できるデバイス 1 台：このデバイス",
  "Use it from any browser on the same network": "同じネットワークのブラウザならどれからでも使える",
  "Nothing goes through our servers": "私たちのサーバーを一切通らない",
  "Free": "無料",
  "USD / month": "USD / 月",
  "Sign-up is by invite for now": "現在は招待コードで登録",
  "Join the waitlist": "ウェイトリストに登録",
  "You have an account already.": "アカウントはすでにお持ちです。",
  "Have an invite code? Sign up": "招待コードをお持ちなら登録へ",
  "Your AI on your phone, wherever you are.": "どこにいても、スマホで AI の続きを。",
  "{{n}} devices that run AI": "AI を実行できるデバイス {{n}} 台",
  "{{n}} devices in all, phones and browsers included": "デバイスは全部で {{n}} 台（スマホとブラウザを含む）",
  "Read the last {{n}} days on your phone while a device is off": "デバイスの電源が切れていても、直近 {{n}} 日分のセッションをスマホで読める",
  "{{n}} GB a month through our servers, then slower": "私たちのサーバー経由の転送は月 {{n}} GB まで、超えると低速に",
  "Waitlist": "ウェイトリスト",
  "or ${{price}} a year: save {{saving}}%": "年払いなら ${{price}}（{{saving}}% お得）",
  "Payments are not open yet: we'll e-mail you when they are.": "お支払いはまだ受け付けていません。始まったらメールでお知らせします。",
  "Everything in Free, plus:": "無料プランのすべてに加えて：",
  "{{n}} GB a month through our servers, then billed by use, at full speed": "私たちのサーバー経由の転送は月 {{n}} GB、超えた分は従量課金で速度はそのまま",

  "Compare": "比較",
  "What each plan includes": "各プランの内容",
  "included": "含まれる",
  "not included": "含まれない",
  "Price": "料金",
  "Free forever, no account": "ずっと無料、アカウント不要",
  "Free; sign-up by invite for now": "無料（現在は招待制）",
  "${{monthly}} a month, or ${{yearly}} a year": "月 ${{monthly}}、または年 ${{yearly}}",
  "Devices that run AI": "AI を実行できるデバイス",
  "1: this device": "1 台：このデバイス",
  "All your devices, phones and browsers included": "デバイスの総数（スマホ・ブラウザを含む）",
  "This device, and browsers on the same network": "このデバイスと、同じネットワークのブラウザ",
  "How far back your phone can read while a device is off": "デバイスの電源が切れているとき、スマホで読める範囲",
  "{{n}} days": "直近 {{n}} 日",
  "A month's transfer through our servers": "私たちのサーバー経由の月間転送量",
  "{{n}} GB, then slower": "{{n}} GB、超えると低速",
  "{{n}} GB, then billed by use": "{{n}} GB、超えた分は従量課金",
  "A device off for longer than that has its copy in the cloud deleted: what is on the device stays, and its last {{free}} days ({{pro}} on Pro) come back once it is online.":
    "それより長く電源が切れたデバイスは、クラウド上のコピーが削除されます。デバイス内の記録はそのままで、オンラインに戻ると直近 {{free}} 日分（Pro は {{pro}} 日分）が自動で戻ります。",

  "Leave your address, and we'll write when it's your turn.": "メールアドレスを残してください。順番が来たらご連絡します。",
  "Sign-up is invite-only while Droppen is in beta, and Pro is not on sale yet. Pick what you are waiting for: we'll e-mail you an invite, or tell you when Pro opens. Submitting the form is all it takes.":
    "Droppen は招待制ベータのため登録には招待コードが必要で、Pro もまだ販売していません。待っているものを選んでください。招待をメールでお送りするか、Pro の開始をお知らせします。フォームを送信するだけで登録完了です。",
  "Plan": "プラン",
  "${{price}} / month": "${{price}} / 月",
  "E-mail address": "メールアドレス",
  "No confirmation mail and no newsletter: we use your address only to send what you asked for.": "確認メールもニュースレターもありません。アドレスは、ご希望の連絡を送るためだけに使います。",
  "That does not look like an e-mail address. Check it and try again.": "メールアドレスの形式ではないようです。確認してもう一度お試しください。",
  "Use an address you keep: throwaway mail providers are not accepted.": "普段お使いのアドレスを入力してください。使い捨てメールは受け付けていません。",
  "Too many tries from your network. Try again in {{n}} min.": "このネットワークからの試行が多すぎます。{{n}} 分後にもう一度お試しください。",
  "Could not reach Droppen. Check your connection and try again.": "Droppen に接続できませんでした。接続を確認してもう一度お試しください。",
  "You're on the list: we'll e-mail you an invite when Pro opens.": "登録しました。Pro が始まったら招待をメールでお送りします。",
  "You're on the list: we'll e-mail you an invite.": "登録しました。招待をメールでお送りします。",
  "We'll send nothing else. To be taken off the list, write to {{email}}.": "それ以外のメールは送りません。リストから外れたい場合は {{email}} までご連絡ください。",
  "Add another address": "別のアドレスを登録",

  "Questions": "よくある質問",
  "Before you choose": "選ぶ前に",
  "What happens when I use up a month's transfer?": "月間の転送量を使い切ったら？",
  "Only what goes through our servers counts: what goes straight between your devices does not. On Free, past {{free}} GB that traffic slows to {{kb}} KB/s until the next month starts; terminals and reading still work, just more slowly. On Pro nothing slows down: what you use past {{pro}} GB is billed by use, at a price we will publish before payments open.":
    "数えるのは私たちのサーバーを経由した分だけで、デバイス同士の直結は含みません。無料プランでは {{free}} GB を超えるとその通信が翌月まで {{kb}} KB/s に制限されます。ターミナルも閲覧も、遅くなるだけで使えます。Pro は速度制限なし。{{pro}} GB を超えた分は従量課金で、単価はお支払い開始前に公開します。",
  "What does “local only” mean?": "「ローカルのみ」とは？",
  "Droppen runs on your device with no account. You use it in a browser on that device, or on another device on the same network, and nothing goes through our servers. Sign in to Droppen when you want your AI on your phone from anywhere, and all your devices on one screen.":
    "Droppen をアカウントなしでデバイス上で動かす使い方です。そのデバイスか、同じネットワークにある別のデバイスのブラウザで開いて使い、私たちのサーバーは一切通りません。外出先からスマホで AI の続きを使いたいとき、すべてのデバイスをひとつの画面にまとめたいときに、Droppen にログインしてください。",
  "Can I host it myself?": "セルフホストできますか？",
  "The Droppen app and server are licensed under the AGPL-3.0, and local only needs nothing from us. Their source code is at github.com/getdroppen/droppen. Droppen Hub, the service at droppen.app that connects your devices, is not open source yet, so it cannot be self-hosted for now.":
    "Droppen のアプリとサーバーは AGPL-3.0 で提供しており、「ローカルのみ」なら私たちのサービスは何も要りません。ソースコードは github.com/getdroppen/droppen で公開しています。デバイス同士をつなぐ Droppen Hub（droppen.app のサービス）はまだオープンソースではないため、現時点ではセルフホストできません。",
  "When can I pay for Pro?": "Pro はいつ購入できますか？",
  "Not yet: payments are on their way. Until then Pro is by waitlist: join it and we'll e-mail you when Pro opens. Nothing is charged before you choose to pay, and prices are in US dollars.":
    "まだです。お支払いの仕組みを準備中で、それまで Pro はウェイトリスト制です。登録いただければ、Pro の開始時にメールでお知らせします。ご自身で支払うと決めるまで料金は一切かかりません。価格は米ドル建てです。",
  "What if a device stays off for longer?": "デバイスの電源が長く切れたままだと？",
  "On Free your phone can read a device's last {{free}} days while it is off, on Pro its last {{pro}}. A device off for longer than that has its encrypted copy in the cloud deleted: what is on the device itself is not affected, and its recent days are uploaded again once it is back online.":
    "電源が切れたデバイスのセッションは、無料プランなら直近 {{free}} 日分、Pro なら {{pro}} 日分をスマホで読めます。それより長く切れたままだと、クラウド上の暗号化コピーは削除されます。デバイス内の記録には影響せず、オンラインに戻ると最近の分が自動で再アップロードされます。",
  "Is my code safer on Pro?": "Pro のほうがコードは安全ですか？",
  "It is just as safe on every plan: all of them are end-to-end encrypted the same way, and we can't read your sessions, your code or your terminals on any. A plan only changes how much we keep and carry for you, never who can read it.":
    "どのプランでも同じだけ安全です。すべて同じ方式でエンドツーエンド暗号化され、どのプランでも私たちはあなたのセッション、コード、ターミナルを読めません。プランで変わるのは私たちが保管・転送する量だけで、誰が読めるかは変わりません。",

  "Privacy notice — Droppen": "プライバシーについて — Droppen",
  "What we keep, and what we can't see.": "私たちが保管するもの、見えないもの。",
  "Droppen is end-to-end encrypted, so there is little to tell: here is all of it.": "Droppen はエンドツーエンド暗号化なので、お伝えすることは多くありません。すべてここに書きます。",
  "Last updated {{date}}": "最終更新：{{date}}",
  "If you joined the waitlist": "ウェイトリストに登録した方へ",
  "What we keep, and why": "保管するものとその理由",
  "Your e-mail address, the plan you chose (Free or Pro), the language of the page and when you joined.": "メールアドレス、選んだプラン（無料または Pro）、ページの言語、登録した日時。",
  "We use them only to e-mail you an invite, or to tell you when Pro opens. Nothing else is sent, and your address is never sold or used for marketing.":
    "用途は招待のお送りと Pro 開始のお知らせだけです。それ以外のメールは送らず、アドレスを販売したりマーケティングに使ったりすることもありません。",
  "Not your IP address, nor anything else about your browser. To stop abuse, our servers count requests per network in memory, for a day at most.":
    "IP アドレスやブラウザに関する情報は保管しません。不正利用を防ぐため、サーバーはネットワークごとのリクエスト数をメモリ上で数えますが、保持は最長 1 日です。",
  "How long, and how to be taken off": "保管期間と削除の方法",
  "Until you ask us to remove it, and never more than 12 months after you last joined: older entries are deleted automatically.":
    "削除のご依頼をいただくまで保管しますが、最後に登録してから 12 か月を超えることはありません。それより古い記録は自動で削除されます。",
  "To be taken off, write to {{email}} from that address and we will delete it.": "削除をご希望の場合は、登録したアドレスから {{email}} までご連絡ください。削除いたします。",
  "Your account": "アカウント",
  "If you use Droppen": "Droppen をお使いの方へ",
  "Your sessions and terminals are encrypted on your devices, and only your devices hold the keys. Our servers pass and keep what they cannot read.":
    "セッションとターミナルはあなたのデバイス上で暗号化され、鍵を持つのはあなたのデバイスだけです。私たちのサーバーが転送・保管するのは、サーバー自身には読めないものです。",
  "What our servers can see": "私たちのサーバーに見えるもの",
  "Your e-mail address, and when you sign in": "メールアドレスとログインの時刻",
  "Which of your devices are online, when they connect and from which IP address": "どのデバイスがオンラインか、いつ、どの IP アドレスから接続したか",
  "What kind of device each one is and which browser, as a short name such as “iPhone · Chrome” made from its User-Agent; it does not keep the User-Agent itself":
    "各デバイスの種類とブラウザ（「iPhone · Chrome」のような短い名前。User-Agent から作り、User-Agent そのものは保存しません）",
  "How much data moves and when. What we store is padded to fixed sizes; live connections are not, so while our servers carry one they see its timing, its volume and the rhythm of typing in a terminal: roughly when you are working": "どれだけのデータがいつ流れたか。保存するデータは決まったサイズまでパディングしますが、ライブの接続はしません。そのため私たちのサーバーを通る間は、そのタイミング、量、ターミナルでの打鍵のリズムが見えます。つまり、だいたいいつ作業しているか",
  "Your plan, and how much of it you use": "契約プランと、その使用量",
  "What they cannot see": "見えないもの",
  "What is said in a session, or typed and shown in a terminal": "セッションでのやり取り、ターミナルの入力と出力",
  "Session titles, folders, repositories and branches": "セッションのタイトル、フォルダ、リポジトリ、ブランチ",
  "Model names, which AI tool you use, your AI accounts and their quotas": "モデル名、どの AI ツールを使っているか、AI のアカウントと使用枠",
  "The names of your devices, your files and their names": "デバイス名、ファイルとファイル名",
  "Contact": "お問い合わせ",
  "Questions, or a request about your data:": "ご質問やデータに関するご依頼は：",
  // Deleting an account (privacy notice)
  "Deleting your account":
    "アカウントの削除",
  "If you delete your account":
    "アカウントを削除した場合",
  "What is deleted":
    "削除されるもの",
  "You can delete your account yourself, in its settings. It is gone at once and for good: nobody, us included, can bring it back.":
    "アカウントは設定からご自身で削除できます。削除はすぐに行われ、元に戻せません。私たちを含め、誰にも復元できません。",
  "Deleted with it: your devices, the devices that run AI you added, the encrypted copies of your sessions and terminals, your keys, your plan, your password and passkeys, and the records of verification mail sent to your address.":
    "あわせて削除されるもの：デバイス、追加した AI を実行できるデバイス、セッションとターミナルの暗号化コピー、鍵、プラン、パスワードとパスキー、そしてあなたのアドレスに送った確認メールの記録。",
  "Your address also leaves the waitlist, if it was on it.":
    "ウェイトリストに登録していた場合は、そのアドレスも削除されます。",
  "What we keep, and for how long":
    "残すものと、その期間",
  "A keyed hash of your e-mail address, never the address itself, for {{days}} days, so the same inbox cannot sign up again straight away. How long is a setting of Droppen Hub: {{days}} days on droppen.app.":
    "メールアドレスの鍵付きハッシュ（アドレスそのものではありません）を {{days}} 日間。同じ受信箱ですぐに再登録できないようにするためです。期間は Droppen Hub の設定で、droppen.app では {{days}} 日です。",
  "For the same {{days}} days, your account's and your devices' random ids and your devices' public keys, which open nothing: our servers use them to turn the account's connections away, and to tell a device that comes back that the account is gone, so it forgets its keys.":
    "同じ {{days}} 日間、アカウントと各デバイスのランダムな ID、そしてデバイスの公開鍵（何も開けません）。私たちのサーバーはこれを使ってこのアカウントの接続を断り、戻ってきたデバイスにアカウントが削除されたことを伝えて、その鍵を消させます。",
  "Deleting removes all this from our live systems at once. Copies in our backups and logs expire after {{period}}.":
    "削除すると、これらは稼働中のシステムからすぐに消えます。バックアップとログに残る写しは {{period}} で期限切れになります。",
};
