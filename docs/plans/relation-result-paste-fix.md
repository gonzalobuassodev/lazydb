# Relation Data / Result Set Clipboard Paste Implementation Plan

> **执行者：Luna。** 按本自动工作流逐项实施、验证、审查并完成后续阶段；无需询问执行方式，不启动子 Agent。本文件使用 writing-plans 技能组织，用户指定的目录、模型分工及阶段协议优先于技能默认的执行技能、worktree、提交和交接要求。

**Goal:** 让终端 Cmd+V 发送的粘贴文本进入 Relation Data / SQL Result Set 的 WHERE、ORDER BY，以及已有 Relation 单元格编辑框，且选区、类型验证和一次撤销行为正确。

**Architecture:** 保留 `Event::Paste → map_paste → Action → App → buffer` 的现有流程。新增 `DataQueryPaste(String)` 和 `RelationEditPaste(String)`，按焦点优先路由；普通输入复用 TextInput::paste，JSON 提供原子字符串插入，选择型和分段型单元格整体替换草稿，统一经过 CellEditorBuffer 的 presence/content 历史。

**Tech Stack:** Rust 2024 / 1.94、crossterm 0.29、ratatui 0.30.2、chrono、现有 TextInput / JsonBuffer / TemporalDraft，无新依赖。

---

## 1. 起点、范围和执行约束

原工作空间 `/Users/yelog/workspace/tui/lazydb`；目标 main；起点 `df42eac22eb662d08066f1e25691b1787181a529`。plan 阶段再次只读检查，HEAD 与 main 均等于起点，status / unstaged diff / staged diff 均为空。当前没有未提交依赖文件，因此从起点建立的任务 worktree 包含全部所需行为。checkpoint.json 仍不存在，不使用旧 next，也不修改 checkpoint.json 或 state.json。

本阶段仅创建计划和范围清单、追加验证记录、写本轮回执；不实施业务代码、不改变 index、不 stash、不提交、不建分支。后续任务分支由插件/Luna 自动命名；Luna 在任务 worktree 实施，保留原工作区可能新增的用户修改，不复制整个原工作区。

预计修改文件（精确清单见 change-scope.json）：

| 文件 | 预计改动 |
| --- | --- |
| src/action.rs | 两个携带 String 的 Action |
| src/input/keymap.rs | 查询栏和 EditCell 的 Paste 路由、Busy 保护 |
| src/app.rs | Relation Action 允许列表、两个更新分支、查询栏行为单测 |
| src/model/cell_editor.rs | JsonBuffer 和 CellEditorBuffer 批量粘贴、类型/历史单测 |
| tests/keymap.rs | 四种查询栏、EditCell、路由边界回归 |
| tests/relation_tabs.rs | 事件到单元格草稿的集成回归、防误插行 |
| docs/keybindings.md | 终端 Paste 与 typed 单元格粘贴规则 |

无预计新增/删除/重命名的仓库文件。报告不复制到 docs/plans，不随业务提交；task 元数据不属于仓库改动清单。仅读取参考的 src/runtime.rs、src/terminal.rs、src/model/text_input.rs、src/model/data_query.rs、src/model/relation_edit.rs、Cargo.toml、Cargo.lock 和 .github/workflows/ci.yml 不列入修改范围。若实际 diff 需要扩大文件范围，Luna 先核实必要性、更新范围与理由，再实施；不无故修改这些参考文件。

## 2. 已确认问题及约束

分析来源：同目录 analysis.md。关键定位均基于起点行号，实施时按符号定位：

1. src/terminal.rs 的 TerminalSession::enter 已启用 EnableBracketedPaste，src/runtime.rs:6689 的 Event::Paste 已转发字符串到 map_paste。
2. src/input/keymap.rs:3800 的 map_paste 没有 DataQuery 输入分支；查询焦点被 is_relation_data_focus 的 query.focus.is_none() 排除，并因 Focus::Results 返回空 actions。
3. EditCell 满足 is_relation_data_focus，因而 map_paste:3900 错误返回 RelationPaste 并丢弃字符串。App::relation_paste:25788 读取内部 yank 行，可能无效果或误插行。
4. src/app.rs:4119 的无 active console 筛选对 Relation 使用 Action 允许列表，新 Action 必须加入 DataQueryInsert / RelationEditInsert 相邻列表（约 4220、4257）。
5. TextInput::paste 已支持原子粘贴；JsonBuffer::insert 每字符记历史；TemporalDraft::insert 是分段覆盖；CellEditorBuffer::apply_text_edit 负责 presence/history。逐字符回放不是可接受方案。
6. SQL Result Set 没有 RelationGridMode::EditCell 或单元格写回入口，本计划只修复它的查询栏，不扩展 SQL Result Set 写回功能。

Cmd+V 默认是终端自身读取剪贴板并发送 Event::Paste 文本。裸 SUPER+v 的终端配置与 Paste 事件不同，当前没有真实事件采样；本次不新增主动读 arboard 的平台/远端剪贴板路径。交付应明确修复的是终端原生 Paste 路径，不能称裸 SUPER+v 或操作系统剪贴板已自动测试通过。

## 3. 必需验收与补充证据

### 3.1 用户需求及现有项目行为：必须满足

- Relation/Sql × Where/OrderBy 四个场景正确接收完整文本，不提交查询。
- 普通文本/JSON 在光标或选区粘贴，Unicode 和多行内容保留；Boolean/Date/Time/DateTime/Timestamp 采用整值替换。
- 一次粘贴一次撤销、一次重做；Null / Unprovided 与 Value 状态共同恢复，编辑后正确丢弃 redo 分支。依据 docs/keybindings.md:505–509 已有原子粘贴约定。
- 粘贴单元格只更新 EditCell 缓冲区；有 yank 时也不插行、不改变已有行值/行历史、不发数据库写命令。
- 不误投 overlay、其它 pane、DDL、不可用 query capability 或 Relation Busy；已有其他 Paste 和网格 p 保持行为。
- 非法 typed 内容保持可撤销草稿，由既有 confirm validator 报错，不默默过滤成另一值或自动确认。

### 3.2 项目既有 Rust 门禁：最终执行一次

.github/workflows/ci.yml:81–83 定义 fmt、clippy、全量 tests。实现齐备后执行对应命令；阶段内先定向测试，不每单元重复全量检查。不增加独立 cargo check、驱动改造或发行检查。本计划不修改 CI，原 CI 的其它 jobs 仍按其配置执行；本地不额外启动数据库容器或发行检查模拟全部 CI jobs。

### 3.3 新建议的补充验证：不自动成为必需门禁

真实 macOS 终端 Cmd+V、PTY bracketed paste 注入、SSH/tmux 传输，以及多行 SQL 在查询栏显示观察属于补充证据。docs/keybindings.md:883–886 提醒真实远端链路需要人工 QA，但用户未指定具体终端/SSH/tmux 环境，本次也不改这些链路，不能将新增人工清单变成自动工作流的停止条件。

环境不可用时准确记录缺少哪种交付证据；最多一次有针对性的环境修复重试，随后由 Luna 收尾审查决定是否补充。不得无限保持 progress；若补充检查发现真实缺陷，则缺陷本身按本任务范围修复、定向回归。

## 4. 精确输入规则

| 输入种类 | 粘贴规则 | 历史/确认 |
| --- | --- | --- |
| WHERE / ORDER BY | TextInput::paste 原始字符串，不替换 CR/LF/tab，不 trim；选区替换和 Unicode 光标复用已有实现 | 一次原子历史，completion 刷新一次，submitted/derived 按 helper 规则维护；无 submit |
| 普通 cell Text | TextInput::paste 原始字符串，按光标/选区插入 | CellEditorBuffer 完整快照历史，无 confirm |
| JSON | JsonBuffer 一次 record、remove_selection、insert_str，cursor 按 chars 数移动，sql_null=false | 保留 JSON 多行；一次完整 presence/content 快照；JSON null 与 SQL NULL 区分 |
| Boolean | trim 外围空白后整体替换布尔草稿 | true/t/false/f 继续由现有校验解析，其它非空文本在 confirm 报错 |
| Temporal | trim 外围空白后由同 kind 的 TemporalDraft::from_kind_and_text 重建 | 同步 segment/fraction_width，保留时区/小数秒；现有 parse 验证 |

真正空字符串对全部类型 no-op，保持选区、错误、presence 和历史。Boolean/Temporal 的纯空白字符串也 no-op：避免 TemporalDraft::from_kind_and_text 的空字符串默认值将空白粘贴转成 1970 年或默认时间。普通 Text/JSON/SQL 中空白是数据，仍正常插入。这是对 analysis.md typed trim 规则的边界细化。

内容或 presence 变化只记录一次有效编辑；Value 状态整体替换同值不人为添加历史。JSON/普通文本粘贴替换同值但光标/选区发生变化时遵循底层输入行为，不为了去重破坏既有语义。

map_paste 优先级：既有 Omni/overlay → 既有 Redis → 当前 DataQuery 输入 → 当前 Relation EditCell → Relation 网格默认 → SQL Editor。查询路由须同时检查 Results pane、Data 视图、query.focus 非空及 capability 是 Relation/Sql；不能单靠 active_data_query_has_focus，因为它不检查 view/pane/capability。当前 Focus::Editor 即使保留 query.focus 字段，也应走原编辑器 Paste。

Relation Data 非查询栏的 EditCell 粘贴优先于 RelationPaste，Busy 返回空。保持 Browse/VisualLine 原有终端 Paste → RelationPaste 和键 p 的行粘贴；不泛化重写网格粘贴协议。

## 5. 实施单元 A：四种查询栏形成完整闭环

**修改：** src/action.rs、src/input/keymap.rs、src/app.rs。
**测试：** tests/keymap.rs；src/app.rs 原有 data_query 测试模块（约 28384、28543）。

步骤依次执行：

1. 在 tests/keymap.rs 复用现有 query fixture，新增 `data_query_paste_routes_relation_and_sql_fields`：循环 Relation/Sql、Where/OrderBy，设置 Focus::Results、Data view、有效 capability，并测试一个包含 Unicode 与换行的 payload。先断言旧实现的 map_paste 非空且携带 payload，确认因空 actions 失败。采用现有 action 进行旧行为断言，或加入新增 Action 声明后再执行携带内容的断言；不能把未声明 enum 导致的编译错误当作已证明行为回归。
2. src/action.rs 在 DataQueryInsert 附近声明 `DataQueryPaste(String)`，保持 derive 和 TextInputEdit 的 Copy 设计。无需 RelationQueryPaste 兼容别名。
3. map_paste 增加第 4 节的 query predicate，返回 `vec![Action::DataQueryPaste(value)]`，不按字符生成 actions。
4. App 无 console Relation 允许列表新增 DataQueryPaste；App::update 分支采用以下完整核心逻辑：

```rust
Action::DataQueryPaste(text) => {
    if !text.is_empty() {
        self.with_active_data_query(|input| input.paste(&text));
        self.refresh_active_data_query_completion();
    }
    Vec::new()
}
```

5. App 测试通过 map_paste 执行 Action，覆盖四种组合。其中 Relation fixture 必须确保没有 active console，证明允许列表实际放行。断言 focus 保持、未聚焦字段不变、返回 commands 为空。
6. 添加 `data_query_paste_replaces_unicode_selection_atomically`：输入 `a中文z`，按字符索引选择中文，粘贴 `é🚀`，值为 `aé🚀z`，一次 undo/redo 完整恢复。补充中间光标与先键入再粘贴的两个独立撤销步骤、空串保持错误/选区、paste 后新编辑清除 redo。
7. 使用现有 SQL derived / completion fixture，新增 `data_query_paste_refreshes_completion_and_invalidates_submitted_query`：设置已有 submitted/derived，验证粘贴遵循 with_active_data_query 现有 submitted 清理规则，completion 按最终文本刷新；不要错误要求非空输入立刻删除所有 derived（helper 仅在两栏都空时清除 derived）。
8. 定向测试通过后复核 diff：无每字符循环、SQL 空白原样、无 submit、Relation no-console 可执行、原 overlay 路由优先级未变。记录本单元验收结果，然后继续 B，不请求 resume。

验证命令（实际执行须有新增测试，0 tests 不算通过）：

```sh
cargo test --locked --test keymap data_query_paste
cargo test --locked --lib data_query_paste
```

预期：先观察目标旧行为回归失败，然后最终命令均退出 0、所有新增用例通过。此单元验收：四种场景事件→Action→input 已贯通，选区与原子历史正确且不执行查询。

## 6. 实施单元 B：各类单元格形成完整闭环

**修改：** src/action.rs、src/input/keymap.rs、src/app.rs、src/model/cell_editor.rs。
**测试：** tests/keymap.rs、tests/relation_tabs.rs、src/model/cell_editor.rs 内 tests。

步骤依次执行：

1. 添加 `relation_cell_paste_does_not_route_to_row_paste`，在 EditCell 设置非空 yank，旧实现返回 RelationPaste 应导致断言失败。声明新增 Action 后，断言新结果只含一个 RelationEditPaste(payload)。添加 Busy 返回空 actions 的用例，避免旧错误进入内部行缓冲。
2. src/action.rs 在 RelationEditInsert 附近声明 `RelationEditPaste(String)`；App 无 console Relation 列表加入该 Action。
3. map_paste 的 Relation 分支按 edit.mode 匹配：EditCell → RelationEditPaste(value)，Busy → 空，其他 → 原 RelationPaste。已有 query 判断置于此前，防止输入栏被网格接管。只改变 Paste 路由，不动 map_relation_data 的普通按键控制。
4. 在 JsonBuffer 内新增批量 API（字段、私有方法均已有）：

```rust
pub fn paste(&mut self, text: &str) {
    if text.is_empty() {
        return;
    }
    self.record();
    self.remove_selection();
    let index = self.byte_index(self.cursor);
    self.value.insert_str(index, text);
    self.cursor += text.chars().count();
    self.sql_null = false;
}
```

5. 在 CellEditorBuffer 新增 `pub(crate) fn paste(&mut self, text: &str) -> bool`，按如下顺序实现：空串立即 false；记录 before snapshot；按 content 分发；普通 Text/JSON 保留原串，Boolean/Temporal trim 后空串 false；Boolean 整值 set、Temporal 同 kind 重建；比较 after content 与 before content，并包括 presence 是否原为 Null/Unprovided；有效变化时 presence=Value、record_presence_transition(before) 一次，返回 true，否则 false。不循环 apply_text_edit、不只调用 input_mut、不重新初始化整个 CellEditorBuffer（会丢历史）。
6. Boolean/Temporal 的底层 set/重建会清除底层历史，由最外层完整 snapshot 承担 undo/redo。普通 TextInput/JsonBuffer 可保留各自原子记录，但保证一次 CellEditorBuffer::undo 不产生残余的第二次相同粘贴撤销。新增测试先撤销 paste，再撤销先前输入，再连续 redo，检验快照与底层历史交互；如有问题，围绕本 paste API 最小修正，不全局重写历史机制。
7. App 增加以下分支，或提炼与现有 relation_edit_insert 同风格 helper：

```rust
Action::RelationEditPaste(text) => {
    if let Some(edit) = self.relation_session_mut()
        && let RelationGridMode::EditCell(state) = &mut edit.mode
        && state.input.paste(&text)
    {
        state.error = None;
    }
    Vec::new()
}
```

8. model 单测按下表覆盖行为，测试名包含 paste。Boolean/Temporal 原本 same-value 的 cursor 变化不应人为制造编辑；构造前已设草稿确保可以验证整体替换。Timestamp 必须检查小数秒及 offset，继续移动/编辑 offset 段，确认 fraction_width 同步。
9. tests/relation_tabs.rs 使用无 active console 的 Relation fixture，从 map_paste 到 App::update 执行；记录 rows/yank/行 undo 深度/原值，粘贴后这些都不变、EditCell 内容改变、commands 为空；执行 undo/redo，再 cancel 仍不写库。另测试 confirm 合法值继续走既有本地草稿路径，非法 typed 值仍保留编辑状态并显示既有错误。
10. 定向测试通过后复核：每个 typed 分支明确、空串/typed 纯空白不默认化、不遗漏 presence、不触发 insert_row/confirm；记录闭环并继续 C。

必需类型测试矩阵：

| 类型 | 重点数据与断言 |
| --- | --- |
| Text | 中文/emoji、光标中间插入、选区替换、原空白、多行文本、一次 undo/redo |
| JSON | Unicode 多行对象、选区替换、JSON null、SQL-null 标记退出、非法 JSON 继续验证 |
| Boolean | true/false/t/f、外围空白、非法非空、纯空白 no-op |
| Date | 2026-10-14 整值替换、非法日期、后续分段编辑 |
| Time | 12:34:56.123 整值替换及 parse |
| DateTime | 2026-10-14 12:34:56.123 整值替换及 parse |
| Timestamp | 2026-10-14 12:34:56.123456 +08:00、parse、offset 段后续编辑 |
| Presence/history | Null 与 Unprovided → Value → undo/redo；相同有效值不增无效历史；空串保持错误；paste 后新编辑截断 redo |

验证命令：

```sh
cargo test --locked --lib model::cell_editor::tests
cargo test --locked --test keymap relation_cell_paste
cargo test --locked --test relation_tabs paste
```

预期：旧 map_paste 回归先失败；最终全部退出 0，新增用例实际运行。此单元验收：既有可编辑类型均接收文本，原子历史/presence 正确，有 yank 也只编辑单元格。

## 7. 实施单元 C：边界回归、文档和最终门禁

**修改：** tests/keymap.rs、docs/keybindings.md；修复 A/B 中确认的必要问题仅使用既有 scope 文件。

1. 新增或扩展命名为 paste 的路由用例：有效 query.focus 但 Focus::Editor（保留原 EditorPaste）；DDL、AwaitingResult/Unavailable（无 DataQueryPaste）；EditCell 但 focus 不在 Results（无 RelationEditPaste）；Busy（无 RelationPaste）；overlay 不误入输入分支。原 Omni/Help/Profile/Redis 用例保持。至少断言实际返回 Action，不只检查非空。
2. 确認网格 `p` 仍对应 RelationPaste，Browse/VisualLine 的既有终端 Paste 行为保持；不把本次修复扩展成网格 TSV/CSV 导入。
3. docs/keybindings.md 的 Non-Vim Text Inputs 和 EditCell 两处说明：终端原生 Paste（如 macOS 默认 Cmd+V）在焦点输入框粘贴；Text/JSON 按光标/选区，Boolean/Temporal 整值替换；粘贴原子 undo，不自动提交。将裸 Command forwarding 与终端 Paste 区分，不误写为必须将 Cmd+V 映射成 SUPER+v。
4. 完整运行两个受影响集成测试套件一次：

```sh
cargo test --locked --test keymap --test relation_tabs
```

5. 功能齐备后最终 Rust 门禁一次，与 CI 的 1.94.0 一致时运行：

```sh
cargo +1.94.0 fmt --all -- --check
cargo +1.94.0 clippy --all-targets --all-features -- -D warnings
cargo +1.94.0 test --all-targets --all-features
```

如果指定工具链未安装，使用现有 Rust 1.94.1 的 `cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test --all-targets --all-features`，如实记录工具链差异，不冒充 CI 原版本运行结果。普通编译/测试错误由 Luna 修复后只重跑有关定向测试；最终确有代码变化时补跑受影响门禁。缺少数据库配置导致自动跳过的环境测试要记录，不将退出 0 等同真实数据库集成全部通过。
6. 可选补充：真实终端 SQLite fixture 中操作四种查询栏、Text/JSON EditCell 并 Cmd+V、undo/redo；有 yank 时确认不插行。PTY 只证明注入的 Paste 处理，不能证明 macOS 剪贴板交付。SSH/tmux 环境不在本次自动验收必需清单；环境失败最多一次定向修复重试后记录限制。
7. Luna 最终审查实际 diff 与 change-scope.json 一致，新增 Action 所有必要匹配已覆盖，无无关更改、新依赖或敏感内容。验证记录注明 HEAD、工作树 diff、命令、exit、测试数量、环境；不要复用 analyze 的静态 PASS 作为实现通过证据。
8. 完成自动工作流后续审查、提交/合并阶段时遵循其授权与协议；本计划不替代后续阶段的具体指令。不每步提交或更改原 main，不要求用户代为实施。

验收：A/B 功能与类型矩阵通过、边界路由和既有输入行为无回归、文档准确、Rust 门禁通过或有真实环境限制记录并完成收尾判断。

## 8. 进度与验证记录规范

每单元记录具体业务闭环，不用“本轮复核完毕”等待 resume。先从实际 diff 与当前 checkpoint 识别完成状态；未提交文件不会自动进入新 worktree，若后续环境出现依赖本地行为，明确纳入范围而不是默认继承。

所有验证追加到本任务目录 validation.md，保留历史。每条包含：实际命令、退出状态、过滤实际测试数/失败信息、相关文件与 HEAD/未提交 diff、操作系统/工具链/数据库或终端环境。无相关代码或环境变化不重复检查；旧结果标明旧版本，不当作新结果。

本 plan 阶段不运行未实现功能的 tests/fmt/clippy，不把可执行计划当作修复交付。完成 plan.md 与 change-scope.json 的内容/JSON/范围核对后，最后写入本轮 plan-e2b3c698-0314-43a1-ad15-5036ce469ede.json，status=completed。

**下一阶段首个动作：** Luna 在由自动工作流建立/选择的任务 worktree 核实起点，实施单元 A 的四种 data_query_paste 回归测试与两个视图共享的 DataQueryPaste 入口；完成定向验收后自动继续单元 B、C。
