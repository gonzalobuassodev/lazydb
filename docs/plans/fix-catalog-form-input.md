# Issue #15 Catalog Form Input Implementation Plan

**Goal:** 使 New database、New login user、New role 三个弹窗可正常输入、删除、移动光标和粘贴，并保持既有 SQL 预览、取消及密码脱敏行为。

**Architecture:** 补齐现有 Keymap → Action → App::update → CatalogDraft 的输入链路，复用 TextInput、SecretTextInput 和 ProfileInput。仅对 Database / Role 增加缺失分发与模型操作，保留其他目录对象的专用处理，不建立新的输入框架。

**Tech Stack:** Rust 2024、crossterm、ratatui、现有 App reducer 和 Rust 集成测试。

---

## 执行约束与基线

本计划使用 writing-plans 技能制定。用户已选择自动执行；后续实现、审查、纠偏及收尾均由 Luna 完成，不询问执行方式，不启动子 Agent。用户指定的任务路径及阶段协议覆盖技能中的默认 docs/plans 路径、执行技能要求和每步提交建议。本阶段不实施、不提交、不创建 worktree、不改变 index。

- 分析来源：同目录 analysis.md；issue：https://github.com/yelog/lazydb/issues/15。
- 原工作区：`/Users/yelog/workspace/tui/lazydb`；目标 main；指定起点及本阶段实查 HEAD 均为 `0fa5d249ffabc8f8014107e09f6e0dd80cee703e`。
- 本阶段 `git status --porcelain=v1`、工作区与暂存区 diff stat 均无输出，没有未提交依赖。后续 worktree 不依赖主工作区的未提交文件；若执行前出现新改动，只读确认其归属，不清空、不 stash、不带入无关改动。
- checkpoint.json 本阶段目录清单中仍不存在；不创建、不修改它。state.json 由插件维护，模型不修改。
- 建议名称：`修复目录创建表单输入`；建议分支 `fix/issue-15-catalog-form-input`，由自动流程最终命名。
- 预计修改八个业务/测试文件，精确范围见 change-scope.json。无新增生产依赖、驱动修改、SQL 生成改动或数据库迁移。

**对分析文本的澄清：** analysis.md 第 2.4 节仍有“Name 上 Enter → Preview 是旧错误”的早期表述，与其后经既有设计文档确认的决策不一致。本计划以确认后的契约为准：普通文本 Enter 仍进入 Preview。修正的是 focus_kind 把所有字段当 Action 的提示错误，不能取消现有 Enter 行为。

## 需求、门禁与补充检查的区别

| 类别 | 必须满足的内容 | 来源 |
| --- | --- | --- |
| 用户需求 | 三个创建弹窗可输入，按共性原因修复 | 当前用户及 issue #15 |
| 修复完整性 | 当前字段的文字编辑、粘贴、删除、光标移动、undo/redo 可用，不能只接通 Insert | 已确认相邻输入缺口与完整表单行为 |
| 项目交互契约 | 普通文本 Enter 预览；toggle 的 Enter/Space 切换；Review/Cancel 准确；密码草稿及 Action Debug 脱敏；不绕过 busy/SQL Review 边界 | 已有 `docs/plans/2026-09-14-catalog-database-role-form-redesign.md`、当前代码 |
| 项目自动化门禁 | rustfmt、clippy、全量 Rust 测试，按 CI 已有参数运行 | `.github/workflows/ci.yml:85` 起 |
| 本修复必需证据 | 新测试先暴露漏接，再验证 Keymap/map_paste → App → 草稿实际值；相关旧测试通过 | 防止现有只测映射/草稿的覆盖缺口再次出现 |
| 补充建议，required:false | PTY/真实终端手测、隔离 PostgreSQL 中真实创建和清理 | 本计划新增补充建议，非用户/项目强制门禁 |

可选手测缺少终端或数据库不阻塞自动实现，记录 deferredChecks 即可。无需运行全部数据库容器矩阵、发布构建或在用户数据库创建测试对象。

## 代码定位及改动范围

行号基于起点，仅帮助定位，实施时以符号为准。

| 文件 | 修改位置及目的 |
| --- | --- |
| `src/app.rs` | CatalogEditorInsert 等九个编辑动作（9542–9986 附近），补 Database / Role；新增密码动作处理 |
| `src/model/catalog_editor.rs` | CatalogDraft 编辑分发（3687 起）、focused_action（3603）、现有字段访问器；补缺失方法 |
| `src/action.rs` | CatalogEditorInsert/Paste 附近增加密码专用脱敏载荷动作 |
| `src/input/keymap.rs` | map_catalog_editor_form、map_paste；接通输入及粘贴，显式保留文本 Enter 语义 |
| `tests/catalog_editor_reducer.rs` | 复用 simple_catalog_editor，覆盖 App 编辑及密码状态 |
| `tests/catalog_editor_state.rs` | 覆盖模型缺口、焦点分类、非文本/禁用字段保护 |
| `tests/keymap.rs` | 三类对象创建 fixture，覆盖真实键盘/粘贴映射后更新草稿 |
| `tests/ui_render.rs` | 复用现有 render fixture 检查 footer 随焦点变化及密码掩码 |

仅读取参考、无需修改：profile_manager.rs 的 ProfileInput、secret_text_input.rs、ui/catalog_editor.rs、tests/catalog_mutation.rs、既有设计文档、CI。不要因读取参考而扩大 scope。此处不新增 docs/plans 文档。

## 单元一：三个弹窗的键盘编辑闭环

本单元同时接通密码字符的脱敏路由，结束时三个弹窗均能通过真实按键完成文字编辑。

### Task 1：增加能暴露共因的失败回归

**文件：** tests/catalog_editor_reducer.rs、tests/keymap.rs。

1. 在 reducer 测试中复用 simple_catalog_editor，新增三组 fixture：Database、RoleDraft::new(true)、RoleDraft::new(false)，为 editor 设置匹配 object_type。
2. 在 keymap 测试中使用 CatalogEditorState::new + Profile anchor + select_object_type 构造三个真实创建入口，设置 Overlay::CatalogEditor；不要只手工塞草稿以掩盖创建类型错误。
3. 新测试名统一使用 `catalog_profile_form_` 前缀，便于精确筛选；循环覆盖三类对象，失败消息包含对象类型。
4. 给三类 Name 字段发送真实字符事件，再把返回 Action 传给 app.update，断言最终 value。基础序列为 `a界b`，预期草稿为 `a界b`；现有代码应在值断言失败。
5. 补 reducer 级完整编辑序列：Home → Right → Delete 得 `ab`；Insert('中') 得 `a中b`；End → Left → Backspace 得 `ab`；Right → Insert('x') 得 `abx`。分别断言值和 cursor，不用“没有 effect”代替成功。
6. 运行新增用例，保存实际失败原因和命令；若出现夹具/编译错误先修测试，必须确认至少一例是当前业务遗漏导致的失败。

**命令（timeout=600000ms）：**

```sh
cargo test --locked --test catalog_editor_reducer --test keymap catalog_profile_form_ -- --test-threads=1
```

**验收：** 修复前可重复观察到按键产生动作但值不变；失败不依赖数据库、PTY 或真实连接。

### Task 2：补齐九个 App 编辑动作与模型操作

**文件：** src/app.rs、src/model/catalog_editor.rs、tests/catalog_editor_reducer.rs、tests/catalog_editor_state.rs。

1. 在 Insert、Backspace、Delete、DeletePreviousWord、DeleteToStart、MoveLeft、MoveRight、MoveHome、MoveEnd 九个 App 动作里，为 Database / Role 加最小转发，调用 CatalogDraft 同名方法后返回 Vec::new()。保留其余对象原路径。
2. 采用与以下逻辑等价的局部分支；不把 Constraint 等全部迁移到 CatalogDraft，因为其既有方法覆盖不同：

```rust
if let Some(draft) = self.catalog_editor.as_mut().and_then(|editor| editor.draft.as_mut())
    && matches!(draft, CatalogDraft::Database(_) | CatalogDraft::Role(_))
{
    draft.insert(character);
    return Vec::new();
}
```

实际代码按文件导入习惯使用全限定名或既有 import。其余八项用对应方法，无需额外状态。

3. CatalogDraft 的四个移动方法补 Database 分支：从既有 selected_input_mut 取得 Option<&mut TextInput>，调用 move_left/right/home/end。
4. CatalogDraft 的 delete_previous_word/delete_to_start 补 Database 和普通 Role 字段，调用 TextInput 现有方法；Role Password 特殊分支必须放在普通 Role 之前，保留 password_delete_* 与 sync_password。
5. 测试 Ctrl+W / Ctrl+U 的字段内效果，并断言其他字段不变；例如 Name 为 `alpha beta` 时 End + DeletePreviousWord 得 `alpha `，再 DeleteToStart 得空。
6. 用 App::update(CatalogEditorFocusFormField(...)) 切换 Comment、ConnectionLimit、Database Owner/Template；不要只手改 Database.focus 而遗漏 selected_field 同步。
7. 覆盖 toggle、Review、Cancel、disabled 字段的编辑 no-op；数据库访问器的历史 selected_field 兼容不能让输入落入其他字段。若发现该边界与此修改直接相关，在原访问器内最小修复并加断言，不全局移除 selected_field。
8. 覆盖 Database edit 的创建专属字段不可写、MariaDb/MySql/ClickHouse 仅已有受支持字段可写；不扩展数据库编辑能力。

**命令（每条 timeout=600000ms）：**

```sh
cargo test --locked --test catalog_editor_reducer --test keymap catalog_profile_form_ -- --test-threads=1
cargo test --locked --test catalog_editor_state --test catalog_editor_reducer
```

**验收：** 三类 Name 的所有编辑动作准确；多字节字符不被破坏；非目标字段保持原值；既有 Table/Constraint 等 reducer 回归通过。

### Task 3：接通安全的密码字符输入

**文件：** src/action.rs、src/input/keymap.rs、src/app.rs、tests/keymap.rs、tests/catalog_editor_reducer.rs。

1. 增加 `CatalogEditorPasswordInsert(ProfileInput)` 与 `CatalogEditorPasswordPaste(ProfileInput)`。ProfileInput 在 action.rs 已导入，无需新增依赖或移动该类型。
2. 在 map_catalog_editor_form 普通字符匹配中，当 draft 为 Role 且 focus=Password 时先生成 PasswordInsert(character.into())，其他文本保持原 Insert。遵循现有修饰键与 Release 过滤，不改变全局大小写规则。
3. App 仅在可编辑 Form、非 busy、Role Password 焦点时消费专用密码动作；Insert 通过 input.character() 检查单字符，再调用 password_insert，Paste 调用 password_paste(input.value())。其他焦点/对象/页面拒绝专用密码动作。
4. LoginRole 和 Role 都用真实键盘输入合成密码并执行移动/删除；断言 RedactedSecret 更新正确、删空后 None、普通字段编辑不改密码。
5. 测试 PasswordInsert/PasswordPaste 的 Debug 不包含合成载荷，草稿 Debug 同样不包含密码；不要将密码打印到失败日志。
6. 真实入口只生成脱敏动作；保留已有普通 Insert/Paste 的兼容行为，不扩展为重写所有输入 Action。

**命令（timeout=600000ms）：**

```sh
cargo test --locked --test catalog_editor_reducer --test keymap catalog_profile_form_ -- --test-threads=1
```

**单元一复核：** 三类真实创建入口 → 字符映射 → App 更新 → 当前字段值形成闭环；九个动作无遗漏，密码输入在 Action 和草稿处均脱敏。记录通过后继续单元二，不等待 resume。

## 单元二：粘贴、焦点提示与预览闭环

### Task 4：终端粘贴及撤销重做

**文件：** src/input/keymap.rs、src/app.rs、tests/keymap.rs、tests/catalog_editor_reducer.rs。

1. 写失败测试：`map_paste(value, &app)` 返回动作，依次送入 app.update，断言 Database/两种 Role 当前字段实际值。
2. map_paste 的 CatalogEditor 分支仅对 Form、!is_busy、Database/Role 且 focus_accepts_text 放行；普通文本返回一个 CatalogEditorPaste，Role Password 返回一个 PasswordPaste(value.into())。
3. 其他对象既有分支不变；不借机修 View/MaterializedView 粘贴或 Owner picker 功能。
4. 粘贴使用 Unicode 和空格，如 `alpha 界 beta`。assert 动作数为 1，并验证一次 Undo 恢复粘贴前值、Redo 恢复粘贴后值；密码也需验证 secret 同步和 Debug 脱敏。
5. 表驱动验证 Loading、SqlPreview、busy、toggle、Review、Cancel、disabled 字段粘贴无动作且草稿不变。使用项目现有合法 busy operation fixture，避免编造枚举字段。

**命令（timeout=600000ms）：**

```sh
cargo test --locked --test keymap --test catalog_editor_reducer catalog_profile_form_ -- --test-threads=1
```

**验收：** 三个弹窗真实 paste 入口可用，单次粘贴可原子撤销，非输入状态不接收粘贴。

### Task 5：修正焦点分类并保留 Enter 契约

**文件：** src/model/catalog_editor.rs、src/input/keymap.rs、tests/catalog_editor_state.rs、tests/keymap.rs、tests/ui_render.rs。

1. 为 Database/Role 的 focus_kind 增加表驱动测试：Name/Comment 为 Text、Password 为 Text、布尔项为 Toggle、Review/Cancel 为 Action；只有 Role 有 Password。
2. 修改 focused_action：Database/Role 提取真实 draft.focus，和现有 View 等共用最终 `matches!(focus, Review | Cancel).then_some(focus)`，不让文本/toggle 返回 Some(Review)。
3. keymap Enter 优先级保持：toggle → ToggleFocused；Review → Preview；Cancel → Cancel；Database/Role text_focus → Preview；其他情况保留原对象规则。不要让所有对象文本 Enter 都变成 Preview。
4. 保留 `role_editor_uses_catalog_editor_field_keymap` 中 Name Enter→Preview 的断言，再覆盖 Database 和 LoginRole；按 Tab/BackTab 走到 Review、Cancel，断言焦点与映射。
5. 在 ui_render 复用 fixture 检查文本焦点显示 Type/edit、toggle 显示 Space/toggle、动作显示 Enter/activate，以及角色密码显示掩码而非明文；不修改 UI 布局。
6. 对有效表单触发 Preview，复用现有离线规划 fixture/测试核对 SQL 预览流程；无连接 fixture 仅证明 Preview 映射，不能声称创建成功。复用 tests/catalog_mutation.rs 对 Database/Role 规划及脱敏的现有回归，不为本修复引入真实数据库前置条件。

**命令（每条 timeout=600000ms）：**

```sh
cargo test --locked --test catalog_editor_state --test catalog_editor_reducer --test keymap
cargo test --locked --test ui_render --test catalog_mutation
```

**单元二验收：** 文本/toggle/动作提示对应当前焦点；文本 Enter 保持预览，toggle 不进入预览；Review 不直接 Apply；粘贴/密码/取消与其他旧表单无回归。

## Task 6：最终门禁与 Luna 收尾复核

**业务文件：** 不另增文件；若检查发现缺陷，仅在上述范围内修复直接相关问题。**证据：** 任务目录 validation.md。

1. 检查 diff，仅包含八个预计文件内的任务相关改动；确认没有修改 Cargo.lock、驱动、ProfileInput 定义或无关业务。若确需扩大范围，先说明依据并更新 change-scope.json，不能隐藏依赖。
2. 检查工具链：`rustup toolchain list`（timeout=120000ms）。CI 使用 1.94.0；分析阶段本地为 1.94.1。优先现有 1.94.0；若缺失，可按环境约定安装，失败则准确记录版本偏差，由收尾审查判断本地替代证据，不把可选手测当作补偿门禁。
3. 完成后执行项目既有 Rust 自动门禁各一次，不在每个任务后重复全量验证：

```sh
cargo +1.94.0 fmt --all -- --check
cargo +1.94.0 clippy --all-targets --all-features -- -D warnings
cargo +1.94.0 test --all-targets --all-features
```

fmt timeout=120000ms；clippy/test 各 timeout=600000ms。若实际使用 1.94.1，记录实际命令，不能声称与 CI 工具链相同。全量测试中依赖外部数据库的跳过/失败按实际结果注明，既有 CI 服务矩阵在 CI 环境执行。

4. 定向/全量失败需区分改动回归与环境问题；普通代码错误自行修复并只重跑相关检查。超时先检查 Cargo 锁、进程和最近代码变化，不立即重复相同命令；环境问题最多一次有针对性的修复重试，随后记录限制交 Luna 收尾审查，禁止 timeout:0。
5. validation.md 追加实际命令、退出结果、相关文件、commit/工作区状态、环境及日志位置。旧结果引用保持原文与旧版本，不改写成当前通过。
6. Luna 对最终 diff 复核：九个动作完整性、两个角色类型覆盖、数据库字段能力、密码 Action/草稿脱敏、paste 原子撤销、Enter 契约、busy 防护，以及其他对象路径未改变。收尾阶段按自动工作流的授权处理提交/合并；本计划不添加逐步提交要求。

**最终验收：** 用户报告的三类弹窗均通过自动化输入链路回归；相关与全量自动门禁有实际证据或明确的环境限制及收尾决定；没有以 fixture 空 effect、旧测试或静态脚本替代修复验证。

## 可选补充验证（deferredChecks，required:false）

- 真实终端/PTY：依次打开三类弹窗，输入名称/备注/密码、光标编辑、粘贴、切换权限，进入 SQL Review 后取消。缺少终端则记录未做，不阻塞。
- 隔离测试 PostgreSQL：仅在已有受控测试环境验证实际创建与清理。没有测试数据库不阻塞；不得在用户业务数据库创建临时对象。

这两项由收尾审查决定是否补充，均非本计划新增的强制人工门禁。

## 已有证据与执行起点

分析阶段旧基线命令：`cargo test --locked --test catalog_editor_state --test catalog_editor_reducer --test keymap editor -- --test-threads=1`，退出 0，52 个通过；这是 `editor` 名称过滤后的旧用例，未覆盖新修复，详见 validation.md 和 baseline-tests.log。本 plan 阶段不重跑无代码变化的编译测试。

后续第一个未完成的可验收单元是单元一：在任务 worktree 中从 Task 1 新增失败回归开始，随后连续完成 Task 2/3，再进入单元二及最终门禁。无需用户再次选择执行方式。
