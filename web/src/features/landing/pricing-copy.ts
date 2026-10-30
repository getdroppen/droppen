/*
 * The words of the pricing page (`pricing.tsx`), its waitlist form
 * (`waitlist.tsx`), the privacy notice (`privacy.tsx`) and the public pages'
 * links to them and to the terms of service (`page-kit.tsx`; the terms'
 * own words are in `terms/terms-copy.ts`) — merged into the landing page's tables
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
  "Terms of Service": "服务条款",
  "How the encryption works, in detail": "加密是怎么做的：详细说明",
  "No code yet? Join the waitlist": "还没有邀请码？加入等候名单",

  // Page
  "Droppen pricing — start free, Pro for ${{price}} a month": "Droppen 定价 — 免费起步，Pro 每月 ${{price}}",
  "Use Droppen on one device with no account, free with an invite, or Pro for more devices and longer offline reading. Every plan is end-to-end encrypted.":
    "一台设备不用账号就能用；凭邀请码免费用；想要更多设备、更长的离线可读时间就选 Pro。每个方案都端到端加密。",
  "Use Droppen on one device with no account, free with an account, or Pro for more devices and longer offline reading. Every plan is end-to-end encrypted.":
    "一台设备不用账号就能用；注册账号免费用；想要更多设备、更长的离线可读时间就选 Pro。每个方案都端到端加密。",
  "Start free. Go Pro when you need more.": "免费开始，需要更多时升级 Pro",
  "Every plan is end-to-end encrypted: on none of them can we read your sessions, your code or your terminals. A plan decides only how many devices you can connect, how many days of sessions your phone can read while a device that runs AI is offline, and how much traffic goes through our servers each month.":
    "所有方案均端到端加密：无论哪个方案，我们都无法读取你的会话、代码和终端。方案只决定三项：可连接的设备数量；可运行 AI 的设备离线时，手机上可查看的会话天数；以及每月经我们服务器转发的流量。",
  "Prices in US dollars. Pro is not on sale yet, and nothing is charged before you choose to pay.": "价格以美元计。Pro 尚未开放购买；在你决定付款之前，不会产生任何费用。",

  // The plans
  "End-to-end encrypted: we can't read your sessions, code or terminals": "端到端加密：我们无法读取你的会话、代码和终端",
  "Local only": "仅本地",
  "Recommended": "推荐",
  "forever": "永久免费",
  "No account, nothing to sign up for": "无需账号，也不用注册",
  "Droppen on the device your AI runs on, and nothing in the cloud.": "Droppen 只安装在运行 AI 的设备上，云端不保存任何内容。",
  "1 device that runs AI: the one Droppen is installed on": "1 台可运行 AI 的设备：即安装 Droppen 的设备",
  "Use it in a browser on that device, or on another device on the same network": "可在该设备或同一网络中其他设备的浏览器上使用",
  "Nothing goes through our servers": "不经过我们的服务器",
  "Free": "免费版",
  "USD / month": "美元 / 月",
  "Sign-up is by invite for now": "目前凭邀请码注册",
  "Sign-up is closed for now": "目前暂停注册",
  "Sign up with an e-mail address": "使用邮箱注册",
  "Sign up": "注册",
  "Join the waitlist": "加入等候名单",
  "You have an account already.": "你已经有账号了。",
  "Have an invite code? Sign up": "有邀请码？去注册",
  "Your AI on your phone, wherever you are.": "无论身在何处，都能在手机上使用你的 AI。",
  "{{n}} devices that run AI": "{{n}} 台可运行 AI 的设备",
  "{{n}} devices in all, including those that run AI, phones and browsers": "共 {{n}} 台设备，含可运行 AI 的设备、手机和浏览器",
  "While a device that runs AI is offline, read its last {{n}} days of sessions on your phone": "可运行 AI 的设备离线时，手机上可查看它最近 {{n}} 天的会话",
  "{{n}} GB a month through our servers, then {{kb}} KB/s until the next month": "每月经我们的服务器转发 {{n}} GB，超出后限速至 {{kb}} KB/s，直到下个月",
  "Waitlist": "等候名单",
  "or ${{price}} a year: save {{saving}}%": "或每年 ${{price}}，省 {{saving}}%",
  "Payments are not open yet: we'll e-mail you when they are.": "尚未开放付款，开放时我们会发邮件通知你。",
  "Everything in Free, with higher limits.": "包含免费版的全部功能，额度更高。",
  "{{n}} GB a month through our servers, then billed by use, at full speed": "每月经我们的服务器转发 {{n}} GB，超出部分不限速，按量计费",

  // Compare
  "Compare": "对比",
  "What each plan includes": "各方案包含的内容",
  "included": "包含",
  "Price": "价格",
  "Free forever, no account": "永久免费，无需账号",
  "Free; sign-up by invite for now": "免费；目前凭邀请码注册",
  "Free; sign-up is closed for now": "免费；目前暂停注册",
  "Free, with an account": "免费，需要账号",
  "${{monthly}} a month, or ${{yearly}} a year": "每月 ${{monthly}}，或每年 ${{yearly}}",
  "Devices that run AI": "可运行 AI 的设备",
  "1: the device Droppen is installed on": "1 台：安装 Droppen 的设备",
  "All your devices, including those that run AI, phones and browsers": "设备总数（含可运行 AI 的设备、手机和浏览器）",
  "That device, and browsers on the same network": "该设备，以及同一网络中的浏览器",
  "Days of sessions your phone can read while a device that runs AI is offline": "可运行 AI 的设备离线时，手机可查看的会话天数",
  "None: nothing is kept in the cloud": "无：云端不保存任何内容",
  "{{n}} days": "最近 {{n}} 天",
  "A month's transfer through our servers": "每月经我们的服务器转发的流量",
  "{{n}} GB, then {{kb}} KB/s": "{{n}} GB，超出后限速至 {{kb}} KB/s",
  "{{n}} GB, then billed by use": "{{n}} GB，超出部分按量计费",
  "Free keeps an encrypted copy of each device's last {{free}} days of sessions in the cloud, Pro its last {{pro}}. A device offline for longer than that has its cloud copy deleted: what is on the device is not affected, and those days are uploaded again once it is back online.":
    "免费版在云端保存每台设备最近 {{free}} 天会话的加密副本，Pro 保存最近 {{pro}} 天。设备离线超过这一期限，其云端副本会被删除；设备上的记录不受影响，重新上线后会自动再次上传这段时间的会话。",

  // Waitlist
  "Leave your address, and we'll write when it's your turn": "留下邮箱，轮到你时我们会发邮件通知你",
  "Leave your address, and we'll write when sign-up or Pro opens": "留下邮箱，开放注册或 Pro 开放购买时，我们会发邮件通知你",
  "Leave your address, and we'll write when Pro opens": "留下邮箱，Pro 开放时通知",
  "Pro is not on sale yet: payments are on their way. Submitting the form is all it takes.": "Pro 尚未开放购买，付款功能仍在准备中。提交邮箱即可加入，无需其他步骤。",
  "Sign-up is closed for now, and Pro is not on sale yet. Pick what you are waiting for: we'll e-mail you when sign-up opens, or when Pro does. Submitting the form is all it takes.":
    "目前暂停注册，Pro 也尚未开放购买。请选择你在等待的项目：开放注册或 Pro 开放购买时，我们会发邮件通知你。提交邮箱即可加入，无需其他步骤。",
  "Sign-up is invite-only while Droppen is in beta, and Pro is not on sale yet. Pick what you are waiting for: we'll e-mail you an invite, or tell you when Pro opens. Submitting the form is all it takes.":
    "Droppen 目前为邀请制测试版，注册需要邀请码；Pro 也尚未开放购买。请选择你在等待的项目：我们会把邀请发到你的邮箱，或在 Pro 开放购买时通知你。提交邮箱即可加入，无需其他步骤。",
  "Plan": "方案",
  "${{price}} / month": "${{price}} / 月",
  "E-mail address": "邮箱地址",
  "No confirmation mail and no newsletter: we use your address only to send what you asked for.": "不发送确认邮件，也不发送营销邮件：你的邮箱仅用于发送你所登记的通知。",
  "That does not look like an e-mail address. Check it and try again.": "这看起来不像邮箱地址，检查一下再试。",
  "Use an address you keep: throwaway mail providers are not accepted.": "请用你长期使用的邮箱：不接受一次性邮箱。",
  "Too many tries from your network. Try again in {{n}} min.": "你所在的网络尝试次数太多，请 {{n}} 分钟后再试。",
  "Could not reach Droppen. Check your connection and try again.": "连不上 Droppen，检查网络后再试。",
  "You're on the list: we'll e-mail you when Pro opens.": "你已在名单上，Pro 开放时我们会发邮件通知你。",
  "You're on the list: we'll e-mail you an invite.": "你已在名单上，我们会把邀请发到你的邮箱。",
  "You're on the list: we'll e-mail you when sign-up opens.": "你已在名单上，开放注册时我们会发邮件通知你。",
  "We'll send nothing else. To be taken off the list, write to {{email}}.": "除此之外，我们不会发送任何邮件。如需退出名单，请发邮件至 {{email}}。",
  "Add another address": "再添加一个邮箱",

  // Questions
  "Questions": "常见问题",
  "Before you choose": "选择方案之前",
  "What happens when I use up a month's transfer?": "每月流量额度用完后会怎样？",
  "Only what goes through our servers counts: what goes straight between your devices does not. On Free, past {{free}} GB that traffic slows to {{kb}} KB/s until the next month starts; terminals and reading still work, just more slowly. On Pro nothing slows down: what you use past {{pro}} GB is billed by use, at a price we will publish before payments open.":
    "只有经我们服务器转发的流量才计入，设备之间直接传输的流量不计入。免费版每月转发流量超过 {{free}} GB 后，转发速度限制为 {{kb}} KB/s，直到下个月开始；终端和会话查看仍可使用，但速度较慢。Pro 不限速，超过 {{pro}} GB 的部分按量计费，单价将在开放付款前公布。",
  "What does “local only” mean?": "「仅本地」是什么意思？",
  "Droppen runs on your device with no account. You use it in a browser on that device, or on another device on the same network, and nothing goes through our servers. Sign in to Droppen when you want to reach your AI from your phone anywhere, and the sessions of all your devices in one list.":
    "Droppen 在你的设备上运行，无需账号。在这台设备或同一网络中其他设备的浏览器上即可使用，任何内容都不经过我们的服务器。需要在外出时从手机使用 AI，或在同一个列表中查看所有设备的会话时，再登录 Droppen。",
  "Can I host it myself?": "可以自行部署吗？",
  "Local only runs entirely on your own devices and needs nothing from us. The Droppen client is open source under the AGPL-3.0, at github.com/getdroppen/droppen. Droppen Hub, the service at droppen.app that connects your devices, is not open source yet, so it cannot be self-hosted for now.":
    "「仅本地」完全在你自己的设备上运行，不依赖我们的任何服务。Droppen 客户端以 AGPL-3.0 开源，源代码位于 github.com/getdroppen/droppen。连接各台设备的 Droppen Hub（即 droppen.app 背后的服务）尚未开源，因此目前还不能自行部署。",
  "When can I pay for Pro?": "何时可以付费开通 Pro？",
  "Not yet: payments are on their way. Until then Pro is by waitlist: join it and we'll e-mail you when Pro opens. Nothing is charged before you choose to pay, and prices are in US dollars.":
    "目前尚未开放，付款功能正在准备中。在此之前，Pro 仅接受等候名单登记，开放购买时我们会发邮件通知你。在你决定付款之前，不会产生任何费用；价格以美元计。",
  "What if a device stays offline for longer?": "设备长时间离线会怎样？",
  "On Free your phone can read a device's last {{free}} days of sessions while it is offline, on Pro its last {{pro}}. A device offline for longer than that has its encrypted copy in the cloud deleted: what is on the device itself is not affected, and its recent sessions are uploaded again once it is back online.":
    "设备离线期间，免费版可在手机上查看它最近 {{free}} 天的会话，Pro 为 {{pro}} 天。离线超过这一期限，该设备在云端的加密副本会被删除；设备上的记录不受影响，重新上线后会自动再次上传最近的会话。",
  "Is my code safer on Pro?": "Pro 会让代码更安全吗？",
  "It is just as safe on every plan: all of them are end-to-end encrypted the same way, and we can't read your sessions, your code or your terminals on any. A plan only changes how much we keep and carry for you, never who can read it.":
    "所有方案的安全性相同：都以同样的方式端到端加密，无论哪个方案，我们都无法读取你的会话、代码和终端。方案只决定我们替你保存和转发多少数据，不改变谁能读取这些数据。",

  // Privacy notice
  "Privacy notice — Droppen": "隐私说明 — Droppen",
  "What we keep, and what we can't see.": "我们保存什么，又看不到什么",
  "Droppen is end-to-end encrypted, so there is little to tell: here is all of it.": "Droppen 是端到端加密的，所以要说明的不多，全在这里了。",
  "Last updated {{date}}": "最后更新：{{date}}",
  "If you joined the waitlist": "如果你加入了等候名单",
  "What we keep, and why": "我们保存什么、为什么",
  "Your e-mail address, the plan you are waiting for (Free or Pro), the language of the page and when you joined.": "你的邮箱地址、你在等的方案（免费版或 Pro）、页面语言，以及加入的时间。",
  "We use them only to e-mail you an invite, or to tell you when sign-up or Pro opens. Nothing else is sent, and your address is never sold or used for marketing.":
    "只用来给你发邀请，或在开放注册、Pro 开放时通知你。不会发别的邮件，你的邮箱也绝不出售、不用于营销。",
  "Not your IP address, nor anything else about your browser; our hosting provider's edge logs do record your IP address and User-Agent (see below). To stop abuse, our servers count requests per address and per network in memory, for a day at most.":
    "不保存你的 IP 地址，也不保存浏览器的任何其他信息；不过托管服务商的边缘日志会记录你的 IP 地址和 User-Agent（见下文）。为防止滥用，服务器会在内存里按地址和网络计数请求，最多保留一天。",
  "How long, and how to be taken off": "保存多久、怎么退出",
  "Until you ask us to remove it, and never more than 12 months after you last joined: older entries are deleted automatically.":
    "保存到你要求删除为止，最长不超过你最后一次加入后的 12 个月：超过的记录会自动删除。",
  "To be taken off, write to {{email}} from that address and we will delete it.": "想退出，用那个邮箱写信到 {{email}}，我们会删除它。",
  // Downloads (privacy notice)
  "Downloads":
    "下载",
  "If you download Droppen":
    "如果你下载了 Droppen",
  "What we count":
    "我们计数什么",
  "When you download Droppen from droppen.app, or run its install command, our servers add one to that day's count for that download: the macOS app, a Linux build or the install script.":
    "你从 droppen.app 下载 Droppen，或运行它的安装命令时，我们的服务器会给这项下载当天的计数加一：macOS App、Linux 版本或安装脚本。",
  "That number is all we keep: one count per download per day, which says nothing about who downloaded.":
    "我们只保存这个数字：每项下载每天一个计数，看不出是谁下载的。",
  "What we don't keep":
    "我们不保存什么",
  "Not your IP address, nor anything else about your browser, and no cookie. To keep one source from inflating the counts, our servers hold the address and the network a download came from in memory, for an hour at most, and write them nowhere.":
    "不保存你的 IP 地址，也不保存浏览器的任何其他信息，不用 Cookie。为防止有人刷高计数，服务器会把下载来自的地址和网络只留在内存里，最多一小时，不写到任何地方。",
  "Our hosting provider's edge logs do record the IP address and User-Agent of every request, downloads included (see below).":
    "不过托管服务商的边缘日志会记录每个请求（包括下载）的 IP 地址和 User-Agent（见下文）。",
  "Your account": "你的账号",
  "If you use Droppen": "如果你在用 Droppen",
  "Your sessions and terminals are encrypted on your devices, and only your devices hold the keys. Our servers pass and keep what they cannot read.":
    "你的会话和终端都在你的设备上加密，钥匙只在你的设备里。我们的服务器转发、保存的，都是它读不了的内容。",
  "What our servers can see": "我们的服务器看得到",
  "Your e-mail address, and when you sign in": "你的邮箱地址，以及登录时间",
  "Which of your devices are online, when they connect and from which IP address": "你的哪些设备在线、何时连接、从哪个 IP 地址",
  "What kind of device each one is and which browser, as a short name such as “iPhone · Chrome” made from its User-Agent; our servers do not keep the User-Agent itself, though our hosting provider's edge logs do (see below)":
    "每台设备的类型和浏览器，只记一个简称，比如「iPhone · Chrome」，由它的 User-Agent 得出；我们的服务器从不保存 User-Agent 本身，不过托管服务商的边缘日志会记录它（见下文）",
  "How much data moves and when. What we store is padded to fixed sizes; live connections are not, so while our servers carry one they see its timing, its volume and the rhythm of typing in a terminal: roughly when you are working": "传了多少数据、什么时候传的。我们保存的数据会填充到固定大小；实时连接不做填充，所以经我们的服务器转发时，它的时序、流量和终端里的打字节奏都看得到：也就是你大概什么时候在工作",
  "Your plan, and how much of it you use": "你的方案，以及用了多少",
  "What they cannot see": "我们的服务器看不到",
  "What is said in a session, or typed and shown in a terminal": "会话里说了什么，终端里输入和显示了什么",
  "Session titles, folders, repositories and branches": "会话标题、文件夹、仓库和分支",
  "Model names, which AI tool you use, your AI accounts and their quotas": "模型名、你用哪个 AI 工具、你的 AI 账号和额度",
  "The names of your devices, your files and their names": "设备名、你的文件以及文件名",
  "Providers and retention": "服务商与保存期限",
  "Who else handles it, and for how long": "还有谁经手，保存多久",
  "These services run parts of Droppen for us. Like our servers, none of them can read what is end-to-end encrypted.":
    "这些服务替我们运行 Droppen 的一部分。和我们的服务器一样，它们都读不了端到端加密的内容。",
  "Our providers": "我们的服务商",
  "Railway, in Singapore, runs our servers. Its edge logs record the IP address and User-Agent of every request to droppen.app, and keep them for as long as Railway keeps its logs.":
    "Railway（位于新加坡）托管我们的服务器。它的边缘日志会记录每个发往 droppen.app 的请求的 IP 地址和 User-Agent，并按 Railway 自己的日志保存期限保留。",
  "PlanetScale runs our database (Postgres), in Singapore.":
    "PlanetScale 替我们运行数据库（Postgres），位于新加坡。",
  "Tigris stores the encrypted pages of your sessions.": "Tigris 存储你会话的加密页面。",
  "Cloudflare runs our DNS and routes mail to our addresses. It also runs one of the STUN servers for direct connections (Google runs the other): a STUN server sees your device's public IP address.":
    "Cloudflare 负责我们的 DNS，并把发往我们地址的邮件转给我们。直连用的 STUN 服务器也有一台是它的（另一台是 Google 的）：STUN 服务器看得到你设备的公网 IP 地址。",
  "An e-mail relay service we operate sends our e-mail, such as the verification mail, to your address.":
    "我们自己运营的一个邮件转发服务负责把我们的邮件（比如验证邮件）发到你的邮箱。",
  "How long we keep it": "保存多久",
  "The encrypted copies of your sessions: the last {{free}} days on Free, {{pro}} on Pro, and {{internal}} on the internal plan, which is not for sale. A device offline for longer than that has its copy deleted.":
    "你会话的加密副本：免费版保存最近 {{free}} 天，Pro 保存 {{pro}} 天，不对外销售的内部方案保存 {{internal}} 天。设备离线超过这个天数，它的副本会被删除。",
  "An account whose e-mail address was never verified, and that never added a device, is deleted after {{days}} days.":
    "邮箱从未验证、也从未添加过设备的账号，会在 {{days}} 天后删除。",
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
  "Deleting removes all this from our live systems at once. Copies in our backups and logs expire within {{days}} days.":
    "删除会立即把这些从我们的在线系统中移除。备份和日志中的副本会在 {{days}} 天内过期。",
};

export const pricingZhTW: Bundle = {
  "Pricing": "價格",
  "Privacy notice": "隱私權聲明",
  "Terms of Service": "服務條款",
  "How the encryption works, in detail": "加密是怎麼做的：詳細說明",
  "No code yet? Join the waitlist": "還沒有邀請碼？加入等候名單",

  "Droppen pricing — start free, Pro for ${{price}} a month": "Droppen 價格 — 免費起步，Pro 每月 ${{price}}",
  "Use Droppen on one device with no account, free with an invite, or Pro for more devices and longer offline reading. Every plan is end-to-end encrypted.":
    "一台裝置不用帳號就能用；憑邀請碼免費用；想要更多裝置、更長的離線可讀時間就選 Pro。每個方案都端對端加密。",
  "Use Droppen on one device with no account, free with an account, or Pro for more devices and longer offline reading. Every plan is end-to-end encrypted.":
    "一台裝置不用帳號就能用；註冊帳號免費用；想要更多裝置、更長的離線可讀時間就選 Pro。每個方案都端對端加密。",
  "Start free. Go Pro when you need more.": "免費開始，需要更多時升級 Pro",
  "Every plan is end-to-end encrypted: on none of them can we read your sessions, your code or your terminals. A plan decides only how many devices you can connect, how many days of sessions your phone can read while a device that runs AI is offline, and how much traffic goes through our servers each month.":
    "所有方案均端對端加密：無論哪個方案，我們都無法讀取你的工作階段、程式碼和終端機。方案只決定三項：可連接的裝置數量；可執行 AI 的裝置離線時，手機上可查看的工作階段天數；以及每月經我們伺服器轉送的流量。",
  "Prices in US dollars. Pro is not on sale yet, and nothing is charged before you choose to pay.": "價格以美元計。Pro 尚未開放購買；在你決定付款之前，不會產生任何費用。",

  "End-to-end encrypted: we can't read your sessions, code or terminals": "端對端加密：我們無法讀取你的工作階段、程式碼和終端機",
  "Local only": "僅本地",
  "Recommended": "推薦",
  "forever": "永久免費",
  "No account, nothing to sign up for": "不需要帳號，也不用註冊",
  "Droppen on the device your AI runs on, and nothing in the cloud.": "Droppen 只安裝在執行 AI 的裝置上，雲端不保存任何內容。",
  "1 device that runs AI: the one Droppen is installed on": "1 台可執行 AI 的裝置：即安裝 Droppen 的裝置",
  "Use it in a browser on that device, or on another device on the same network": "可在該裝置或同一網路中其他裝置的瀏覽器上使用",
  "Nothing goes through our servers": "不經過我們的伺服器",
  "Free": "免費版",
  "USD / month": "美元 / 月",
  "Sign-up is by invite for now": "目前憑邀請碼註冊",
  "Sign-up is closed for now": "目前暫停註冊",
  "Sign up with an e-mail address": "使用電子郵件註冊",
  "Sign up": "註冊",
  "Join the waitlist": "加入等候名單",
  "You have an account already.": "你已經有帳號了。",
  "Have an invite code? Sign up": "有邀請碼？去註冊",
  "Your AI on your phone, wherever you are.": "無論身在何處，都能在手機上使用你的 AI。",
  "{{n}} devices that run AI": "{{n}} 台可執行 AI 的裝置",
  "{{n}} devices in all, including those that run AI, phones and browsers": "共 {{n}} 台裝置，含可執行 AI 的裝置、手機和瀏覽器",
  "While a device that runs AI is offline, read its last {{n}} days of sessions on your phone": "可執行 AI 的裝置離線時，手機上可查看它最近 {{n}} 天的工作階段",
  "{{n}} GB a month through our servers, then {{kb}} KB/s until the next month": "每月經我們的伺服器轉送 {{n}} GB，超出後限速至 {{kb}} KB/s，直到下個月",
  "Waitlist": "等候名單",
  "or ${{price}} a year: save {{saving}}%": "或每年 ${{price}}，省 {{saving}}%",
  "Payments are not open yet: we'll e-mail you when they are.": "尚未開放付款，開放時我們會寄信通知你。",
  "Everything in Free, with higher limits.": "包含免費版的全部功能，額度更高。",
  "{{n}} GB a month through our servers, then billed by use, at full speed": "每月經我們的伺服器轉送 {{n}} GB，超出部分不限速，按用量計費",

  "Compare": "比較",
  "What each plan includes": "各方案包含的內容",
  "included": "包含",
  "Price": "價格",
  "Free forever, no account": "永久免費，不需要帳號",
  "Free; sign-up by invite for now": "免費；目前憑邀請碼註冊",
  "Free; sign-up is closed for now": "免費；目前暫停註冊",
  "Free, with an account": "免費，需要帳號",
  "${{monthly}} a month, or ${{yearly}} a year": "每月 ${{monthly}}，或每年 ${{yearly}}",
  "Devices that run AI": "可執行 AI 的裝置",
  "1: the device Droppen is installed on": "1 台：安裝 Droppen 的裝置",
  "All your devices, including those that run AI, phones and browsers": "裝置總數（含可執行 AI 的裝置、手機和瀏覽器）",
  "That device, and browsers on the same network": "該裝置，以及同一網路中的瀏覽器",
  "Days of sessions your phone can read while a device that runs AI is offline": "可執行 AI 的裝置離線時，手機可查看的工作階段天數",
  "None: nothing is kept in the cloud": "無：雲端不保存任何內容",
  "{{n}} days": "最近 {{n}} 天",
  "A month's transfer through our servers": "每月經我們的伺服器轉送的流量",
  "{{n}} GB, then {{kb}} KB/s": "{{n}} GB，超出後限速至 {{kb}} KB/s",
  "{{n}} GB, then billed by use": "{{n}} GB，超出部分按用量計費",
  "Free keeps an encrypted copy of each device's last {{free}} days of sessions in the cloud, Pro its last {{pro}}. A device offline for longer than that has its cloud copy deleted: what is on the device is not affected, and those days are uploaded again once it is back online.":
    "免費版在雲端保存每台裝置最近 {{free}} 天工作階段的加密副本，Pro 保存最近 {{pro}} 天。裝置離線超過這一期限，其雲端副本會被刪除；裝置上的紀錄不受影響，重新上線後會自動再次上傳這段時間的工作階段。",

  "Leave your address, and we'll write when it's your turn": "留下電子郵件，輪到你時我們會寄信通知你",
  "Leave your address, and we'll write when sign-up or Pro opens": "留下電子郵件，開放註冊或 Pro 開放購買時，我們會寄信通知你",
  "Leave your address, and we'll write when Pro opens": "留下電子郵件，Pro 開放購買時我們會寄信通知你",
  "Pro is not on sale yet: payments are on their way. Submitting the form is all it takes.": "Pro 尚未開放購買，付款功能仍在準備中。送出電子郵件即可加入，無需其他步驟。",
  "Sign-up is closed for now, and Pro is not on sale yet. Pick what you are waiting for: we'll e-mail you when sign-up opens, or when Pro does. Submitting the form is all it takes.":
    "目前暫停註冊，Pro 也尚未開放購買。請選擇你在等待的項目：開放註冊或 Pro 開放購買時，我們會寄信通知你。送出電子郵件即可加入，無需其他步驟。",
  "Sign-up is invite-only while Droppen is in beta, and Pro is not on sale yet. Pick what you are waiting for: we'll e-mail you an invite, or tell you when Pro opens. Submitting the form is all it takes.":
    "Droppen 目前為邀請制測試版，註冊需要邀請碼；Pro 也尚未開放購買。請選擇你在等待的項目：我們會把邀請寄到你的信箱，或在 Pro 開放購買時通知你。送出電子郵件即可加入，無需其他步驟。",
  "Plan": "方案",
  "${{price}} / month": "${{price}} / 月",
  "E-mail address": "電子郵件地址",
  "No confirmation mail and no newsletter: we use your address only to send what you asked for.": "不寄送確認信，也不寄送行銷郵件：你的信箱僅用於寄送你所登記的通知。",
  "That does not look like an e-mail address. Check it and try again.": "這看起來不像電子郵件地址，檢查一下再試。",
  "Use an address you keep: throwaway mail providers are not accepted.": "請用你長期使用的信箱：不接受拋棄式信箱。",
  "Too many tries from your network. Try again in {{n}} min.": "你所在的網路嘗試次數太多，請 {{n}} 分鐘後再試。",
  "Could not reach Droppen. Check your connection and try again.": "連不上 Droppen，檢查網路後再試。",
  "You're on the list: we'll e-mail you when Pro opens.": "你已在名單上，Pro 開放時我們會寄信通知你。",
  "You're on the list: we'll e-mail you an invite.": "你已在名單上，我們會把邀請寄到你的信箱。",
  "You're on the list: we'll e-mail you when sign-up opens.": "你已在名單上，開放註冊時我們會寄信通知你。",
  "We'll send nothing else. To be taken off the list, write to {{email}}.": "除此之外，我們不會寄送任何郵件。如需退出名單，請寄信至 {{email}}。",
  "Add another address": "再新增一個信箱",

  "Questions": "常見問題",
  "Before you choose": "選擇方案之前",
  "What happens when I use up a month's transfer?": "每月流量額度用完後會怎樣？",
  "Only what goes through our servers counts: what goes straight between your devices does not. On Free, past {{free}} GB that traffic slows to {{kb}} KB/s until the next month starts; terminals and reading still work, just more slowly. On Pro nothing slows down: what you use past {{pro}} GB is billed by use, at a price we will publish before payments open.":
    "只有經我們伺服器轉送的流量才計入，裝置之間直接傳輸的流量不計入。免費版每月轉送流量超過 {{free}} GB 後，轉送速度限制為 {{kb}} KB/s，直到下個月開始；終端機和工作階段查看仍可使用，但速度較慢。Pro 不限速，超過 {{pro}} GB 的部分按用量計費，單價將在開放付款前公布。",
  "What does “local only” mean?": "「僅本地」是什麼意思？",
  "Droppen runs on your device with no account. You use it in a browser on that device, or on another device on the same network, and nothing goes through our servers. Sign in to Droppen when you want to reach your AI from your phone anywhere, and the sessions of all your devices in one list.":
    "Droppen 在你的裝置上執行，無需帳號。在這台裝置或同一網路中其他裝置的瀏覽器上即可使用，任何內容都不經過我們的伺服器。需要在外出時從手機使用 AI，或在同一個列表中查看所有裝置的工作階段時，再登入 Droppen。",
  "Can I host it myself?": "可以自行架設嗎？",
  "Local only runs entirely on your own devices and needs nothing from us. The Droppen client is open source under the AGPL-3.0, at github.com/getdroppen/droppen. Droppen Hub, the service at droppen.app that connects your devices, is not open source yet, so it cannot be self-hosted for now.":
    "「僅本地」完全在你自己的裝置上執行，不依賴我們的任何服務。Droppen 用戶端以 AGPL-3.0 開源，原始碼位於 github.com/getdroppen/droppen。連接各台裝置的 Droppen Hub（即 droppen.app 背後的服務）尚未開源，因此目前還不能自行架設。",
  "When can I pay for Pro?": "何時可以付費開通 Pro？",
  "Not yet: payments are on their way. Until then Pro is by waitlist: join it and we'll e-mail you when Pro opens. Nothing is charged before you choose to pay, and prices are in US dollars.":
    "目前尚未開放，付款功能正在準備中。在此之前，Pro 僅接受等候名單登記，開放購買時我們會寄信通知你。在你決定付款之前，不會產生任何費用；價格以美元計。",
  "What if a device stays offline for longer?": "裝置長時間離線會怎樣？",
  "On Free your phone can read a device's last {{free}} days of sessions while it is offline, on Pro its last {{pro}}. A device offline for longer than that has its encrypted copy in the cloud deleted: what is on the device itself is not affected, and its recent sessions are uploaded again once it is back online.":
    "裝置離線期間，免費版可在手機上查看它最近 {{free}} 天的工作階段，Pro 為 {{pro}} 天。離線超過這一期限，該裝置在雲端的加密副本會被刪除；裝置上的紀錄不受影響，重新上線後會自動再次上傳最近的工作階段。",
  "Is my code safer on Pro?": "Pro 會讓程式碼更安全嗎？",
  "It is just as safe on every plan: all of them are end-to-end encrypted the same way, and we can't read your sessions, your code or your terminals on any. A plan only changes how much we keep and carry for you, never who can read it.":
    "所有方案的安全性相同：都以同樣的方式端對端加密，無論哪個方案，我們都無法讀取你的工作階段、程式碼和終端機。方案只決定我們替你保存和轉送多少資料，不改變誰能讀取這些資料。",

  "Privacy notice — Droppen": "隱私權聲明 — Droppen",
  "What we keep, and what we can't see.": "我們保存什麼，又看不到什麼",
  "Droppen is end-to-end encrypted, so there is little to tell: here is all of it.": "Droppen 是端對端加密的，所以要說明的不多，全在這裡了。",
  "Last updated {{date}}": "最後更新：{{date}}",
  "If you joined the waitlist": "如果你加入了等候名單",
  "What we keep, and why": "我們保存什麼、為什麼",
  "Your e-mail address, the plan you are waiting for (Free or Pro), the language of the page and when you joined.": "你的電子郵件地址、你在等的方案（免費版或 Pro）、頁面語言，以及加入的時間。",
  "We use them only to e-mail you an invite, or to tell you when sign-up or Pro opens. Nothing else is sent, and your address is never sold or used for marketing.":
    "只用來寄邀請給你，或在開放註冊、Pro 開放時通知你。不會寄別的信，你的信箱也絕不出售、不用於行銷。",
  "Not your IP address, nor anything else about your browser; our hosting provider's edge logs do record your IP address and User-Agent (see below). To stop abuse, our servers count requests per address and per network in memory, for a day at most.":
    "不保存你的 IP 位址，也不保存瀏覽器的任何其他資訊；不過託管服務商的邊緣日誌會記錄你的 IP 位址和 User-Agent（見下文）。為防止濫用，伺服器會在記憶體裡按位址和網路計數請求，最多保留一天。",
  "How long, and how to be taken off": "保存多久、怎麼退出",
  "Until you ask us to remove it, and never more than 12 months after you last joined: older entries are deleted automatically.":
    "保存到你要求刪除為止，最長不超過你最後一次加入後的 12 個月：超過的紀錄會自動刪除。",
  "To be taken off, write to {{email}} from that address and we will delete it.": "想退出，用那個信箱寫信到 {{email}}，我們會刪除它。",
  // Downloads (privacy notice)
  "Downloads":
    "下載",
  "If you download Droppen":
    "如果你下載了 Droppen",
  "What we count":
    "我們計數什麼",
  "When you download Droppen from droppen.app, or run its install command, our servers add one to that day's count for that download: the macOS app, a Linux build or the install script.":
    "你從 droppen.app 下載 Droppen，或執行它的安裝指令時，我們的伺服器會把這項下載當天的計數加一：macOS App、Linux 版本或安裝腳本。",
  "That number is all we keep: one count per download per day, which says nothing about who downloaded.":
    "我們只保存這個數字：每項下載每天一個計數，看不出是誰下載的。",
  "What we don't keep":
    "我們不保存什麼",
  "Not your IP address, nor anything else about your browser, and no cookie. To keep one source from inflating the counts, our servers hold the address and the network a download came from in memory, for an hour at most, and write them nowhere.":
    "不保存你的 IP 位址，也不保存瀏覽器的任何其他資訊，不使用 Cookie。為防止有人灌高計數，伺服器會把下載來自的位址和網路只留在記憶體裡，最多一小時，不寫到任何地方。",
  "Our hosting provider's edge logs do record the IP address and User-Agent of every request, downloads included (see below).":
    "不過託管服務商的邊緣日誌會記錄每個請求（包括下載）的 IP 位址和 User-Agent（見下文）。",
  "Your account": "你的帳號",
  "If you use Droppen": "如果你在用 Droppen",
  "Your sessions and terminals are encrypted on your devices, and only your devices hold the keys. Our servers pass and keep what they cannot read.":
    "你的工作階段和終端機都在你的裝置上加密，金鑰只在你的裝置裡。我們的伺服器轉送、保存的，都是它讀不了的內容。",
  "What our servers can see": "我們的伺服器看得到",
  "Your e-mail address, and when you sign in": "你的電子郵件地址，以及登入時間",
  "Which of your devices are online, when they connect and from which IP address": "你的哪些裝置在線、何時連線、從哪個 IP 位址",
  "What kind of device each one is and which browser, as a short name such as “iPhone · Chrome” made from its User-Agent; our servers do not keep the User-Agent itself, though our hosting provider's edge logs do (see below)":
    "每台裝置的類型和瀏覽器，只記一個簡稱，例如「iPhone · Chrome」，由它的 User-Agent 得出；我們的伺服器從不保存 User-Agent 本身，不過託管服務商的邊緣日誌會記錄它（見下文）",
  "How much data moves and when. What we store is padded to fixed sizes; live connections are not, so while our servers carry one they see its timing, its volume and the rhythm of typing in a terminal: roughly when you are working": "傳了多少資料、什麼時候傳的。我們保存的資料會填充到固定大小；即時連線不做填充，所以經我們的伺服器轉送時，它的時序、流量和終端機裡的打字節奏都看得到：也就是你大概什麼時候在工作",
  "Your plan, and how much of it you use": "你的方案，以及用了多少",
  "What they cannot see": "我們的伺服器看不到",
  "What is said in a session, or typed and shown in a terminal": "工作階段裡說了什麼，終端機裡輸入和顯示了什麼",
  "Session titles, folders, repositories and branches": "工作階段標題、資料夾、儲存庫和分支",
  "Model names, which AI tool you use, your AI accounts and their quotas": "模型名稱、你用哪個 AI 工具、你的 AI 帳號和額度",
  "The names of your devices, your files and their names": "裝置名稱、你的檔案以及檔名",
  "Providers and retention": "服務商與保存期限",
  "Who else handles it, and for how long": "還有誰經手，保存多久",
  "These services run parts of Droppen for us. Like our servers, none of them can read what is end-to-end encrypted.":
    "這些服務替我們運行 Droppen 的一部分。和我們的伺服器一樣，它們都讀不了端對端加密的內容。",
  "Our providers": "我們的服務商",
  "Railway, in Singapore, runs our servers. Its edge logs record the IP address and User-Agent of every request to droppen.app, and keep them for as long as Railway keeps its logs.":
    "Railway（位於新加坡）託管我們的伺服器。它的邊緣日誌會記錄每個發往 droppen.app 的請求的 IP 位址和 User-Agent，並依 Railway 自己的日誌保存期限保留。",
  "PlanetScale runs our database (Postgres), in Singapore.":
    "PlanetScale 替我們運行資料庫（Postgres），位於新加坡。",
  "Tigris stores the encrypted pages of your sessions.": "Tigris 儲存你工作階段的加密頁面。",
  "Cloudflare runs our DNS and routes mail to our addresses. It also runs one of the STUN servers for direct connections (Google runs the other): a STUN server sees your device's public IP address.":
    "Cloudflare 負責我們的 DNS，並把寄往我們地址的郵件轉給我們。直連用的 STUN 伺服器也有一台是它的（另一台是 Google 的）：STUN 伺服器看得到你裝置的公網 IP 位址。",
  "An e-mail relay service we operate sends our e-mail, such as the verification mail, to your address.":
    "我們自己營運的一個郵件轉發服務負責把我們的郵件（例如驗證信）寄到你的信箱。",
  "How long we keep it": "保存多久",
  "The encrypted copies of your sessions: the last {{free}} days on Free, {{pro}} on Pro, and {{internal}} on the internal plan, which is not for sale. A device offline for longer than that has its copy deleted.":
    "你工作階段的加密副本：免費版保存最近 {{free}} 天，Pro 保存 {{pro}} 天，不對外銷售的內部方案保存 {{internal}} 天。裝置離線超過這個天數，它的副本會被刪除。",
  "An account whose e-mail address was never verified, and that never added a device, is deleted after {{days}} days.":
    "信箱從未驗證、也從未新增過裝置的帳號，會在 {{days}} 天後刪除。",
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
  "Deleting removes all this from our live systems at once. Copies in our backups and logs expire within {{days}} days.":
    "刪除會立即把這些從我們的線上系統中移除。備份和日誌中的副本會在 {{days}} 天內過期。",
};

export const pricingJa: Bundle = {
  "Pricing": "料金",
  "Privacy notice": "プライバシーについて",
  "Terms of Service": "利用規約",
  "How the encryption works, in detail": "暗号化のしくみ（詳しく）",
  "No code yet? Join the waitlist": "招待コードがない方はウェイトリストへ",

  "Droppen pricing — start free, Pro for ${{price}} a month": "Droppen の料金 — 無料から、Pro は月 ${{price}}",
  "Use Droppen on one device with no account, free with an invite, or Pro for more devices and longer offline reading. Every plan is end-to-end encrypted.":
    "1 台ならアカウントなしで、招待があれば無料で、もっと多くのデバイスと長いオフライン閲覧なら Pro で。どのプランもエンドツーエンド暗号化です。",
  "Use Droppen on one device with no account, free with an account, or Pro for more devices and longer offline reading. Every plan is end-to-end encrypted.":
    "1 台ならアカウントなしで、アカウントがあれば無料で、もっと多くのデバイスと長いオフライン閲覧なら Pro で。どのプランもエンドツーエンド暗号化です。",
  "Start free. Go Pro when you need more.": "無料で始めて、必要なら Pro へ。",
  "Every plan is end-to-end encrypted: on none of them can we read your sessions, your code or your terminals. A plan decides only how many devices you can connect, how many days of sessions your phone can read while a device that runs AI is offline, and how much traffic goes through our servers each month.":
    "どのプランもエンドツーエンド暗号化で、どのプランでも私たちはあなたのセッション、コード、ターミナルを読めません。プランで決まるのは 3 つだけです。接続できるデバイスの数、AI を実行できるデバイスがオフラインのときにスマホで読めるセッションの日数、そして私たちのサーバーを経由する毎月の転送量です。",
  "Prices in US dollars. Pro is not on sale yet, and nothing is charged before you choose to pay.":
    "価格は米ドル建てです。Pro はまだ販売していません。ご自身で支払うと決めるまで、料金は一切かかりません。",

  "End-to-end encrypted: we can't read your sessions, code or terminals": "エンドツーエンド暗号化：セッション、コード、ターミナルは私たちにも読めません",
  "Local only": "ローカルのみ",
  "Recommended": "おすすめ",
  "forever": "ずっと無料",
  "No account, nothing to sign up for": "アカウント不要、登録もいりません",
  "Droppen on the device your AI runs on, and nothing in the cloud.": "Droppen は AI が動くデバイスの中だけで動き、クラウドには何も保存しません。",
  "1 device that runs AI: the one Droppen is installed on": "AI を実行できるデバイス 1 台：Droppen をインストールしたデバイス",
  "Use it in a browser on that device, or on another device on the same network": "そのデバイスか、同じネットワークにある別のデバイスのブラウザで使える",
  "Nothing goes through our servers": "私たちのサーバーを一切通らない",
  "Free": "無料",
  "USD / month": "USD / 月",
  "Sign-up is by invite for now": "現在は招待コードで登録",
  "Sign-up is closed for now": "現在は新規登録を停止中",
  "Sign up with an e-mail address": "メールアドレスで登録",
  "Sign up": "新規登録",
  "Join the waitlist": "ウェイトリストに登録",
  "You have an account already.": "アカウントはすでにお持ちです。",
  "Have an invite code? Sign up": "招待コードをお持ちなら登録へ",
  "Your AI on your phone, wherever you are.": "どこにいても、スマホからあなたの AI を使えます。",
  "{{n}} devices that run AI": "AI を実行できるデバイス {{n}} 台",
  "{{n}} devices in all, including those that run AI, phones and browsers": "デバイスは全部で {{n}} 台（AI を実行できるデバイス、スマホ、ブラウザを含む）",
  "While a device that runs AI is offline, read its last {{n}} days of sessions on your phone": "AI を実行できるデバイスがオフラインでも、直近 {{n}} 日分のセッションをスマホで読める",
  "{{n}} GB a month through our servers, then {{kb}} KB/s until the next month": "私たちのサーバー経由の転送は月 {{n}} GB まで。超えると翌月まで {{kb}} KB/s",
  "Waitlist": "ウェイトリスト",
  "or ${{price}} a year: save {{saving}}%": "年払いなら ${{price}}（{{saving}}% お得）",
  "Payments are not open yet: we'll e-mail you when they are.": "お支払いはまだ受け付けていません。始まったらメールでお知らせします。",
  "Everything in Free, with higher limits.": "無料プランのすべての機能を、より大きな上限で。",
  "{{n}} GB a month through our servers, then billed by use, at full speed": "私たちのサーバー経由の転送は月 {{n}} GB。超えた分も速度はそのままで、従量課金",

  "Compare": "比較",
  "What each plan includes": "各プランの内容",
  "included": "含まれる",
  "Price": "料金",
  "Free forever, no account": "ずっと無料、アカウント不要",
  "Free; sign-up by invite for now": "無料（現在は招待制）",
  "Free; sign-up is closed for now": "無料（現在は登録停止中）",
  "Free, with an account": "無料（アカウントが必要）",
  "${{monthly}} a month, or ${{yearly}} a year": "月 ${{monthly}}、または年 ${{yearly}}",
  "Devices that run AI": "AI を実行できるデバイス",
  "1: the device Droppen is installed on": "1 台：Droppen をインストールしたデバイス",
  "All your devices, including those that run AI, phones and browsers": "デバイスの総数（AI を実行できるデバイス、スマホ、ブラウザを含む）",
  "That device, and browsers on the same network": "そのデバイスと、同じネットワークのブラウザ",
  "Days of sessions your phone can read while a device that runs AI is offline": "AI を実行できるデバイスがオフラインのとき、スマホで読めるセッションの日数",
  "None: nothing is kept in the cloud": "なし：クラウドには何も保存しません",
  "{{n}} days": "直近 {{n}} 日",
  "A month's transfer through our servers": "私たちのサーバー経由の月間転送量",
  "{{n}} GB, then {{kb}} KB/s": "{{n}} GB、超えると {{kb}} KB/s",
  "{{n}} GB, then billed by use": "{{n}} GB、超えた分は従量課金",
  "Free keeps an encrypted copy of each device's last {{free}} days of sessions in the cloud, Pro its last {{pro}}. A device offline for longer than that has its cloud copy deleted: what is on the device is not affected, and those days are uploaded again once it is back online.":
    "無料プランは各デバイスの直近 {{free}} 日分のセッションを、Pro は直近 {{pro}} 日分を、暗号化したコピーとしてクラウドに保存します。それより長くオフラインのデバイスは、クラウド上のコピーが削除されます。デバイス内の記録はそのままで、オンラインに戻るとその期間のセッションが自動で再アップロードされます。",

  "Leave your address, and we'll write when it's your turn": "メールアドレスを登録してください。順番が来たらメールでお知らせします",
  "Leave your address, and we'll write when sign-up or Pro opens": "メールアドレスを登録してください。新規登録の受付を再開したとき、または Pro の販売を始めたときにお知らせします",
  "Leave your address, and we'll write when Pro opens": "メールアドレスを登録してください。Pro の販売を始めたらお知らせします",
  "Pro is not on sale yet: payments are on their way. Submitting the form is all it takes.": "Pro はまだ販売していません。お支払いの仕組みを準備中です。メールアドレスを送信すれば登録は完了です。",
  "Sign-up is closed for now, and Pro is not on sale yet. Pick what you are waiting for: we'll e-mail you when sign-up opens, or when Pro does. Submitting the form is all it takes.":
    "現在は新規登録を停止しており、Pro もまだ販売していません。お待ちの項目を選んでください。新規登録の受付を再開したとき、または Pro の販売を始めたときにメールでお知らせします。メールアドレスを送信すれば登録は完了です。",
  "Sign-up is invite-only while Droppen is in beta, and Pro is not on sale yet. Pick what you are waiting for: we'll e-mail you an invite, or tell you when Pro opens. Submitting the form is all it takes.":
    "Droppen は招待制のベータ版のため、登録には招待コードが必要です。Pro もまだ販売していません。お待ちの項目を選んでください。招待をメールでお送りするか、Pro の販売開始をお知らせします。メールアドレスを送信すれば登録は完了です。",
  "Plan": "プラン",
  "${{price}} / month": "${{price}} / 月",
  "E-mail address": "メールアドレス",
  "No confirmation mail and no newsletter: we use your address only to send what you asked for.": "確認メールもマーケティングメールも送りません。アドレスは、登録いただいたお知らせを送るためだけに使います。",
  "That does not look like an e-mail address. Check it and try again.": "メールアドレスの形式ではないようです。確認してもう一度お試しください。",
  "Use an address you keep: throwaway mail providers are not accepted.": "普段お使いのアドレスを入力してください。使い捨てメールは受け付けていません。",
  "Too many tries from your network. Try again in {{n}} min.": "このネットワークからの試行が多すぎます。{{n}} 分後にもう一度お試しください。",
  "Could not reach Droppen. Check your connection and try again.": "Droppen に接続できませんでした。接続を確認してもう一度お試しください。",
  "You're on the list: we'll e-mail you when Pro opens.": "登録しました。Pro が始まったらメールでお知らせします。",
  "You're on the list: we'll e-mail you an invite.": "登録しました。招待をメールでお送りします。",
  "You're on the list: we'll e-mail you when sign-up opens.": "登録しました。新規登録の受付を再開したらメールでお知らせします。",
  "We'll send nothing else. To be taken off the list, write to {{email}}.": "それ以外のメールは送りません。リストから外れたい場合は {{email}} までご連絡ください。",
  "Add another address": "別のアドレスを登録",

  "Questions": "よくある質問",
  "Before you choose": "プランを選ぶ前に",
  "What happens when I use up a month's transfer?": "月間の転送量を使い切ったら？",
  "Only what goes through our servers counts: what goes straight between your devices does not. On Free, past {{free}} GB that traffic slows to {{kb}} KB/s until the next month starts; terminals and reading still work, just more slowly. On Pro nothing slows down: what you use past {{pro}} GB is billed by use, at a price we will publish before payments open.":
    "数えるのは私たちのサーバーを経由した通信だけで、デバイス同士で直接やり取りした分は含みません。無料プランでは、月の転送量が {{free}} GB を超えると、翌月になるまでその通信が {{kb}} KB/s に制限されます。ターミナルもセッションの閲覧も使えますが、遅くなります。Pro は速度制限がなく、{{pro}} GB を超えた分は従量課金です。単価はお支払いの受付開始前に公表します。",
  "What does “local only” mean?": "「ローカルのみ」とは？",
  "Droppen runs on your device with no account. You use it in a browser on that device, or on another device on the same network, and nothing goes through our servers. Sign in to Droppen when you want to reach your AI from your phone anywhere, and the sessions of all your devices in one list.":
    "アカウントなしで、Droppen をデバイス上で動かす使い方です。そのデバイスか、同じネットワークにある別のデバイスのブラウザで使い、私たちのサーバーは一切通りません。外出先からスマホで AI を使いたいとき、すべてのデバイスのセッションをひとつのリストで見たいときに、Droppen にサインインします。",
  "Can I host it myself?": "セルフホストできますか？",
  "Local only runs entirely on your own devices and needs nothing from us. The Droppen client is open source under the AGPL-3.0, at github.com/getdroppen/droppen. Droppen Hub, the service at droppen.app that connects your devices, is not open source yet, so it cannot be self-hosted for now.":
    "「ローカルのみ」は完全にご自身のデバイスで動き、私たちのサービスを何も必要としません。Droppen のクライアントは AGPL-3.0 のオープンソースで、ソースコードは github.com/getdroppen/droppen にあります。デバイス同士をつなぐ Droppen Hub（droppen.app で動くサービス）はまだオープンソースではないため、現時点ではセルフホストできません。",
  "When can I pay for Pro?": "Pro はいつ購入できますか？",
  "Not yet: payments are on their way. Until then Pro is by waitlist: join it and we'll e-mail you when Pro opens. Nothing is charged before you choose to pay, and prices are in US dollars.":
    "まだです。お支払いの仕組みを準備中で、それまで Pro はウェイトリストのみの受付です。販売を始めたらメールでお知らせします。ご自身で支払うと決めるまで料金は一切かかりません。価格は米ドル建てです。",
  "What if a device stays offline for longer?": "デバイスが長くオフラインのままだと？",
  "On Free your phone can read a device's last {{free}} days of sessions while it is offline, on Pro its last {{pro}}. A device offline for longer than that has its encrypted copy in the cloud deleted: what is on the device itself is not affected, and its recent sessions are uploaded again once it is back online.":
    "デバイスがオフラインの間、無料プランなら直近 {{free}} 日分、Pro なら {{pro}} 日分のセッションをスマホで読めます。それより長くオフラインのままだと、クラウド上の暗号化コピーは削除されます。デバイス内の記録はそのままで、オンラインに戻ると最近のセッションが自動で再アップロードされます。",
  "Is my code safer on Pro?": "Pro のほうがコードは安全ですか？",
  "It is just as safe on every plan: all of them are end-to-end encrypted the same way, and we can't read your sessions, your code or your terminals on any. A plan only changes how much we keep and carry for you, never who can read it.":
    "どのプランでも安全性は同じです。すべて同じ方式でエンドツーエンド暗号化され、どのプランでも私たちはあなたのセッション、コード、ターミナルを読めません。プランで変わるのは、私たちが保存・転送する量だけで、誰が読めるかは変わりません。",

  "Privacy notice — Droppen": "プライバシーについて — Droppen",
  "What we keep, and what we can't see.": "私たちが保管するもの、見えないもの。",
  "Droppen is end-to-end encrypted, so there is little to tell: here is all of it.": "Droppen はエンドツーエンド暗号化なので、お伝えすることは多くありません。すべてここに書きます。",
  "Last updated {{date}}": "最終更新：{{date}}",
  "If you joined the waitlist": "ウェイトリストに登録した方へ",
  "What we keep, and why": "保管するものとその理由",
  "Your e-mail address, the plan you are waiting for (Free or Pro), the language of the page and when you joined.": "メールアドレス、お待ちのプラン（無料または Pro）、ページの言語、登録した日時。",
  "We use them only to e-mail you an invite, or to tell you when sign-up or Pro opens. Nothing else is sent, and your address is never sold or used for marketing.":
    "用途は招待のお送りと、新規登録の受付再開や Pro 開始のお知らせだけです。それ以外のメールは送らず、アドレスを販売したりマーケティングに使ったりすることもありません。",
  "Not your IP address, nor anything else about your browser; our hosting provider's edge logs do record your IP address and User-Agent (see below). To stop abuse, our servers count requests per address and per network in memory, for a day at most.":
    "IP アドレスやブラウザに関する情報は保管しません。ただし、ホスティング事業者のエッジのログには IP アドレスと User-Agent が記録されます（下記参照）。不正利用を防ぐため、サーバーはアドレスごと・ネットワークごとのリクエスト数をメモリ上で数えますが、保持は最長 1 日です。",
  "How long, and how to be taken off": "保管期間と削除の方法",
  "Until you ask us to remove it, and never more than 12 months after you last joined: older entries are deleted automatically.":
    "削除のご依頼をいただくまで保管しますが、最後に登録してから 12 か月を超えることはありません。それより古い記録は自動で削除されます。",
  "To be taken off, write to {{email}} from that address and we will delete it.": "削除をご希望の場合は、登録したアドレスから {{email}} までご連絡ください。削除いたします。",
  // Downloads (privacy notice)
  "Downloads":
    "ダウンロード",
  "If you download Droppen":
    "Droppen をダウンロードした方へ",
  "What we count":
    "数えるもの",
  "When you download Droppen from droppen.app, or run its install command, our servers add one to that day's count for that download: the macOS app, a Linux build or the install script.":
    "droppen.app から Droppen をダウンロードしたとき、またはインストールコマンドを実行したとき、サーバーはそのダウンロードのその日の件数に 1 を足します。対象は macOS アプリ、Linux 版、インストールスクリプトです。",
  "That number is all we keep: one count per download per day, which says nothing about who downloaded.":
    "保管するのはこの数だけです。ダウンロードごと、1 日ごとの件数で、誰がダウンロードしたかはわかりません。",
  "What we don't keep":
    "保管しないもの",
  "Not your IP address, nor anything else about your browser, and no cookie. To keep one source from inflating the counts, our servers hold the address and the network a download came from in memory, for an hour at most, and write them nowhere.":
    "IP アドレスやブラウザに関する情報は保管せず、Cookie も使いません。件数を水増しされないよう、サーバーはダウンロード元のアドレスとネットワークをメモリ上でだけ最長 1 時間保持し、どこにも書き込みません。",
  "Our hosting provider's edge logs do record the IP address and User-Agent of every request, downloads included (see below).":
    "ただし、ホスティング事業者のエッジのログには、ダウンロードを含むすべてのリクエストの IP アドレスと User-Agent が記録されます（下記参照）。",
  "Your account": "アカウント",
  "If you use Droppen": "Droppen をお使いの方へ",
  "Your sessions and terminals are encrypted on your devices, and only your devices hold the keys. Our servers pass and keep what they cannot read.":
    "セッションとターミナルはあなたのデバイス上で暗号化され、鍵を持つのはあなたのデバイスだけです。私たちのサーバーが転送・保管するのは、サーバー自身には読めないものです。",
  "What our servers can see": "私たちのサーバーに見えるもの",
  "Your e-mail address, and when you sign in": "メールアドレスとログインの時刻",
  "Which of your devices are online, when they connect and from which IP address": "どのデバイスがオンラインか、いつ、どの IP アドレスから接続したか",
  "What kind of device each one is and which browser, as a short name such as “iPhone · Chrome” made from its User-Agent; our servers do not keep the User-Agent itself, though our hosting provider's edge logs do (see below)":
    "各デバイスの種類とブラウザ（「iPhone · Chrome」のような短い名前。User-Agent から作り、User-Agent そのものは私たちのサーバーには保存しません。ホスティング事業者のエッジのログには記録されます。下記参照）",
  "How much data moves and when. What we store is padded to fixed sizes; live connections are not, so while our servers carry one they see its timing, its volume and the rhythm of typing in a terminal: roughly when you are working": "どれだけのデータがいつ流れたか。保存するデータは決まったサイズまでパディングしますが、ライブの接続はしません。そのため私たちのサーバーを通る間は、そのタイミング、量、ターミナルでの打鍵のリズムが見えます。つまり、だいたいいつ作業しているか",
  "Your plan, and how much of it you use": "契約プランと、その使用量",
  "What they cannot see": "見えないもの",
  "What is said in a session, or typed and shown in a terminal": "セッションでのやり取り、ターミナルの入力と出力",
  "Session titles, folders, repositories and branches": "セッションのタイトル、フォルダ、リポジトリ、ブランチ",
  "Model names, which AI tool you use, your AI accounts and their quotas": "モデル名、どの AI ツールを使っているか、AI のアカウントと使用枠",
  "The names of your devices, your files and their names": "デバイス名、ファイルとファイル名",
  "Providers and retention": "委託先と保管期間",
  "Who else handles it, and for how long": "ほかに誰が扱い、どれだけ保管するか",
  "These services run parts of Droppen for us. Like our servers, none of them can read what is end-to-end encrypted.":
    "これらのサービスが、Droppen の一部を私たちに代わって動かしています。私たちのサーバーと同じく、どれもエンドツーエンド暗号化された内容は読めません。",
  "Our providers": "委託先",
  "Railway, in Singapore, runs our servers. Its edge logs record the IP address and User-Agent of every request to droppen.app, and keep them for as long as Railway keeps its logs.":
    "Railway（シンガポール）が私たちのサーバーをホストしています。そのエッジのログには、droppen.app へのすべてのリクエストの IP アドレスと User-Agent が記録され、Railway 自身のログ保存期間のあいだ保管されます。",
  "PlanetScale runs our database (Postgres), in Singapore.":
    "PlanetScale が、私たちのデータベース（Postgres）をシンガポールで運用しています。",
  "Tigris stores the encrypted pages of your sessions.": "Tigris が、セッションの暗号化されたページを保存しています。",
  "Cloudflare runs our DNS and routes mail to our addresses. It also runs one of the STUN servers for direct connections (Google runs the other): a STUN server sees your device's public IP address.":
    "Cloudflare が私たちの DNS を運用し、私たちのアドレス宛てのメールを転送しています。直結に使う STUN サーバーの一つも Cloudflare のものです（もう一つは Google）。STUN サーバーには、あなたのデバイスのパブリック IP アドレスが見えます。",
  "An e-mail relay service we operate sends our e-mail, such as the verification mail, to your address.":
    "私たちが運用するメール配信サービスが、確認メールなど私たちからのメールをあなたのアドレスに送っています。",
  "How long we keep it": "保管期間",
  "The encrypted copies of your sessions: the last {{free}} days on Free, {{pro}} on Pro, and {{internal}} on the internal plan, which is not for sale. A device offline for longer than that has its copy deleted.":
    "セッションの暗号化コピー：無料プランは直近 {{free}} 日分、Pro は {{pro}} 日分、一般には提供していない内部プランは {{internal}} 日分を保管します。それより長くオフラインのデバイスは、そのコピーが削除されます。",
  "An account whose e-mail address was never verified, and that never added a device, is deleted after {{days}} days.":
    "メールアドレスが一度も確認されず、デバイスも追加されなかったアカウントは、{{days}} 日後に削除されます。",
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
  "Deleting removes all this from our live systems at once. Copies in our backups and logs expire within {{days}} days.":
    "削除すると、これらは稼働中のシステムからすぐに消えます。バックアップとログに残る写しは {{days}} 日以内に期限切れになります。",
};
