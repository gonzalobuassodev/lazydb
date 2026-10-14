# Explorer 搜索 Ctrl-W 实施计划

**Goal:** 在 Explorer 的 `/` 与 `f` 搜索 Editing 状态支持 Ctrl-W 向左删除一个词，并即时刷新匹配结果。

**Architecture:** 保留两套搜索 Action，分别新增 DeletePreviousWord 操作，复用 TextInput 的空白分词、Unicode 边界和原子历史。通过既有 find/search 编辑入口更新匹配，不增加数据库请求。

**Tech Stack:** Rust 2024、crossterm KeyEvent、现有 App reducer 与 TextInput；无需新增依赖。

**执行方:** 实现、审查与提交合并由 Luna 执行，不启动子 Agent。计划阶段只写指定任务目录。

本计划依据已读取的 analysis.md，并按 writing-plans 技能整理；用户的自动工作流、指定目录和阶段限制优先于技能默认的 worktree、docs/plans、逐步提交及执行方式询问。此阶段不实施、不提交；不另外创建 docs/plans 文档。

## 基线和范围

起点及本阶段确认的 HEAD：`df42eac22eb662d08066f1e25691b1787181a529`。`git status --short` 无输出；没有依赖未提交行为。checkpoint.json 当前不存在，不创建或更新。不得修改插件 state.json 或旧回执。

任务名称建议为“Explorer 搜索按词删除”，分支名称建议为 `fix/explorer-search-word-delete`，最终由 Luna 确定。本阶段不创建分支；实施前再次检查原工作空间状态，若使用新 worktree，应从指定起点创建并确认相关文件没有依赖未复制的本地改动。

整个需求按一个端到端验收单元交付。限定修改文件：

- `src/action.rs`：两个新增 Action。
- `src/input/keymap.rs`：Editing 按键映射与 find 修饰符过滤。
- `src/app.rs`：新增 Action 的门控与 reducer。
- `src/help.rs`、`docs/keybindings.md`：快捷键说明。
- `tests/keymap.rs`、`tests/catalog_reducer.rs`：输入与状态回归覆盖。
- `tests/ui_render.rs`：共享快捷键目录文字变化涉及的 help/渲染回归断言（仅更新直接受影响的内容）。

上述 8 个预计修改文件全部列入同目录 change-scope.json。没有预计新增/删除/重命名的仓库文件，没有未提交依赖文件；分析参考用的模型、CONTRIBUTING.md、CI 配置不列入范围。任务目录内报告和协议文件不属于交付代码范围。

默认不改 TextInput、workspace 模型、依赖、配置、数据库适配器或 Confirmed 分支。只有相关测试确实暴露必要问题时才调整范围，并在任务目录说明理由。

## 步骤 1：构造实际回归用例

在 `tests/keymap.rs` 现有 Explorer 测试附近添加以下命名测试：

- `editing_explorer_find_maps_ctrl_w_to_word_delete`。
- `editing_explorer_search_maps_ctrl_w_to_word_delete`。
- `editing_explorer_find_rejects_unhandled_modified_characters`。
- `confirmed_explorer_searches_preserve_ctrl_w_window_bindings`。

复用 `ctrl`、`key`、`Keymap::default` 和 `open_find/open_search`。Editing 的 Ctrl-W 必须分别返回 `ExplorerFindDeletePreviousWord` 与 `ExplorerSearchDeletePreviousWord`；普通 w 与 Shift-W 插入；Ctrl-U 清空；find 的 Ctrl-R、Alt-W 返回 None。Confirmed 的 Ctrl-W 仍使用既有窗口前缀，可用后续 h/l 或现有窗口绑定断言完整序列；不得仅断言“不是删除”。保持 undo/redo 的平台修饰符规则和 search 的 Home/End 列表端点映射。

在 `tests/catalog_reducer.rs` 复用 connected_app 与已加载 catalog 测试装置，添加：

- `explorer_find_ctrl_w_refreshes_matches_and_restores_origin`：输入 `users extra`，按键映射后调用 App::update；删除后值为 `users `、字符光标为 6、重新匹配 users、current 重置。再次删除至空，恢复打开前 selection/scroll。
- `explorer_search_ctrl_w_refreshes_local_results`：同样输入，删除后 generation 增加、匹配 users、状态 Ready；删空为 Idle，结果清空；所有操作返回空 Command。
- `explorer_searches_word_delete_preserves_atomic_history`：两个模式各验证删除一次后 undo 恢复原片段，redo 重做。
- `explorer_searches_word_delete_handles_boundaries_without_console`：在实际 Relation 或 Dashboard tab 上确认无活动 console，验证两个 Action 均未被门控丢弃；使用空查询、尾部空白、Unicode 输入覆盖安全边界。需要时直接设置 TextInput 光标验证中部删除保留后缀，不新增用户快捷键。

避免让宽泛字符串匹配假通过：输入前装载确定节点，删除后断言确切节点 ID，而不只检查结果非空。空输入的 Ctrl-W 不改变 value/cursor/generation，不生成命令。已有 TextInput 测试承担算法细节，无须复制整个模型测试集。

先运行 `cargo test --test keymap explorer` 和 `cargo test --test catalog_reducer explorer`，记录真实初始失败。缺少 Action 导致编译失败可以作为初始证据；声明 Action 后应补充运行得到映射或 reducer 的行为失败，不把编译失败视为功能复现。

## 步骤 2：实现完整输入链路

1. 在 `src/action.rs` 的 ExplorerFindBackspace、ExplorerSearchBackspace 附近分别声明无参数 `ExplorerFindDeletePreviousWord`、`ExplorerSearchDeletePreviousWord`。
2. 在 `src/input/keymap.rs` find Editing 分支保留 undo/redo 检查，在普通字符匹配前处理 CONTROL + Char('w') 和原 Ctrl-U；然后拒绝非 NONE/SHIFT 修饰符。search Editing 同样在现有修饰符拒绝之前处理 Ctrl-W。

局部逻辑采用以下形式，各分支替换对应 Action 名称：

```rust
if event.modifiers == KeyModifiers::CONTROL {
    match event.code {
        KeyCode::Char('w') => return Some(Action::ExplorerFindDeletePreviousWord),
        KeyCode::Char('u') => return Some(Action::ExplorerFindClear),
        _ => {}
    }
}
if !event.modifiers.is_empty() && event.modifiers != KeyModifiers::SHIFT {
    return None;
}
```

3. 在 `src/app.rs` 无活动 console 门控中，与已有 ExplorerFindBackspace/ExplorerSearchBackspace 相邻的允许列表加入新增 Action。保留其他门控条件。
4. 在 reducer 中添加：

```rust
Action::ExplorerFindDeletePreviousWord => {
    self.explorer
        .edit_find_input(|query| query.delete_previous_word());
    Vec::new()
}
Action::ExplorerSearchDeletePreviousWord => {
    self.edit_explorer_search(|query| query.delete_previous_word())
}
```

5. 再运行步骤 1 的两个定向测试命令。若新增测试暴露 fixture 不完整，修正 fixture，保留业务断言，不通过弱化断言规避问题。

## 步骤 3：同步用户说明

遵循 CONTRIBUTING.md 的快捷键变更要求：

- `src/help.rs` 的 ExplorerFindEdit 与 ExplorerSearchEdit 键列表均包含 `type / Backspace / Ctrl-w / Ctrl-u`。说明文字明确 Ctrl-w 删除前一词、Ctrl-u 清空。
- `docs/keybindings.md` 的 Explorer Editing 说明加入 Ctrl-W，说明词边界为空白，保留 Enter/Esc、导航及 Confirmed 的 n/N 行为。
- 不引入新配置。若已有 help/渲染断言依赖文字，只更新直接相关断言并运行对应定向测试。

## 步骤 4：最终验证与审查

验证要求按来源区分：

- 用户需求验收：`/` 与 `f` Editing 中 Ctrl-W 删除左侧一个词。由上述自动回归测试覆盖，匹配刷新和既有操作回归用于确认实现完整性；用户没有要求额外的人工验收门禁。
- 项目强制要求：CONTRIBUTING.md 规定共享快捷键目录与 docs/keybindings.md 同步，以及下面的格式、clippy、全量测试检查。
- 实施定向验证：步骤 1、2 的 keymap/catalog_reducer 测试用于定位失败与确认闭环。快捷键文字变化后运行 `cargo test --test ui_render explorer`，预期退出 0；测试选择若没有覆盖受影响 help 断言，再按实际测试名补一个精确命令并记录。此处不新增与功能无关的必需门禁。
- 补充建议：真实终端/PTY 操作、未修改模型的额外单测、外部数据库环境验证。缺少这些补充证据不自动阻塞本功能完成，需如实记录其边界。

功能完整后一次执行项目要求，预期各命令退出 0：

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

工具链需兼容 Cargo.toml 的 Rust 1.94；CI 使用 1.94.0。若实现未改模型且定向回归覆盖充分，不额外反复运行全量测试。全量测试中的外部数据库测试若因环境变量缺失跳过，需明确记录，不能宣称数据库环境通过。

实际终端/PTY 操作为补充检查：分别打开 `/` 与 `f`，输入 `alpha beta` 后 Ctrl-W，观察 `alpha ` 与结果刷新。不是本项目额外强制要求；环境受限最多一次针对性修复重试，然后记录限制，由 Luna 收尾审查决定是否补证。不要无限循环同一环境失败。

在本任务目录 `validation.md` 追加每条实际命令、退出结果、相关代码版本/工作区状态和环境，区分分析阶段静态证据与实现后验证。最终审查确认新增 Action 的映射、门控、reducer 三处齐全，未改变 Confirmed 窗口操作或 Home/End 结果导航，未加入异步副作用。

## 验收与后续协议

验收需同时满足：两个 Editing 输入框支持 Ctrl-W；词删除复用空白分词、Unicode 安全、空输入无害；结果即时更新；undo/redo 原子；无活动 console 场景有效；普通字符、Ctrl-U 与确认后窗口操作正确；快捷键 catalog 和文档一致；项目要求检查结果已如实记录。

本计划完成后由 Luna 命名并推进实现单元；业务功能未完成不构成阻塞。本轮计划阶段先写入 change-scope.json、完成计划文件与静态复核，最后仅写指定回执 plan-7a22e90e-f8f2-4a40-99ca-d5f0a77e3ba2.json，token 为 7a22e90e-f8f2-4a40-99ca-d5f0a77e3ba2，stage 为 plan，status 为 completed。不复用或覆盖 analyze 回执；不得更新 checkpoint.json 或 state.json。
