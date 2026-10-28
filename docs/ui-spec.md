# remote-hand UI 规范

以当前应用主流取值为基准，统一同用途的界面，不改变整体紧凑程度。以下 px 数字均为默认字号、根字号 16px 的边框盒尺寸；Tailwind rem 值随 `small/default/large/xlarge` 缩放。视觉语言为「制图纸」：一种墨、1px 细线、直角、颜色只表示状态；第 7 节是可机检的规则。

## 1. 主题与文字

- 颜色源为 `web/src/index.css` 的 light / `.dark` 变量：`bg`、`panel`、`raised`、`hover`、`border`、`border-strong`、`fg`、`muted`、`faint`、`mark`。`panel` 与 `bg` 同值：区域靠细线分开，不靠底色块；`raised` 只用于井（用户消息带、代码/输出块、只读输入）。
- 没有蓝色强调色：`accent` 的值等于 `fg`（墨色），主操作是实心墨块，焦点是墨色。状态色是唯一的色相：工作中/活着 `ok`，在问你 `warn`（实心底）与 `warn-fg`（作文字），失败/离线/破坏操作 `danger`，未读 `unread`（= 墨色实心方块）。CLI 来源 `claude` / `codex` 用中性文字标签区分，不着色。这些语义不能互换，也不能拿状态色做装饰。
- 字体只用系统栈，不加载 webfont：sans = `ui-sans-serif, system-ui, -apple-system, "PingFang SC", …`；mono = `ui-monospace, "SF Mono", …`。内容使用 sans；界面标签、路径、时间、数字和工具栏使用 mono。正文主流 12–13px，transcript 正文 13px / 21px 且段落最大宽 48em；辅助标签 `text-2xs`（11px/16px），界面内容 `text-xs`（12px/16px）。
- 标签（分区标题、表头、消息角色、步骤组）统一为 `label`：mono、uppercase、字距 0.12em、muted；11px，消息角色与表头可用 10px。面板标题为 sans 12–13px / semibold。计数、时间、额度、diff 统计使用 `tabular-nums`。
- 9–10px 仅用于已有微型状态（chip、未读数）、额度、rail 标签、消息角色头与表头，不扩散到正文或表单。
- 大号字（display，600，字距 −0.045em，CJK −0.03em）只用于空状态和云端页面标题，不进会话主屏。
- 手机（< 768px）transcript 正文 14px / 22px，会话列表标题 14px / 20px；其余字号与桌面相同。
- 手机 input / textarea / select 的 computed font-size 为 16px，以防 iOS 输入自动缩放。不能删除这个覆盖来让尺寸「看起来相同」。选择框是 `components/ui/select.tsx`（Base UI，不用原生 `<select>`），表单款触发器在手机上同样 16px，与旁边的输入框一致。
- 键盘焦点一律是 2px 墨色外框、外扩 1px（`:focus-visible`，光暗主题相同，见 `index.css` 的 base 层）。控件可以另外把边框变 `fg` 作为辅助，但不能用 `focus:outline-none` 只换边框；分段选项的焦点环必须画在墨底之外，不能叠在选中项的墨色填充上。
- 用词（`docs/local-aggregator.md` §3.1、§6）：界面上一律叫「设备」/「裝置」/「デバイス」/ device，可运行 AI 的设备也是；要分开时说「可运行 AI 的设备」或靠「可运行 AI」徽章。不说「机器」/ machine，也不说「电脑」/ computer（ja 不说「マシン」「コンピューター」「端末」）。每台可运行 AI 的设备一把的 `MK` 叫「内容密钥」/ content key（「设备钥」已指每台设备自己的身份钥）。说页面所在的这台设备也是：「这台设备」/「這台裝置」/「このデバイス」/ "this device"，不说「本机」「本機」「このコンピュータ」/ "this computer"（「本地」照用）。落地页的卖点文案是营销文案，另说；下载页是产品界面，照此说（指平台时直接说 Mac、Linux 服务器、Windows）。`web/test/ui-wording.test.ts` 检查四张表、下载页的表和界面源码里上屏的每段文字，没有例外清单。

## 2. 控件尺寸

| 用途 | 默认尺寸 | 参照/实现 |
| --- | --- | --- |
| 常规单行表单 | 高 32px / `h-8`（手机 40px / `h-10`），水平 padding 12px，sans 13px；模型 ID、路径、时间等值可用 mono | `Input`、`Combobox`、`Select` 表单款、设置表单 |
| 分段选项 | 外框高 32px，`border-strong` 1px，无内 padding；项间 1px `border` 分隔，选中项墨底反白，文字 sans 13px | `TabsList`；手机分类条 36px |
| 选择框触发器 | 高 32px（手机 40px），水平 padding 12px，值 sans 13px，chevron 14px；`border` → hover `border-strong` → open `fg`；150ms 旋转 180°；disabled 用固定灰阶（`bg-raised` + `text-faint`），不用整体 opacity | `SelectTrigger` 表单款 |
| 选择框浮层 | 宽 `max(触发器, 10rem)`、上限 20rem；距触发器 4px；1px `border-strong`，无投影；上下 padding 6px；最大高 `min(20rem, 可用高度)`；行桌面 32px、触屏 44px，左右 12px；hover 与键盘高亮同一 `bg-hover`；选中行 `bg-raised` + 右侧 14px ✓ + 500 字重；分组标题 10–11px mono uppercase muted；分隔线 1px；空列表一句 13px muted；开合 120–150ms ease-out（opacity + 2px 位移，`prefers-reduced-motion` 只留 opacity）；滚动箭头占位、不覆盖行 | `SelectContent`、`DevicePicker` |
| 设置与弹窗操作 | 高 28px / `h-7`，sans 12px / 500 | 保存、取消、测试、设置页的增删改 |
| 32px 页头内的文字按钮 | 高 24px，sans 11px | 停止 |
| 面板页头图标操作 | 20×20px / `size-5`，图标 14px | 折叠、展开、新建、共享弹窗关闭 |
| 文件浏览工具栏 | 32×32px / `size-8`，图标 16px | 前后、刷新、隐藏文件；排序框高 32px |
| rail 导航 | 桌面 32×32px，手机 36×36px；当前项墨底反白 | 保留现有 desktop rail / mobile nav 布局 |
| transcript composer 工具 | 有边框 select 高 22px；ghost 高 24px | `ComposerSelect`；新建会话用同一个 composer（`ComposerBox`），智能体、工作目录、设备、账号也是 24px ghost chip |
| 云端页面 | 主操作与输入 36px，顶栏 56px；「我的设备」的行操作 28px（触屏 36px） | 只用于 cloud 登录 / 「我的设备」 |

选择框的弹出列表与按钮下拉菜单（会话操作、语言、新建…）同一张浮层：`bg` + 1px `border-strong`、直角、无投影、上下 6px 内边距；行桌面 32px、触屏 44px，文字距框 12px（与表单字段的文字对齐），字体随触发器（表单字段 sans 13px，composer chip 为 mono 11px，路径建议为 mono 12px）；高亮行（指针与键盘同一种）`hover` 底；选中行 `bg-raised` 底、右侧 14px 墨色 ✓、字重 500，没有蓝色；分组标题 10–11px mono uppercase muted，分隔线 1px；空列表显示一句 13px muted。宽度不小于触发器与 10rem 中的较大者，不超过 20rem 和视口；列表贴着触发器：间隔 4px、与触发器左边对齐（右侧放不下时改为右边对齐），默认在下方，下方放不下时翻到上方，不盖住触发器，以该侧可用高度为限（最高 20rem）在列表内滚动、不出视口；滚动箭头占据列表内的一行，不覆盖首尾行。开合 120–150ms ease-out（opacity + 2px 位移），`prefers-reduced-motion` 下只保留 opacity。composer 里的 ghost chip 也一样：composer 在窗格底部时列表在 chip 正上方打开、盖在输入框上，不跳到整个 composer 卡片之外。

composer 的附加文件/拍照与 ghost 选项为 24px，主发送按钮为 26px 墨色实心，是主操作层级的合理差异。消息图片查看器使用 28px 工具按钮，区别于共享表单弹窗的 20px 关闭按钮。

同一上下文控件共用高度和对齐方式。文本链接、breadcrumb、checkbox、badge 和清单行属于不同角色，不因与按钮同排就拉成同尺寸。图标为 lucide，`strokeWidth` 1.5，通常 12/14/16px（手机 18px），选择用途已有档位。

圆角一律为 0：控件、弹窗、菜单、分段、chip、未读数、图片缩略图都是直角；只有状态点和 radio 指示是圆形。按钮四种：primary（`bg-fg text-bg`）、secondary（`border-strong` 描边）、ghost（无框，muted → fg）、danger（`danger` 文字 + 描边）；不再使用 accent 15% / 25% 着色按钮。输入背景 `bg`，边框 `border`，焦点时边框变 `fg`；键盘焦点另加 2px `fg` 的 `focus-visible` 外框。

开关与勾选框各只有一种实现：`components/ui/switch` 与 `components/ui/checkbox`，不允许把原生 checkbox 的默认外观留在页面上。开关是 24×14px 的方轨（选中墨底、未选 `border-strong`）加 8px 方点，整行 28px、触屏 36px；勾选框是 14px 方框（触屏 16px），选中墨底 + 10px 勾，键盘焦点在方框外画 2px 墨环。开关用于立刻生效的 on/off；勾选框用于表单里的选择与多选。radio 仍按各自的组语义使用，形状统一为墨色圆点 + 1px 方框。

## 3. 间距与容器

- 细小图标/控件 gap 为 4px，常规同行 6–8px，设置标签与控件的宽屏间隔 16px。
- 面板页头高 32px；侧栏页头水平 padding 10px，内容页头 12–16px。shell 中 rail 品牌格、列表页头与内容页头共用同一条 32px 页头线。页头内部动作居中，长标题允许截断，图标动作不能被挤缩。
- 会话列表行：上下 8px、左 12px 右 10px，两行（标题 13px/18px，meta 11px mono），行间 1px 细线；状态边条 2px 贴左缘。
- transcript：消息行 padding 8px 16px 10px，行间 1px 细线，用户消息为 `raised` 底带；工具行上下 3px，列为「序号（仅折叠组内）· ⚙ · 描述 · 状态」，工具名留在图标 tooltip；diff/输出块在所属工具行下方，1px 细线框；activity 条高 28px；composer 外边距 12px（窗格内 8px）。
- transcript 只追加：信息流里只有已经写入的条目，agent 工作时已显示的内容不改变高度，最后一条下方不放临时内容。尚未写入的实时状态只在 composer 上方的 activity 条里说：思考时「N 条消息 · 思考中… ~50 tokens · 7m14s」（替换「最近活动」那一段）；token 计数在 5 个字符宽的槽里右对齐（`tabular-nums`；槽宽由同格里看不见的 `~0.0k` 撑住，`~50` 与 `~1.2k` 同宽，后面的「tokens」与时长不动），条宽不足 26rem 时省掉「tokens」字样，不用 tooltip（触屏点不到）；思考结束后「最近活动」从计数最后一次变化算起（只算本轮）；activity 条始终一行，放不下时截断而不换行。
- 新建会话是一个空会话视图（`/sessions/new`、分屏的 `new-session` 窗格），页头与 composer 与会话相同，不用弹窗；页头没有路径条，目录只出现在 composer 的目录 chip 和下面的列表里。版式同新对话：一栏居中，最宽 48rem（与 transcript 段落的 48em 同宽），栏内左对齐。桌面与窗格里自上而下是问题标题（h2 20/26、600）、composer、最近目录；标题上方留白为视图高度的 50% − 12rem（至少 16px），只随视图高度变，所以打字、贴图时 composer 顶边不动；目录区在 composer 下方单独滚动，矮窗格里也是。手机（< 768px）composer 钉在底部（上细线 + safe-area），标题和目录在它上方、贴着它，可滚动。目录行单行，桌面 32px、手机 40px，是共用边线的相接方框（1px `border`）：radio 圆点、目录名 13px（手机 14px）、上级路径 11px mono `faint`（中间截断：上级目录名保留，前面的部分先省略）、最近活动 11px mono 靠右；会话数只放在行的 title 里。选中按 radio 的方式标：墨色圆点 + 墨色 1px 方框（同问题卡片的选项），不用 `hover` 底块，不画状态边条。先列 6 个，下方一行是「显示全部（N）」/「收起」和「选择其他目录…」（路径输入 + 文件夹浏览）；选中的目录不在前 6 个里时补在最后。↑/↓ 在列表里移动选中（radio group），在 composer 里消息为空时直接 ↑/↓、否则 Alt/⌥+↑/↓；Enter 仍是发送。点目录 chip 跳到列表里选中的那一行。发送后待送达的消息显示在目录的位置，不挤动 composer。
- 设置分区和普通共享弹窗 padding 12px；分区标题下间距 8px；设置项之间 12px。面板过滤区 padding 8px。文档型页面（设置内容、云端）使用带左右细线的内容框：设置 48rem（`max-w-3xl`）；云端在 ≥1280px 时内容栏上限约 1200px、两侧留 32px，不再铺满 2400px。
- 设置页骨架（`features/settings/layout.ts`）：所有 section 共用一个 48rem / `max-w-3xl` 内容宽，页面标题与分区标题同在 12px 左缘；≥36rem 时设置行是两列栅格（label 16rem / control 14rem / gap 16px），同一分区里所有控件同宽同缘，窄于 36rem 时说明在上、控件在下并占满宽度。行高 44px，带说明 56px；label sans 13/18 / 500，说明 sans 12/18 muted，值 sans 13/18。mono 只用于 ID、路径、时间、模型 ID 等值。控件词汇：≤3 项用分段，≥4 项用选择框，布尔用方形 Switch/Checkbox，多选用 Checkbox。设置页操作按钮 28px、sans 12/500，不再手写 mono 11。
- 「智能体与账号」（设置里的 `accounts`）的卡片网格可以用整张 sheet 的宽度：`auto-fill minmax(17.5rem, 1fr)`，桌面 sheet 上三列、手机一列，同「我的设备」；说明文字与表单行仍在 48rem 的内容列内。页内的 Claude Code / Codex / ACP 切换是文字 tab（13px，选中项墨色加 2px 下划线，在 1px 细线上），不是分段控件，手机上不与设置分类条相混。
- 「我的设备」（`/devices`）是一页、没有筛选：账户层面的提示（`AccountNotices`）在最上；页头是 28–32px（手机 28px）标题、一句摘要「N 台 · N 台离线 · N 件需要你」与唯一的主操作「添加设备」（手机上占满一行），点开先是两格选择（浏览器或手机 · 可运行 AI 的设备），再进各自的流程。区块之间用 24–32px 留白；分区标题是 sans 16px/24、600，可配 10px mono 编号，标题下只画一条 1px 线，其余分组不再画框。用量（`UsageSummary`）：先一句套餐与用量的总结，再 5 个数字——可用宽度 ≥1120px 五格一行、≥576px 三格 + 两格、更窄两格 + 两格 + 占满一行，格间只有 1px、没有外框也没有空格；数字 sans 20px/600，上限与说明 mono 12px，4px 用量条（墨色，≥90% 琥珀，满了红）。再下是「这台设备」：16px 标题行（标题 + 本设备名 mono 12px）、设置行（左文字 ≤48rem，右 `SquareSwitch`）、折叠的「中继」行（≥40px）。设备列表：容器 <44rem 时是行式，每行固定 56px、单行，为「图标 1rem · 名称（最多 2 个 badge）· 状态点 + 文字 · 28px 主操作 · 28×28px（触屏 36px）的 `⋯`」，更窄时 badge 收起、状态移到名称下、主操作缩到图标，行高不变；容器 ≥44rem 时是卡片网格（`auto-fill minmax(20rem, 1fr)`，间距 12px），每张卡 1px `border`、16px 内距、最小高 88px，卡内两行：名称与状态在左，主操作与 `⋯` 在右；只有一两台时卡片按 20rem 轨道左对齐，不拉伸成整行；每张卡按自身内容高度顶部对齐，一张卡展开详情或带有面板时不拉伸同排的其它卡。离线、待批准、已移除都不改变卡/行高度。hover 只在卡片上把边框变 `border-strong`，并给名称加下划线、主操作描边加重——不再铺整行 `hover` 底色；键盘焦点仍是 2px 墨色框，当前设备仍是 1px 墨色左边线 + `aria-current`。详情（设备 ID、指纹、路径、最近在线、离线副本、续期、会话计数与告警）收进行内 `<details>` 展开区，危险与次要操作在 `⋯`，菜单与会话操作同一张浮层，暂时不能按的项置灰并在下面一行写原因。待批准与已移除用独立的紧凑行（无 badge、无展开区），不套正常行加 badge；已移除折叠标题是 13px muted、带数量，与分区标题区分。列表之后是有内容时的「需要你」台账、钥匙与恢复词；最后是折叠的「已移除的设备（n）」。空列表一句话加一个动作。
- 「我的账户」（`/account`，顶栏「我的账户」、「我的设备」页头都能到）与「我的设备」同一种页、同一个页框：页头是 32px（手机 26px）标题、邮箱（mono）与唯一的主操作「添加通行密钥」（手机上占满一行；有表单开着时不显示，一页只有一个主操作）；下面是 40px 区域标题行（label）分开的「登录方式」「恢复词」「这台设备」。设置行同「这台设备」：左文字 ≤48rem，右 28px 操作（触屏 36px），放不下时操作换到文字下面。通行密钥每把一行：14px 钥匙图标、名称 sans 13px/500、其下 mono 11px 的「提供方 · 是否同步 · 添加时间 · 上次使用」（名称就是提供方时不再重复提供方），右侧是带文字的 `SquareSwitch`「新设备可用它加入」（手机上文字也显示）与 28px 的「重命名」「移除」；改名、改密码、移除的确认在行内展开（确认用 `Note`），不开弹窗。新设备申请加入时，每台完整设备的顶栏下与「我的设备」「我的账户」页框里都有一条告警（同恢复的告警条；文字用 `role="status"`，不用 `alert` 包住按钮；设备 id 用 mono、不断行）：「批准」在条内展开普通的配对（同「添加浏览器或手机」，默认「输入数字码」一格），「不是我：移除」先展开 `Note` 警告（这把通行密钥还能登录，先保护它同步所在的账户），移除后留一行「删除那个通行密钥」，直达「我的账户」里那把的移除确认。等待批准的新设备显示普通的配对页（标题「等待批准」，打开时就在「改用数字码」一格）。登录方式有增减改时，其它完整设备在同一位置有一条 warn 告警（同「新设备用密码登录」那条）：哪种变化、哪台设备、何时，「登录方式」「是我操作的」。
- 共享弹窗宽 `min(26rem, 100vw - 2rem)`；可为确实需要更多空间的内容覆写宽度。最大高 `100dvh - 2rem`，内容超出时在弹窗内滚动；标题与关闭按钮至少间隔 8px。弹窗页头 36px。
- 先测量完整容器再量内部元素：输入框处于带边框的搜索 label 中时，整个 label 才是对齐参照。

## 4. 响应式与窗格

- 768px 是 shell 桌面/手机断点；业务内容的适配按可用窗格宽度，不能只看整个浏览器宽度。
- 手机导航按钮组保留 `min-w-0`。宽度不足 480px 时六个入口固定为 36px、不能收缩，缩小 logo/外侧间距，在线状态和主题使用图标并保留屏幕阅读器文字；不依赖把按钮挤成图标大小来避免溢出。
- 设置内容使用 `@container/settings`。可用宽度不足 36rem 时偏好说明在上、选项在下；达到 36rem 后左右排列。控件限于容器宽度，保持文字可读。
- 手机设置页头只显示分类选项，分类条高 36px，长标签不换行/不挤缩，超出时仅分类条水平滚动。Tab 聚焦到屏外分类时应自动进入可视范围。手机上的设置字段 40px、控件行 44px；安全区规则不变。
- 安全页的设备列表按所在 section 的宽度适配：不足 36rem 时设备名、时间上下排列，右侧退出按钮留在视口内；宽窗格保留表格。
- 账号行的操作和删除确认限于所在内容区宽度，放不下时换行，确认与取消按钮必须可见。
- 分屏窗格之间是 1px 细线。窗格状态框为 2px 内框（工作中 `ok`、在问你 `warn`、未读 `unread`），静止不呼吸；聚焦窗格画四个 18×4px 的墨色角标，与状态框各占各的像素。
- 分屏内容选择器在空间充足时居中；高度不足时从顶部开始并在窗格内滚动，所有选项必须可达。会话、聊天和终端的选择列表限于窗格宽高，列表内容独立滚动。
- 套准标记（9×9px 的「+」，`mark` 色）只画在大面积接缝：shell 页头线与 rail/列表分隔线的交点、分屏缝的 T 形交点、文档型内容框与分区线的交点。宽度 < 768px 或窗格 < 480px 时不画；列表、行、卡片、弹窗、菜单内从不画。
- dialog / form 中的模型与 effort 可换行，长模型名不能推宽弹窗；多行输入保留原有 rows 和 resize。
- 文档本身不滚动，各视图/窗格持有自己的滚动容器。保留 `min-w-0`、`min-h-0`、safe-area、`.pb-safe`、键盘 inset 和 transcript 的滚动锚点规则。

## 5. 功能特有的合理例外

- Files 的 breadcrumb 是文字链接，搜索框内部 input 是无边框内容；外框与排序和工具按钮同为 32px。
- 终端（xterm）在两种主题下都是深色，底色 `#0f1115`（与暗色主题的墨面同值），ANSI 色沿用信号色相。
- 账户额度微型图表、状态 chip、未读数量、session receipt、消息气泡和终端字体各有语义，不强行统一成输入框或操作按钮。
- 用户上传的图片和屏幕 emoji 是用户内容，不去色。应用图标、favicon 与启动图由 `web/scripts/gen-pwa-assets.mjs` 从套准标记矢量生成，不手工改 PNG。

## 6. 验证原则

测量默认字号和 xlarge、明暗主题、1920 / 2560（云端宽屏页面）、1440 / 700 / 390 / 320px 宽度；短视口与长内容检查弹窗可达性。同一前后对比使用相同 fixture 和滚动位置，截图需实际查看，并与对应的设计稿对照（设计稿不在本仓库）。fixture 必须覆盖第 7 节状态矩阵中的每个状态（工作中、排队、在问你、未读、后台、空闲、失败、离线）和三窗格分屏。尺寸扫描只发现候选偏差，规范和用途决定是否修改。隔离 mock 只能证明前端渲染，不能证明真实后台业务行为。

## 7. 视觉语言（可机检）

以下规则以 computed style 为准，由仓库内的 `scripts/ui-audit`（`pnpm ui-audit --check`，配合 `.claude/skills/ui-consistency`）逐项检查；`[data-signal]`、`[data-edge]`、`[data-activity]`、`[data-unread]`、`[data-pane-state]` 标出合法使用状态色的元素。每次修改 `web/src` 里的 UI，合并前都要跑一次 before/after。

| # | 规则 | 检查方法 | 允许的例外 |
| --- | --- | --- | --- |
| V1 | `border-radius` 全为 0 | 所有可见元素 | 状态点与 radio 等圆形指示（`[data-shape]`）、原生 radio / checkbox、xterm 内部 |
| V2 | 没有投影：任何 `box-shadow` 的 blur 与 spread 外扩为 0 | 解析 computed `box-shadow` | 无；内框 `inset 0 0 0 Npx` 可用作描边 |
| V3 | 线宽：边框 1px；2px 只用于状态边条、窗格状态框、tab 下划线、键盘焦点；4px 只用于聚焦角标 | `border-*-width`、inset 宽度 | — |
| V4 | 非灰色（RGB 极差 ≥ 3）的 `color` / `background-color` / `border-color` / `box-shadow` 颜色必须位于带信号标记的元素内 | 祖先或自身含 `data-signal`、`data-edge`、`data-activity`、`data-unread`、`data-pane-state`、`data-diff` 或 `data-allow-color` | diff 行、插件里有业务含义的颜色（`data-allow-color`）、终端、用户图片 |
| V5 | 中性色来自 token：`bg`…`fg` 都是纯灰，旧的灰蓝色板会被 V4 当作非灰色报出 | 同 V4 | 同 V4 |
| V6 | `label` 元素：mono、uppercase、字距 0.12em ±0.01、字号 10 或 11px、颜色 `muted`/`fg` | 带 `data-label` 或 `.label` 的元素 | CJK 文本 uppercase 无效属正常 |
| V7 | 尺寸表（§2）中的控件高度；设置行 44px（带说明 56px），≥36rem 时控件列 14rem、同分区左缘一致 | 现有 rows / kinds 分析 | 窄于 36rem 的单列布局、§5 例外 |
| V8 | 套准标记 `[data-regmark]` 为 9×9px，只出现在 shell / 分屏 / 文档内容框容器内；视口 < 768px 时数量为 0 | DOM 计数与位置 | — |
| V9 | 状态矩阵：工作中行有 `data-edge=working` 与 `LiveDot`；在问你 chip 为 `bg-warn` + 墨字；未读数为 `bg-unread`（= fg）方块；后台无边条；空闲什么都不画 | fixture 中每种状态各一行 | — |
| V10 | 主操作按钮为 `bg-fg text-bg`；页面内同时可见的 primary 不超过 1 个（弹窗内 1 个） | 扫描 `[data-variant=primary]` | 分屏的每个窗格各算一页 |
| V11 | 分区标题为 sans 16px/24、600（可配 10px mono 编号），不与 badge / meta 同字号；标题下只有一条 1px 线，分组之间用留白 | `[data-section-head] h2` 的 computed 字号 / 行高 / 字重；页内 `border-b` 计线 | 折叠的「更多设置」「已移除的设备」是 13px muted 的补充标题，不算分区标题 |
| V12 | 「我的设备」每行首行固定 56px（`h-14`），状态最多一行、详情在 `<details>`；离线 / 待批准 / 已移除不改变行高，待批准与已移除用独立紧凑行 | `[data-testid^=device-]` 首行高度与 `[data-status]` 结构 | — |
