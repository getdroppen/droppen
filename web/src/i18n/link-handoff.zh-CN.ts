// The zh-CN table of `link-handoff.ts`, which says what it covers: a file of its
// own, so that it is in the zh-CN chunk alone (`lang-zh-CN.ts`).

export const linkHandoffZhCN: Record<string, string> = {
  // The device's page.
  "Check that the fingerprint there matches this one, then add this device.": "核对那边显示的指纹和这里一致，然后加入这台设备。",
  "Come back here: this device joins your account by itself.": "回到这里，这台设备会自己加入你的账户。",
  "waiting for droppen.app… the link works for another {{left}}": "等待 droppen.app… 链接还能用 {{left}}",
  "waiting for droppen.app…": "等待 droppen.app…",
  "open the browser again": "重新打开浏览器",
  "copy link": "复制链接",
  "The wrong browser opened? Copy the link into the one you use droppen.app in, on this device.": "打开的不是你常用的浏览器？复制链接，粘贴到这台设备上你登录 droppen.app 的那个浏览器里。",
  "Whoever opens this link can add this device to their own Droppen account: never send it to anyone, and open it only on this device.":
    "谁打开这个链接，谁就能把这台设备加进自己的 Droppen 账户：不要发给任何人，只在这台设备上打开。",
  "continue in a browser tab instead": "改在浏览器标签页里继续",
  // droppen.app's /link.
  "handing the code to the device…": "正在把注册码交给设备…",
  "Added {{id}}. Go back to Droppen on that device: it comes online in a moment. You can close this tab.":
    "已加入 {{id}}。回到那台设备上的 Droppen，它马上就会上线。这个标签页可以关掉了。",
  "This sign-in link has expired. Start again from Droppen on the device: Settings → Access → Droppen → Sign in to Droppen.":
    "这个登录链接已过期。请在那台设备的 Droppen 里重新开始：设置 → 访问 → Droppen → 登录 Droppen。",
  "Cancelled: nothing was added. You can close this tab.": "已取消，没有加入任何设备。这个标签页可以关掉了。",
  "Check that Droppen on that device shows the same fingerprint. When you add it, a new tab hands the code to that device, then closes.":
    "请核对那台设备上的 Droppen 显示的是同一串指纹。加入时会打开一个新标签页，把注册码交给那台设备，然后自动关闭。",
  "The browser did not open the tab that hands the code to the device. Open it here:": "浏览器没有打开把注册码交给设备的标签页。请在这里打开：",
  "hand the code to the device": "把注册码交给设备",
  "If the device's tab says it did not take the code, cancel here and start again from Droppen on the device.":
    "如果那台设备的标签页说没有收下注册码，就在这里取消，然后在那台设备的 Droppen 里重新开始。",
  // The device's /link/return.
  "Signing in to Droppen": "登录 Droppen",
  "handing the sign-in to Droppen on this device…": "正在把登录交给这台设备上的 Droppen…",
  "This device has the sign-in.": "这台设备已收到登录。",
  "droppen.app, in the other tab, finishes adding it. You can close this tab and go back to Droppen.":
    "droppen.app 会在另一个标签页里完成加入。这个标签页可以关掉，回到 Droppen 即可。",
  "Nothing to hand over here. To sign this device in, open Droppen on it: Settings → Access → Droppen → Sign in to Droppen.":
    "这里没有要交接的内容。要让这台设备登录，请在它上面打开 Droppen：设置 → 访问 → Droppen → 登录 Droppen。",
  "This sign-in ran out of time. Start again from Droppen on this device.": "这次登录已超时。请在这台设备的 Droppen 里重新开始。",
  "This sign-in is no longer open on this device: it was used, cancelled or started again. If it did not finish, start again from Droppen on this device.":
    "这台设备上已没有这次登录：它已被使用、取消或重新开始。如果没有完成，请在这台设备的 Droppen 里重新开始。",
  "This answer does not belong to the sign-in open on this device. Nothing was added.": "这个答复不属于这台设备上正在进行的登录。没有加入任何设备。",
  "This device is in a Droppen account already.": "这台设备已经在一个 Droppen 账户里了。",
  "This device could not reach droppen.app. Check its connection, then start again from Droppen on this device.":
    "这台设备连不上 droppen.app。请检查网络，然后在这台设备的 Droppen 里重新开始。",
  "Droppen on this device did not answer. Is it still running?": "这台设备上的 Droppen 没有响应。它还在运行吗？",
  "This device could not take the sign-in: {{message}}": "这台设备没能接收登录：{{message}}",
};
