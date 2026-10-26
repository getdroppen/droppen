// The zh-CN table of `setup.ts`, which says what it covers: a file of its
// own, so that it is in the zh-CN chunk alone (`lang-zh-CN.ts`).

export const setupZhCN: Record<string, string> = {
  "setup code": "设置码",
  "confirm password": "再输一次密码",
  "create account and sign in": "创建账号并登录",
  "creating account…": "正在创建账号…",
  "Or create the account on this device, without a code:": "或者不用设置码，直接在这台设备上创建账号：",
  "a setup code is 16 letters and digits, like ABCD-EFGH-JKMN-PQRS": "设置码由 16 个字母和数字组成，形如 ABCD-EFGH-JKMN-PQRS",
  "a user name is 1-32 characters of a-z, 0-9, '.', '_' or '-'": "用户名为 1–32 个字符，只能用 a-z、0-9、「.」「_」或「-」",
  "a password is at least 12 characters long": "密码至少要 12 个字符",
  "a password is at most 256 bytes long": "密码最多 256 字节",
  "a password may not start or end with whitespace": "密码不能以空白字符开头或结尾",
  "a password may not be the user name": "密码不能与用户名相同",
  "that password is one of the most common ones; pick another": "这个密码太常见了，换一个",
  "That is not the setup code this device's server printed. `{{command}} setup-code` on this device shows it again.":
    "这不是这台设备的服务端打印的设置码。在这台设备上运行 `{{command}} setup-code` 可以再看一次。",
  "The first account is made on this device's own page, not through Droppen.": "第一个账号要在这台设备自己的页面上创建，不能经由 Droppen。",
  "This device already has an account.": "这台设备已经有账号了。",
  "Local login is switched off on this device (auth.enabled = false), so there is no account to create here.":
    "这台设备关闭了本地登录（auth.enabled = false），这里没有要创建的账号。",
  "This device's server has no setup code. Restart it to get one, or create the account on this device with `{{command}} user add <name>`.":
    "这台设备的服务端没有设置码。重启服务端即可得到一个，或者在这台设备上用 `{{command}} user add <名字>` 创建账号。",
  "Too many wrong setup codes. Try again in {{seconds}} s.": "错误的设置码太多了，请 {{seconds}} 秒后再试。",
  "Too many wrong setup codes. Wait a little, then try again.": "错误的设置码太多了，请稍等再试。",
  /* settings › security, when local login is switched off */
  "switched off": "已关闭",
  "Local login is switched off on this device (auth.enabled = false), so its page answers anyone who can reach it. To require a login, remove this setting from the server's configuration and restart the server:":
    "这台设备关闭了本地登录（auth.enabled = false），任何能连上它的人都能访问它的页面。要启用登录，从服务端配置里去掉这项设置，再重启服务端：",
  "Then make its first account on this device:": "然后在这台设备上创建它的第一个账号：",
  "Could not ask this device's server about its login.": "无法向这台设备的服务端查询登录状态。",
  /* settings › security, this device's owner while it has no account */
  "no account yet": "还没有账号",
  /* the setup page: nobody signed in, no account yet */
  "This device has no account yet.": "这台设备还没有账号。",
  "This is the Droppen app's own server. In the app's menu-bar menu, choose Open in Browser: it opens this page signed in, and the first account is made there.":
    "这是 Droppen App 自己的服务端。在 App 的菜单栏菜单里选「在浏览器中打开」:它会以已登录的状态打开这个页面,第一个账号就在那里创建。",
  "On this device: open Droppen from the app, or run `{{command}} open` in a terminal — it opens this page signed in.":
    "在这台设备上：从 App 打开 Droppen，或在终端里运行 `{{command}} open`，它会以已登录的状态打开这个页面。",
  "Or create the first account with the one-time setup code this device's server keeps (`{{command}} setup-code` on this device shows it)":
    "或者用这台设备的服务端保存的一次性设置码创建第一个账号（在这台设备上运行 `{{command}} setup-code` 可以看到）",
  "That link was already used or has expired. Run `{{command}} open` on this device again.":
    "这个链接已经用过或已过期。请在这台设备上重新运行 `{{command}} open`。",
  "This device has an account now: sign in.": "这台设备现在已经有账号了，请登录。",
  "That link works only on the device itself, not through Droppen.": "这个链接只能在这台设备本身上使用，不能经由 Droppen。",
  /* the first account, made by this device's owner without the code */
  "You opened this page as this device's owner. This device has no account yet: create one to sign in from other devices; from then on every browser signs in, this one too.":
    "你以这台设备主人的身份打开了这个页面。这台设备还没有账号：创建一个，就能从其它设备登录；从那以后每个浏览器都要登录，包括这一个。",
  "The setup code is needed now. `{{command}} setup-code` on this device shows it, or run `{{command}} open` there again.":
    "现在需要设置码。在这台设备上运行 `{{command}} setup-code` 可以查看，或者在那里重新运行 `{{command}} open`。",
  "That name belongs to the Droppen app's own account on this device. Pick another.": "这个名字已被这台设备上 Droppen App 自己的账号占用，请换一个。",
};
