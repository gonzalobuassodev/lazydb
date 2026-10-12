# 快捷键前缀提示框四方向导航 Implementation Plan

**执行者：** Luna。仅分析/计划使用 GPT-6.1 Sol；此计划不要求子 Agent，不在当前阶段实施或提交。

**Goal:** 输入快捷键前缀后，提示框内高亮候选支持符合视觉位置的上下左右移动，Enter 准确激活高亮候选。

**Architecture:** 保留列优先显示，抽取小型共享网格计算供 Keymap 和 UI 使用，Runtime 提供真实视口宽度。沿用现有 pending、Action 与刷新机制，不引入生产依赖，不改变主面板焦点导航。

**Tech Stack:** Rust 2024 / Rust 1.94、crossterm、ratatui、现有 TestBackend。

---

## 1. 基线与执行约束

- 原工作区：`/Users/yelog/workspace/tui/lazydb`。
- 目标分支：`main`；起点：`6d022f8be961f9b81d74022220cd942e3b39ec81`。
- analyze 时 HEAD/main 均等于起点，已暂存、未暂存和未跟踪差异为空。没有需复制到新 worktree 的本地行为依赖。
- 实施前重新核对实际状态；如出现用户修改，不 reset/stash，不替用户提交。原工作区允许脏，新 worktree 不会自动继承未提交文件；只有确实依赖的本地行为才纳入明确修改范围。
- 分支名称仍待自动流程/Luna 命名，本阶段不创建分支或 worktree。
- 报告、计划和验证摘要仅写原任务目录 `.git/opencode-tasks/ses_ed07ee83fffeKT6HZuwPB8KsQs`。不修改 `state.json` 或插件维护的 `checkpoint.json`。
- plan 阶段读取 checkpoint 时文件仍不存在；不创建它，不使用历史 next 扩大当前阶段。
- 本次正式 plan 协议指定回执 `plan-0f20c7b3-b8b3-4bb9-8152-7d0658269064.json`，token 为 `0f20c7b3-b8b3-4bb9-8152-7d0658269064`。先写 `change-scope.json`，完成阶段后最后写该回执；不覆盖 analyze 或其他历史回执。
- 普通实现取舍自行推进；按下面闭环连续执行，实施阶段不要因完成一个单元要求用户 resume。

## 2. 锁定的产品行为

**要求分级：** 用户明确要求的是提示框内四方向空间导航；以下边界循环、末列 clamp 等是为实现该需求选定的设计决策，非用户逐条指定。显示/执行一致性与高亮可见性属于本功能的正确性验收。项目强制门禁是单元 C 中已有 CI 的 fmt/clippy/test；新增定向自动测试用于证明本次实现，不代表新增全项目门禁。人工/PTY、截图以及无默认特性构建仅为补充验证，失败或不可用不自动升级为强制阻塞。

1. “焦点移动”仅指提示框高亮，不是主界面 pane 焦点。无 pending 时的方向键行为保持原样。
2. 普通列表保留宽度阈值：小于 60 一列，60–99 两列，100 及以上三列；保留列优先索引 `index = column * rows + row`。
3. Up/Down 在当前非空列内循环。Left/Right 到相邻非空列，保持行号；目标列短则取其末项。左右外边界停留，不跨边界循环。
4. 单列 Left/Right 无水平邻项时消费按键但不移动选择，避免移动底层 pane 焦点。EditorLeader 不属于该 floating chooser，继续原有按键行为。空列表不计算零除数，单项稳定，越界 selected 先规范化。
5. 新导航仅使用无修饰方向键。Repeat 支持移动，Release 继续忽略。不覆盖已配置的完整后缀/有效续接匹配和已有全局快捷键优先级。
6. 方向键不执行命令，Enter 激活当前高亮项，Esc/Ctrl-C 按原规则取消。非 leader 超时、模式/focus/tab 变化失效规则保持。
7. 当前 resize 会清除 pending，保留该行为；不加入 resize 保持菜单的新需求。
8. 不改变既有 popup 最小尺寸和高度处理。
9. 标题用 `Arrows select` 或同等简短说明，并保留 Enter 与原取消提示。
10. pane Window/PaneSequence popup 与普通 help chooser 分开处理；保留既有 PaneSequence parser 与真实绑定输入，不新增 Enter 激活模型。

## 3. 单元 A：普通前缀的四方向选择闭环

### 文件

- 新增：`src/input/sequence_navigation.rs`（crate 内纯计算与测试）。
- 修改：`src/input/mod.rs`（模块注册）。
- 修改：`src/input/keymap.rs`（视口宽度及普通 pending 选择）。
- 修改：`src/ui/mod.rs`（共享布局及标题）。
- 修改：`src/runtime.rs`（真实视口宽度同步）。
- 测试：`tests/keymap.rs`、`tests/ui_render.rs`，也可在 keymap 原有单元测试区补内部状态测试。

### A1. 先写共享计算的失败测试

实现目标是小型值对象/函数，不是通用几何导航框架。需要的能力：由 width/count 得到逻辑 columns/rows；从 selected 得到 row/column；由 direction 得到有效 selected。候选少时仅导航非空列，不改变原 rows 对既有多项列表的解释。

锁定例子（count=8、width=120、rows=3）：

| 初始索引 | 按键 | 目标索引 |
| --- | --- | --- |
| 0 | Right | 3 |
| 3 | Left | 0 |
| 0 | Up | 2 |
| 2 | Down | 0 |
| 2 | Right | 5 |
| 5 | Right | 7 |
| 7 | Right | 7 |
| 7 | Up | 6 |
| 6 | Down | 7 |

覆盖 width=59/60/99/100、count=0/1/2、整除/非整除列数、空末列、selected 超范围及每个边界。

运行：`cargo +1.94.0 test --lib sequence_navigation`。预期新增断言先失败，完成纯计算后通过；记录实际结果，不把未创建模块时的编译失败当成行为失败证据。

### A2. 写真实 Keymap 空间选择测试

- 用真实 leader 候选计算 count，设置视口为 120；首项 Right 的 selected 必须为 rows，不是 1。
- Up/Down 不离开当前列；Left 返回原列。
- 每次方向键 map 返回 None、pending 仍在、App.focus 不变。
- Enter 的 `ExecuteHelpShortcut` id 必须等于 UI 同序候选中当前 selected 的 id。
- 单列左右 no-op，Repeat 移动、Release 不移动，空候选不崩溃。

带视口宽度 setter 的测试放在 `src/input/keymap.rs` 内部单元测试中：crate 内接口不能从 `tests/keymap.rs` 的外部 crate 直接访问。渲染的组合测试可放 `src/ui/mod.rs` 内部测试中调用该 setter；`tests/ui_render.rs` 则使用构造的公开 sequence 快照验证坐标/背景。不要仅为测试把 setter 扩大为公共 API。

### A3. 最小实现与接线

1. 完成共享计算，避免 UI 与 Keymap 各写一套宽度阈值。
2. `Keymap` 增加 crate 内视口宽度 setter，默认无视口场景解释为单列；保留 `map(event, app)` 的公开签名与现有调用方式。
3. 在普通 pending 的 Up/Down 分支接入二维计算，加入 Left/Right 分支；只消费符合导航条件的事件。
4. UI 普通候选布局接入同一计算；保留候选顺序、样式和描述截断逻辑。
5. Runtime 首次绘制以及后续每次绘制同步实际绘制 area.width 到 keymap，使下一次输入使用已显示的布局。terminal 尺寸不能取任意默认常量，不能仅新建前缀时同步一次。
6. 标题改为四方向说明；不扩大为 resize 后保持提示框或新的高度滚动行为。

### A4. TestBackend 验收

- 用相同 sequence 与视口检查实际选择背景的行列，而非仅查 selected 数字。
- 测宽度 59/60/99/100/120；窄宽标题可能裁剪，应针对可见片段断言，不要求完整长标题总可见。
- 在 59/60/99/100/120 宽度以 TestBackend 验证高亮位于预期视觉列。既有 TooSmall/弹窗高度行为不变。
- 更新 `pending_prefix_opens_floating_shortcut_window` 等原有 `Up/Down select` 字符串断言。
- 新增测试统一包含 `sequence_navigation` 名称片段。

定向运行：

```sh
cargo +1.94.0 test --lib input::keymap::tests
cargo +1.94.0 test --test keymap
cargo +1.94.0 test --test ui_render sequence_navigation
cargo +1.94.0 test --test ui_render pending_prefix
```

**闭环验收：** 普通菜单显示与四方向选择一致，Enter 目标正确，高亮可见，无主面板动作副作用。通过后更新 validation.md 并继续 B。

## 4. 单元 B：兼容性与提示框可见性复核

pane Window/prefix popup 与普通 floating shortcut chooser 是不同路径：普通 chooser 输入端依据 HelpShortcut 列表移动并由 Enter 执行所选 id；Window popup 展示 PaneCommand 配置后缀，现有按键端继续处理真实配置按键。不要臆造 Window 的 Enter 激活或重写其 parser，也不要仅为对齐截图扩大本功能到 pane 序列。

复核已有 `configured_pane_prefix_renders_its_candidate` 与 `counted_pending_prefix_keeps_count_in_footer_label` 测试，确保显示内容与标签保持。需要更新标题时，仅在行为一致且确实为本功能提示时更新并断言；保留配置后缀直接执行和 count 行为。

针对普通 chooser 验证：

- Explorer/Results 普通 leader 下 Right/Left 在多列之间移动；Up/Down 在当前列内循环。
- 单列左右不改变选择且不泄漏到 pane 焦点动作；pane 与 EditorLeader 非 floating chooser 不被意外接管。
- 无 pending 时方向键、超时、上下文失效、Repeat/Release、直接键入后缀、Enter 执行高亮 id 兼容。
- 标题只调整本普通 floating chooser，不改其它帮助/弹窗内独立的 Up/Down 指引。

定向命令：

```sh
cargo +1.94.0 test --lib input::keymap::tests
cargo +1.94.0 test --lib sequence_navigation
cargo +1.94.0 test --test keymap
cargo +1.94.0 test --test ui_render pending_prefix
cargo +1.94.0 test --test ui_render configured_pane_prefix
cargo +1.94.0 test --test ui_render counted_pending_prefix
```

通过后继续 C；不用为了一个独立旧路径新增未经需求要求的交互。

## 5. 单元 C：完整验证与 Luna 收尾审查

### 强制项目验证

功能齐备后按 `.github/workflows/ci.yml` 81–83 执行一次：

```sh
cargo +1.94.0 fmt --all -- --check
cargo +1.94.0 clippy --all-targets --all-features -- -D warnings
cargo +1.94.0 test --all-targets --all-features
```

不额外要求每个单元重复 check+clippy+全量测试。发生相关代码/环境变化时，只重跑有失效风险的定向检查，并在功能齐备的版本上完成最终全量验证。

全特性含 Oracle。本机环境受限时记录真实原因与失败，不能以无默认特性成功冒充全特性通过。编译和测试失败属于实现需自行处理的问题，不直接 blocked。

### 补充检查

人工/PTY 的 1/2/3 列视觉操作是补充证据，不是用户要求的前置条件。环境受限最多一次针对性修复重试，再由 Luna 决定记录限制或补充证据，不无限保持 progress 重试。

### 审查清单

- UI 与输入共用列数/rows 规则，Runtime 使用真实显示宽度。
- 末列、空列表、越界索引及内高为零均安全。
- 上下不跨列，左右不线性串行、不选空格。
- 高亮项可见，Enter 目标与高亮候选同源。
- pane 配置后缀解析和 count 不变；EditorLeader/pane prefix 不因 chooser 导航被劫持。
- 无 pending 的导航、超时、上下文失效、Repeat/Release 不回归。
- diff 仅包含本功能；没有覆盖用户修改或修改 state/checkpoint。
- validation.md 包含实际命令、退出结果、相关文件、代码版本/工作区状态与环境；本计划的预期结果不能当成通过证据。

提交、合并和任务命名交由后续 Luna/自动流程按当时授权执行，本阶段不执行这些动作。

## 6. 实施范围与完成状态

本计划由 GPT-6.1 Sol 按要求从 analysis.md 生成/更新，实施由 Luna 完成。实现与验证状态见任务目录 validation.md。本文件随任务 worktree 携带，已更正范围以反映真实实现（普通 floating chooser，不改 pane parser）。

计划阶段的 `change-scope.json` 预测文件清单包含 `tests/keymap.rs`；实施中相同 Keymap 行为测试放在 `src/input/keymap.rs` 的单元测试区，所以 `tests/keymap.rs` 实际未修改。实现源码/测试只改 `src/input/sequence_navigation.rs`、`src/input/mod.rs`、`src/input/keymap.rs`、`src/runtime.rs`、`src/ui/mod.rs`、`tests/ui_render.rs`。此外 docs/plans 下的本计划随任务 worktree 携带并更新。未改 pane parser、配置、CI 文件；无未提交行为依赖。

工作流已确定任务分支 `task/four-way-shortcut-navigation` 和 worktree。A 单元纯布局、Keymap、实际 UI 高亮测试已完成。C 单元全项目门禁已在最终交付前完整通过；PaneSequence 分支最后按 HEAD 精确还原后，再次 fmt、Clippy、网格/UI 测试通过，pane/editor 定向回归亦通过。无需用户代替实施或再次询问执行方式。
