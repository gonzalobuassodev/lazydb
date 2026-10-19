# New Database 默认 Owner Implementation Plan

> **执行者：Luna。** 用户已选择自动工作流继续实施；依次完成本计划，不询问执行方式，不启动子 Agent。当前 GPT-6.1 Sol 仅完成 plan 阶段，不实施业务代码。用户阶段与输出目录约束优先于 writing-plans 技能的默认文档路径、提交步骤及交接选项。

**Goal:** PostgreSQL New Database 打开时，Owner 默认填入目标连接的当前数据库用户，并保持可编辑。

**Architecture:** 在 App 创建草稿成功后同步注入默认值。复用连接探测的 `ServerInfo.current_user`，仅在目标 profile 与该连接身份一致时使用，缺失或空白时回退目标 profile 的配置用户名。底层 DatabaseDraft、UI、SQL 和异步角色加载协议保持原有设计。

**Tech Stack:** Rust 2024 / Rust 1.94.0、App reducer、TextInput、Cargo integration tests。

---

## 基线与改动范围

来源：同目录 `analysis.md`。plan 阶段重新读取并确认 HEAD 为 `7091bbc746d040f8441224d134179e224f82c868`，`git status --porcelain=v1 --untracked-files=all` 无输出。当前不存在需复制到任务 worktree 的未提交依赖。`checkpoint.json` 当前不存在，未读取或修改 `state.json`。

预计仅修改以下仓库文件，已同步到 `change-scope.json`：

| 文件 | 改动 |
| --- | --- |
| `src/app.rs` | 账号来源 helper、创建草稿后的初始化及三个入口调用 |
| `tests/catalog_editor_reducer.rs` | 默认值、回退、入口、身份隔离和用户编辑的 reducer 回归 |

参考但不修改：`src/model/catalog_editor.rs`、`src/model/text_input.rs`、`src/model/explorer_add.rs`、`src/db/postgres.rs`、`src/ui/catalog_editor.rs`、`tests/catalog_editor_state.rs`、`CONTRIBUTING.md`、`.github/workflows/ci.yml`。不额外生成 docs/plans 文件，不修改 Cargo.lock 或配置；这是运行时草稿初始值修复，不涉及 CONTRIBUTING 的用户配置默认项变更规则。

后续实施阶段在工作流创建/指定的任务 worktree 中修改业务代码。若恢复时出现新 diff，先辨认用户内容与任务内容，不清空、不 stash、不提交整个原工作区。若发现确实需要第三个文件，先将具体文件和必要理由补入范围清单及计划，再修改；普通取舍自行解决。

## 需求、门禁和补充验证

**用户需求及本修复的验收条件：** Owner 是实际草稿值；服务端有效用户优先；无服务端用户时使用同一目标连接的配置账号；各个创建入口一致；Name 留空、焦点留在 Name；用户修改/清空后不再自动回填；取消重开才恢复默认值；已有数据库编辑保留 definition.owner；Schema 默认值保持原行为；其他引擎不新增 owner 行为。

**项目明确要求：** `CONTRIBUTING.md:6–12` 与 `.github/workflows/ci.yml:85–87` 要求 fmt、all-targets/all-features Clippy 和全量 Rust 测试；`CONTRIBUTING.md:57–65` 要求 App::update 不执行 IO 等现有设计规则。执行下方项目门禁一次并记录真实结果。CI 的平台分发、安装器、其他数据库服务矩阵仍属于远端 CI，不要求为本次 App 初始化修复在 macOS 本地逐项搭建全部服务。

**本计划选择的定向验证：** 新增 reducer 测试及现有 catalog reducer/state 回归，用来直接证明需求；不是用户额外指定的人工门禁。

**补充建议（required:false）：** PTY/终端连接 PostgreSQL 后打开表单，查看默认值、编辑再取消。`CONTRIBUTING.md:35–52` 明确数据库创建不是标准 suite 必需步骤，本任务不要求真实 CREATE DATABASE、CREATEDB 权限、截图或外部数据库环境。缺少这些条件不能阻塞自动工作流。

## 一个端到端实施单元：默认 Owner 初始化与编辑保持

### Task 1：建立实际入口回归证据

**修改：** `tests/catalog_editor_reducer.rs`。

1. 复用现有 `opening_create_on_schema_uses_capability_ordered_options` 的连接夹具方式：创建目标 PostgreSQL profile，配置 `user=login_user`；执行 `RequestConnect` 并捕获 generation，再发送 `ConnectionSucceeded`，server.current_user 使用 `effective_role`，能力使用 `PostgresAdapter::catalog_mutation_capabilities_for_version(150_000)`。只构造状态，不启动数据库连接。
2. 增加少量本文件私有夹具：创建连接 app、借用 Database draft、通过真实添加菜单打开 Database。fixture 只服务于新增 case，避免重构无关旧测试。
3. 添加 `new_database_owner_uses_effective_user_from_add_menu`：选中 `ExplorerNodeId::Profile(profile_id)`，发送 `OpenExplorerAdd`；在 overlay menu.options 中按 `ExplorerAddKind::Database` 找索引；发送 `ExplorerAddSelect(index)`、`ExplorerAddConfirm`。断言 Overlay::CatalogEditor、page=Form、Database draft.owner=`effective_role`、name 为空、focus=Name；返回命令中不新增 `LoadCatalogOwnerContext`。
4. 添加 `new_database_owner_uses_effective_user_from_object_picker`：选中同一 profile 后发送 `OpenCatalogCreate`，按 editor.options 中 Database 的位置发送 `CatalogEditorMove` 和 `CatalogEditorSelect`，断言同一默认值。选项索引按真实内容求得，不硬编码菜单布局。
5. 单选直达路径复用可达的非 PostgreSQL单选 Database 入口，验证不会注入 owner；PostgreSQL 当前 profile capabilities 若总是多选，则不伪造线上不存在的单选能力，不为仅测分支新增生产开关。后续公共初始化 helper 覆盖单选调用处，并在代码复核确认；已有 Schema 单选直达测试提供该调用点回归。
6. 在改业务代码前运行 `cargo +1.94.0 test --test catalog_editor_reducer new_database_owner_`，tool timeout=600000ms。预期至少菜单与对象选择的 owner 断言失败（实际为空）；若是编译错误先修测试，不把编译错误当作有效红灯。记录确切失败用例与命令到 `validation.md`。

**本任务复核：** 测试必须从 App action 进入表单，不只手工创建 DatabaseDraft 并检查字段。若某路径被现有能力禁用，明确记录并换成可达入口验证，不修改能力以迎合测试。

### Task 2：最小同步实现，覆盖所有草稿创建位置

**修改：** `src/app.rs`。

1. 将 `default_schema_owner`（基线约 :21561）替换为 `default_catalog_owner(profile_id: Uuid)`。完整逻辑如下，按实际 imports 调整名称即可：

```rust
fn default_catalog_owner(&self, profile_id: Uuid) -> String {
    self.connection
        .active_identity()
        .filter(|identity| identity.profile_id == profile_id)
        .and_then(|_| self.connection.server.as_ref())
        .and_then(|server| server.current_user.as_deref())
        .filter(|user| !user.trim().is_empty())
        .or_else(|| {
            self.profiles
                .iter()
                .find(|profile| profile.id == profile_id)
                .and_then(|profile| profile.user.as_deref())
                .filter(|user| !user.trim().is_empty())
        })
        .unwrap_or_default()
        .to_owned()
}
```

2. 增加一个小的 App 私有关联函数，限定新建草稿的初始化；通过 TextInput::set 设置初始值并清除编辑历史：

```rust
fn initialize_catalog_create_owner(editor: &mut CatalogEditorState, owner: String) {
    if editor.mode != CatalogMutationMode::Create {
        return;
    }
    match editor.draft.as_mut() {
        Some(crate::model::catalog_editor::CatalogDraft::Database(draft))
            if draft.database_kind == DatabaseKind::Postgres =>
        {
            draft.owner.set(owner);
        }
        Some(crate::model::catalog_editor::CatalogDraft::Schema(draft)) => {
            draft.owner.set(owner);
        }
        _ => {}
    }
}
```

3. 在 `OpenCatalogCreate`（约 :8336）将 selection.anchor 对应的 profile_id 在移动 anchor 前取出。按 `CatalogMutationAnchor` match：Profile 用 profile_id，Catalog 用 id.profile_id，Group 用 schema.profile_id，Principal 用 entry.id.profile_id。用该 profile 查 kind 及默认账号；将 editor.database_kind 设为这个目标 kind，替代 active_profile kind 的误绑定。这个修正只限定在当前创建路径，不扩展连接选择逻辑。单选创建成功后调用初始化函数，替换原 Schema 专用赋值。保留其后的 Schema owner-context 加载逻辑。
4. 在 `CatalogEditorSelect`（约 :8898）借用 mutable editor 前，先根据当前 editor.anchor 求目标 profile 并算默认账号。只有 `select_catalog_editor_option` 返回 true 后才初始化；busy、无效选项不得重置已有字段。保持原 View 能力配置调用。
5. 在 `open_profile_catalog_create`（约 :24606）用函数参数 profile_id 求默认账号；在 `select_object_type` 成功后、挂载 editor 前调用同一初始化函数。保留 read-only、active identity、capability 检查。
6. 如果 anchor → profile_id 的 match 需要复用，可增加本文件一个小的私有函数，避免复制四个分支；不修改公共 CatalogMutationAnchor API 或模型构造签名。
7. 不在 `CatalogOwnerContextLoaded`、焦点移动、插入文本、Review 或 render 时调用初始化函数。不要加载新 owner context 来实现默认值，不改 SQL 和驱动。
8. 再运行 Task 1 的定向命令，tool timeout=600000ms，预期新增测试通过。出现失败自行定位并修复，不结束为 blocked。

**本任务复核：** 检查三个调用位置均仅在新草稿生成时注入；配置 fallback 是目标 profile；Schema 原有默认账号和角色列表加载没有丢失；数据库种类来自目标 profile；无新增 IO/Command/持久化字段。

### Task 3：补齐边界与用户编辑的行为回归

**修改：** `tests/catalog_editor_reducer.rs`。

1. 增加 `new_database_owner_falls_back_to_target_profile_user`，表驱动覆盖 server.current_user 为 None、空字符串、纯空白；预期配置账号 `login_user`。覆盖配置也缺失/空白时 owner 为空。有效角色字符串保留原始内容，不 trim 后再写入。
2. 增加 `new_database_owner_does_not_use_another_profiles_user`：active A 的服务端用户为 role_a，通用创建 anchor 指向 PostgreSQL B，B 配置用户为 login_b，预期仅使用 login_b。B 无配置用户时保持空白。此测试不绕过添加菜单对非 active profile 的限制，使用当前允许的通用 profile 对象选择路径。
3. 增加 `new_database_owner_is_not_applied_to_other_database_kinds`：用当前支持的 MySQL/MariaDB/ClickHouse profile 创建 Database；断言 draft.database_kind 是实际目标 kind，owner 保持原空值。根据已有 capability factory 设置可达夹具，不更改 production capabilities。可以表驱动，避免三份长测试。
4. 增加 `new_database_owner_preserves_user_edits_until_reopened`：通过菜单打开后，用现有 owner focus 和文字编辑 actions 设置 `custom_owner`，切换字段后仍相同；清空后切换字段仍为空；设置合法 name 后 `draft.validate()` 给出既有 owner 必填错误。发送 Cancel，再从菜单重开，owner 恢复有效用户。
5. 对预览返回的覆盖复用本文件现有 plan request/result 夹具，如已有测试可断言同一草稿保持 owner 即追加到新增 case；不得为它调用真实数据库。异步角色结果不会填值可以通过源码复核确认，不新增无必要的复杂 mock。
6. 编辑现有 Database 的 owner 与 Schema 的有效用户默认值利用已有测试验证；若需要额外断言，仅修改本测试文件，不更改底层模型测试来迎合新默认值。
7. 运行 `cargo +1.94.0 test --test catalog_editor_reducer --test catalog_editor_state`，tool timeout=600000ms。预期全部相关回归通过；这一步已包含新增测试，无需立刻再重复运行定向测试。

**本任务复核：** case 与业务边界对应，不测试 helper 实现细节；初始化值真实进入 draft，手改值/空值不被下一次普通 action 覆盖；取消重开才重新初始化。

### Task 4：项目门禁、范围审查与交付证据

**业务文件：** 不新增；必要修复仍限定上述两文件。

1. 完成功能后顺序执行项目要求，设置工具有限 timeout：

| 实际执行命令 | timeout | 预期 |
| --- | --- | --- |
| `cargo +1.94.0 fmt --all -- --check` | 120000ms | exit 0，无格式 diff |
| `cargo +1.94.0 clippy --all-targets --all-features -- -D warnings` | 600000ms | exit 0，无 warning |
| `cargo +1.94.0 test --all-targets --all-features` | 600000ms | exit 0；记录真实执行/跳过的外部服务测试情况 |

2. 若 fmt 报错，执行 `cargo +1.94.0 fmt --all`（120000ms）后检查仅目标文件有变更，随后补跑 fmt check。其他失败按相关代码或环境修复；无新变更或未解决疑点不重跑已经通过的完整命令。需要全量验证后改代码时，只重跑受影响检查及必要最终门禁，不按工作流轮次机械重跑。
3. 超时后先用 `ps -axo pid,ppid,etime,command` 与 cargo 锁/进程检查定位，不直接重复同一挂起命令，不使用 timeout:0。环境检查不输出凭据。人工/PTY 环境限制最多一次有针对性的修复重试；可选手测失败记 required:false，不无限维持 progress。
4. `git diff --check`（120000ms）检查空白错误；`git diff --stat`、`git diff -- src/app.rs tests/catalog_editor_reducer.rs`（120000ms）复核范围与语义：没有额外文件、没有新 SQL/依赖、没有默认值覆盖用户输入、没有身份串用。自动实现后的审查由 Luna 完成，不切回分析模型。
5. 在本任务 `validation.md` 追加每条真实命令、退出状态、关键输出、HEAD 和工作区 diff 状态、toolchain/平台及 skipped 项，保留 analyze/plan 历史。命令未跑即写未跑，不把预期或旧证据当本版本通过。
6. 可选：有现成 PTY 与 PostgreSQL 时打开 New Database，检查 Owner 并编辑后取消。无条件则记 deferredChecks `{required:false}`，不实际建库，也不把截图作为阻塞条件。
7. 按自动工作流后续阶段交接实施结果、风险和验证限制；commit/合并由相应阶段依协议处理。本计划不授权在 plan 阶段提交或改变 index，用户无需反复 resume。

## 完成判定与限制处理

- 用户要求通过真实 App reducer 入口测试得到证明，默认值优先级及身份隔离符合本计划。
- 用户编辑保持、Schema 及原 Database 状态回归通过。
- fmt、Clippy 和全量测试记录真实结果；外部数据库未配置时不能宣称 live adapter 测试通过，但允许按项目现有环境门控跳过并记录。
- 新增业务改动仅在 `src/app.rs` 与 `tests/catalog_editor_reducer.rs`；任务报告在工作流专用目录。
- 普通代码/编译错误自行解决。仅确实缺少必要外部环境且无独立可推进工作时才按协议 blocked，并明确 required:true 的条件与恢复方式；本计划没有新增任何必需人工验收或数据库写入门禁。

## plan 阶段实际验证

本阶段已读取分析、项目本地检查约定、CI Rust 门禁与相关入口源码；HEAD 和工作区检查结果见同目录 `validation.md` 的 plan 记录。未运行 Cargo、未实施以上步骤，以上测试结果均为预期。计划已完成，可由自动工作流命名分支并交由 Luna 实施。
