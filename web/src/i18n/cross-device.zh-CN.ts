// The zh-CN table of `cross-device.ts`, which says what it covers: a file of its
// own, so that it is in the zh-CN chunk alone (`lang-zh-CN.ts`).

export const crossDeviceZhCN: Record<string, string> = {
  "Screens are kept on the main device, which is the same on all your devices: with {{new}} as the main device, the screens on {{old}} are out of sight on all of them until you switch back to {{old}}.": "屏幕布局存在主设备上，这一设置对你的所有设备生效：换成 {{new}} 后，你的每台设备上都会暂时看不到 {{old}} 上的屏幕，换回 {{old}} 即可恢复。",
  "This pane is on another of your devices. Open this screen on droppen.app to see it.": "这个窗格属于你的另一台设备。在 droppen.app 上打开这个屏幕即可看到。",
  "this pane could not be drawn": "这个窗格无法显示",
  "Font size and language follow the main device: change them in its settings.": "字号和语言跟随主设备，在主设备的设置里改。",
  "main device settings": "主设备的设置",
  "main device": "主设备",
  "set as main device": "设为主设备",
  "The main device keeps your screens and layouts.": "主设备保存你的屏幕与布局。",
  "this device is not on this page": "这台设备不在本页",
  "It may have been removed from the account, or the link is from another account.": "它可能已从账户中移除，或者这个链接来自另一个账户。",
  "offline — a new session needs it online": "离线 — 新建会话需要它在线",
  "waiting for another of your devices to confirm": "等待你的另一台设备确认",
  "this device does not accept control from this browser": "这台设备不接受这个浏览器的控制",
  "on this device: {{name}}": "所在设备：{{name}}",
  "The device that keeps this screen runs an older Droppen and cannot keep panes of other devices: they went back to it. Update it, then choose them again.":
    "保存这个屏幕的设备上 Droppen 版本较旧，存不下其它设备的窗格，这些窗格已回到它自己。升级它之后再重新选择。",
  // The local app showing other devices through its gateway (S1-7, features/gateway, the sessions list).
  "read-only here: this session is on another device": "只读：这个会话在另一台设备上",
  "there is no copy of this session on this device yet": "这台设备上还没有这个会话的副本",
  "this needs that device itself — its sessions are read-only here": "这需要那台设备本身——它的会话在这里只读",
  "show the sessions of": "显示哪些设备的会话",
  "read-only here: these sessions are on another device": "只读：这些会话在另一台设备上",
  "read-only here: start new sessions on that device itself": "在这里只读：请在那台设备上新建会话",
  "read-only here": "在这里只读",
};
