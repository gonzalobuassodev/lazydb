# PostgreSQL 非空 Schema 二次确认删除 Implementation Plan

**执行者：** Luna；由已选择的自动工作流依次执行，不启动子 Agent，不再询问执行方式。writing-plans 技能的执行方式询问、额外技能、默认文档目录和逐步提交建议服从本任务明确协议；本阶段只生成计划，后续提交/合并由工作流对应阶段完成。

**Goal:** PostgreSQL schema 首次确认执行 RESTRICT，存在依赖时再次明确确认，随后通过 CASCADE 删除并正确更新界面。

**Architecture:** 保留默认受限删除，以 PostgreSQL SQLSTATE `2BP01` 作为二次确认的触发条件。adapter 负责构建受约束的 CASCADE 计划，runtime 保留结构化错误分类，App 复用现有确认框并隔离迟到结果。级联成功后清理目标子树，对同数据库其他目录和关系页按潜在依赖变化失效，不建立依赖图。

**Tech Stack:** Rust 1.94.0、Tokio、sqlx PostgreSQL、Ratatui、现有 Action/Command/reducer 架构。

---

## 0. 基线、授权和范围

- 分析基线 HEAD 为 `773a2a381ce92cd8cd788804156a32abd7f01ecb`。实施 worktree：`/Users/yelog/workspace/tui/lazydb-worktree/confirm-nonempty-postgres-schema-drop`，分支 `task/confirm-nonempty-postgres-schema-drop`。
- 首次检查时业务工作区无改动，只有自动工作流生成的未跟踪计划文档；没有未提交依赖复制进 worktree。checkpoint.json 不存在；没有创建它，也没有修改 state.json。
- 本计划已执行到两个单元，worktree 包含计划文档和未提交实现。继续前检查真实 diff，保护已有改动；不清理、stash、reset 或 commit 整个工作区。
- 本计划不新增依赖，不修改数据库连接配置或 CI，不扩展其他数据库的 schema 删除，不开放表、列等对象的 CASCADE。
- scope 中列出的文件是预计修改文件；任务目录的报告和回执为工作流元数据，不作为业务变更提交。

## 1. 需求验收与验证分级

### 必须满足的行为

| 编号 | 验收标准 | 主要证据 |
| --- | --- | --- |
| A1 | 空 PG schema 经当前一次 Drop 确认即可删除 | adapter 默认计划测试、reducer 成功测试 |
| A2 | 非空 schema 首次 RESTRICT 不删除对象，2BP01 后进入第二确认，不报通用删除失败、不自动重试 | runtime 分类测试、reducer 状态转换测试 |
| A3 | 第二确认默认 Cancel，准确展示 CASCADE SQL、全部内部对象/数据和跨 schema 依赖风险、不可撤销 | reducer、TestBackend 渲染测试 |
| A4 | 二次取消不执行 SQL；明确 Drop 才执行一次 CASCADE；键盘鼠标同一规则 | reducer Confirm/Activate、keymap、UI hit 测试 |
| A5 | 权限、对象不存在、网络等错误不触发升级；CASCADE 失败不循环升级 | runtime 分类表、失败 reducer 测试 |
| A6 | 引用名称、系统 schema、只读、连接/epoch、重复提交、迟到结果保护完整；选中数据库与执行数据库一致 | planner、runtime helper、reducer 测试 |
| A7 | 成功移除 schema 子树与补全，相关关系页失效；跨 schema 可能受影响目录重新验证；其他数据库不受影响 | reducer 状态和刷新命令断言 |
| A8 | 原有其他对象删除策略不变，通用 CASCADE/IF EXISTS/多语句拒绝保留 | 现有 catalog_drop 回归与全量测试 |

### 项目现有强制检查

来自 `.github/workflows/ci.yml:55–96`：Rust 1.94.0，`fmt --all -- --check`、`clippy --all-targets --all-features -- -D warnings`、`test --all-targets --all-features`。Linux CI 还单独构建测试产物；macOS CI 检查二进制依赖。现有数据库 CI 的 PostgreSQL adapter 测试属于已有项目门禁，其 URL/fixture 由 CI 提供。

本次实施必须补充并运行无需外部数据库的定向测试，再在功能齐备后运行相关 Rust 全量检查。不要把“测试过滤后成功但 0 tests”算通过：核对实际匹配的测试名称和数量。

### 补充验证（required:false）

- 真实隔离 PostgreSQL schema/cross-schema 测试：编写 ignored 集成测试，有明确专用实例时运行；本次不修改 CI 将其强制化。无实例就记录未运行，不阻塞可独立完成的实现。
- PTY 人工交互、窄终端视觉观察：补充验证；自动 TestBackend 测试仍须覆盖可读文案与命中规则。没有 PTY 不构成门禁失败。
- 本地无法运行的 Linux 原生构建和 macOS 完整分发依赖扫描：沿用现有 CI，不因本次业务逻辑修改另外建立发布环境。
- 不把分析报告中的建议真实数据库/人工验收升级为用户强制前置条件。

## 2. 最终设计决策

1. 以 DB 的 RESTRICT 结果判断依赖，不读取 Explorer 子节点数量，不另写“schema 非空”查询。
2. 计划增加私有行为标记（例如 `CatalogDropBehavior::{Restrict, Cascade}`）与只读 getter；`new` 默认 Restrict。无须给 Overlay 增加确认阶段字段，UI 从已验证的 plan 行为渲染。
3. CASCADE 只能通过 crate 内 PG schema 专用入口构建。该入口接收原 entry/request，经 `postgres_schema_name` 核验 identity、parent、系统 schema 和引用名，生成准确单条 SQL。`validate()` 对受约束的 schema 例外重新核对行为、entry、object、CurrentConnection 和完整期望 SQL；通用 new 继续拒绝 CASCADE。共享校验所需逻辑可留在现有 catalog_drop/postgres 模块，不新增通用 DDL 框架。
4. 专用 action `CatalogDropCascadeRequired { original_plan, cascade_plan }`。runtime 仅在真实 PostgreSQL + Schema + Restrict + `code.as_deref()==Some("2BP01")` 时生成它，不读英文 message 决定行为。
5. runtime 生成升级计划后，App 匹配 original_plan/连接/epoch/busy，分配新的 `next_profile_request_id()`，通过受限 rebind 方法更新升级计划 request_id 后显示二次弹窗。rebind 只改变 request_id 并再次 validate，不允许改对象、SQL、连接或 epoch。新 ID 避免与第一次执行任务的去重 key 冲突。
6. 重用 `CatalogDropConfirm`、`ToggleCatalogDropFocus`、`ActivateCatalogDrop`、Cancel、busy 和既有鼠标 hit target。二次确认永远重置 Cancel；初次 Drop 的焦点不能沿用。
7. CascadeRequired 与执行 Failed 匹配 busy overlay 中的 expected plan、连接和 epoch；级联计划确认时分配新 request id。初次 PlanReady 保留现有 reducer 协议，以兼容现有 action 测试夹具，不增加公共 pending-request 状态。
8. PG schema 实际执行通过 entry.database 构造 `ExecutionTarget`，仅以完整连接 identity + target 精确复用 session，否则用现有 credential 路径定向连接并在返回/显示确认前关闭 owned pool。连接身份必须仍在线；新建目标池不绕过断连/只读检查。Restrict 和 Cascade 共用同一目标规则，不修改 console target/search_path。
9. 级联成功先按 plan 验证结果，再移除 schema 子树并处理 removed_set。清除同 profile/目标数据库的潜在过期目录、补全/搜索数据和 pending load，递增 epoch；保留数据库/其他 schema 的导航锚点，复用刷新请求加载 schema 列表和仍存在 schema 的组/已加载对象目标。不能只 `update(RefreshCatalog)`：它当前只刷新选中目标。
10. 对 schema 内关系页调用已有删除失效方法；不在服务器确认目录变化前宣称跨 schema 依赖已删除。保留未保存编辑内容，禁止自动提交/回滚或丢弃用户编辑。

推荐英文正文：

```text
This schema contains other objects.
Continuing will delete all objects and data in this schema.
Dependent objects in other schemas may also be deleted.
This action cannot be undone.
```

标题 `DROP SCHEMA CASCADE`，同时展示 plan.sql()。不显示猜测的对象数，不承诺只影响内部对象；错误/detail 如展示，继续使用 sanitize_terminal_text。

## 3. 可验收单元一：从首次 Drop 到二次确认并完成删除

本单元结束必须完成实际业务闭环，不停留在只增加模型/弹窗的半成品。以下步骤按依赖顺序实施，每一步保持具体、可检查。

### Task 1：受约束的 PG schema 级联计划

**Modify:** `src/db/catalog_drop.rs`、`src/db/postgres.rs`

**Test:** `tests/catalog_drop.rs`

1. 在现有 PG schema fixture 基础上写测试：默认仍为 `DROP SCHEMA "odd""schema" RESTRICT`；升级后准确 `DROP SCHEMA "odd""schema" CASCADE`；request object/identity/epoch 不变；新 request_id rebind 不改变目标。
2. 添加拒绝用例：普通 new 的 schema/table CASCADE、非 schema 升级、系统 schema、entry/object 不一致、MaintenanceDatabase、篡改公开 object/kind/entry、IF EXISTS、多语句；引用标识符 `cascade`/双引号/分号内容正确处理。
3. Run `cargo +1.94.0 test --locked --test catalog_drop`，timeout 600000ms；新断言在实现前应失败（编译缺少新 API 也可），记录实际失败，勿多次无意义重复编译。
4. 实施私有行为、专用构造/升级入口、严格 validate 和 request_id rebind。普通 new 的安全策略保持原样，避免先 globally allow 再依赖 UI 限制。
5. 重跑同一测试至通过。复核 SQL 来自 PG adapter，App/runtime 没有字符串替换。

**验收:** A1/A6/A8 的计划模型部分通过；不引入 public 可随意切换 CASCADE 的布尔开关。

### Task 2：运行时依赖结果分类及数据库路由

**Modify:** `src/action.rs`、`src/runtime.rs`

**Test:** `src/runtime.rs` 中现有单元测试区域（新增名称含 `catalog_drop`），必要的规划模型测试仍放 `tests/catalog_drop.rs`。

1. 增加专用 CascadeRequired action，携带原计划和可信级联计划。
2. 将分类条件提成可无数据库测试的小函数，覆盖 PG schema restrict/2BP01 为 true；table、DuckDB/MySQL、cascade 模式、42501、3F000、无 code、仅 message 含依赖词均为 false。
3. 目标由 entry.database + schema 构造 `ExecutionTarget` 并验证 catalog scope；只按完整连接 key 复用 session，否则用 profile credentials 定向连接并关闭 owned pool。缺失目标/凭据必须失败。
4. 在普通执行分支保留 profile/readonly/validate，PG schema 单独走定向连接。其他后端和维护库路径保持现状。
5. Restrict result 返回 2BP01 时通过 PG 专用入口生成升级计划，发送专用 action 后结束任务；其他错误发送现有 Failed。Cascade 不再升级。
6. Run `cargo +1.94.0 test --locked --lib catalog_drop`，timeout 600000ms，确保新分类/目标测试实际运行。

**复核:** 不丢弃 SQLSTATE 后再解析字符串；不会在等待用户时持锁/事务；没有第二次自动 execute；身份与目标数据库同时校验。

### Task 3：App 确认状态、过期结果与键鼠一致性

**Modify:** `src/app.rs`

**Test:** `tests/catalog_reducer.rs`、`tests/keymap.rs`

1. 不增加初次计划 pending state；CascadeRequired 和 Failed 只在 expected overlay 正在 busy，且 plan/connection/epoch 匹配时生效。
2. 新测试驱动闭环：Request→Ready→选择 Drop→Execute Restrict→CascadeRequired→默认 Cancel→选择 Drop→Execute Cascade→Succeeded。分别断言两个 Execute 的 SQL/行为/request_id 和中间目录不变。
3. 补二次 Enter 默认取消、Esc 取消、鼠标 Activate、一旦 busy 重复 Enter/Activate 无命令；二次取消时没有 Cascade Execute。
4. 补迟到 original Failed/Ready、不同 request_id/连接/epoch 的 CascadeRequired 不覆盖新 overlay；断连或 epoch 改变后确认不执行；CASCADE Failed 保留正确计划、复位 Cancel 且不发升级命令。
5. 实施新 action 接收、分配新 ID、验证升级计划 lineage、重置焦点和 busy。把失败结果的隔离补齐；初次 Ready 同样验证 pending/identity/epoch。
6. Run `cargo +1.94.0 test --locked --test catalog_reducer catalog_drop`；Run `cargo +1.94.0 test --locked --test keymap catalog_drop`，各 timeout 600000ms。新测试统一使用 `catalog_drop` 前缀，避免过滤遗漏。

**验收:** A2/A4/A5/A6；仍复用原 overlay 结构，预期不需修改 input/keymap 或 input/mouse 业务代码。keymap 测试验证二次 plan 下映射仍相同。

### Task 4：二次确认风险提示和准确 SQL

**Modify:** `src/ui/mod.rs`

**Test:** `tests/ui_render.rs`

1. 通过可信 Cascade plan 创建 overlay，TestBackend 验证标题、实际引用后的 CASCADE SQL、数据/跨 schema 风险、默认 Cancel、按钮 hit target。
2. 为 80×24 和较窄尺寸（例如 60×24）做渲染测试，验证风险正文未因固定高度截断；busy 时按钮不可重复提交的现有测试保留。
3. 在现有 renderer 内按 plan 行为选择标题、正文、合适高度和换行，保持现有主题、sql_preview 和按钮组件。
4. Run `cargo +1.94.0 test --locked --test ui_render catalog_drop`，timeout 600000ms。

**验收:** A3；文案和 SQL 不互相矛盾，不在通用 error 中伪装二次确认，不新增独立 UI 框架。

## 4. 可验收单元二：级联成功后的状态闭环

### Task 5：子树清理和 schema 列表刷新

**Modify:** `src/app.rs`

**Test:** `tests/catalog_reducer.rs`

1. Cascade Succeeded 使用 removed_set 移除目标子树补全并失效子树内已打开的 relation tabs。
2. 根据 plan entry 的 database 找回其 CatalogId，调用已有 `commands_for_catalog_targets` 刷新 schema 列表；对同 database 所有其余已知 schema 的 Groups 与已知 Objects targets 标 stale 并重新加载。不要只调用 RefreshCatalog（只刷新当前选择）。
3. 同 database 非目标 relation tabs 标 native identity stale，保留快照和用户未保存编辑；不声称 view 已删除，保留对象行直到 catalog refresh 实际确认。其它 profile/database 不受影响。
4. 测试目标 schema 子树移除、relation tab stale、schema/group/object refresh command；失败/取消不触发清理。未确认的跨 schema 对象不标记为已删除，不清除用户编辑。
5. Run `cargo +1.94.0 test --locked --test catalog_reducer catalog_drop`，timeout 600000ms。

**验收:** A7 的目标子树、本地补全、同 database 目录和相关 relation 页均失效/重载；其它 database 不触碰。对服务器未回报的依赖对象不推断已删除，直到数据库刷新确认。

### Task 6：真实 PostgreSQL 补充回归用例

**Modify/Test:** `tests/postgres_catalog_drop.rs`

1. 新增 `#[ignore = "requires an explicitly isolated PostgreSQL admin URL"]` schema 集成测试，使用专用 `LAZYDB_TEST_POSTGRES_ADMIN_URL`。
2. 使用随机唯一且含双引号的 schema，创建 table/view，经实际 `DatabaseConnection` 和 adapter plans 执行，而非只测 raw SQL。
3. 验证非空 RESTRICT 返回 SQLSTATE 2BP01 且 schema 仍在；执行 adapter Cascade plan 后 schema 消失。测试显式 ignored，不是项目门禁。
4. 使用 URL 中现有 database，不拼接或重写 URL；以明确清理删除 fixture，不打印 URL/凭据。
5. Run `cargo +1.94.0 test --locked --test postgres_catalog_drop`（600000ms），确认普通模型测试通过、新集成标记 ignored。
6. 若明确存在隔离实例，Run `cargo +1.94.0 test --locked --test postgres_catalog_drop -- --ignored --test-threads=1`（600000ms）。否则记录 deferredChecks：`required:false`、条件为专用隔离 PostgreSQL URL，不读取用户已有生产连接充当 fixture。

**验收:** 测试代码可编译，运行状态如实记录；有环境时补充 DB 证据，无环境不阻塞实现与自动测试。

## 5. 收尾复核与必要验证

由 Luna 在上述两个单元完成后执行，不切回分析模型，不启动子 Agent。

1. 检查实际 diff 对照 change-scope：没有意外后端、依赖、配置、历史文档或 lockfile 改动；普通 Drop SQL 行为没有改变。
2. 复核每个 A1–A8 均有实际匹配的测试或静态证据；检查“首次仍 RESTRICT”和“二次默认取消”没有被错误简化。
3. 执行项目 Rust 门禁（各自独立记录；fmt 120000ms，其余最多 600000ms）：

```sh
cargo +1.94.0 fmt --all -- --check
cargo +1.94.0 clippy --locked --all-targets --all-features -- -D warnings
cargo +1.94.0 test --locked --all-targets --all-features
git diff --check
```

4. 无需重复运行 `cargo check` 或本地单独 `test --no-run`：上面编译已涵盖。Linux CI 的单独 no-run 步骤仍由现有 CI 负责。
5. 记录实际 Rust/工作区版本、命令、退出码、匹配测试数量/ignored 数量、环境与限制到任务目录 validation.md。保留既有 analyze/plan 记录，不将其变成新测试结果。
6. 发生超时，先检查 Cargo 锁、进程、最近代码和依赖编译状态，不能立即重复同一挂起命令；只允许有限 timeout，不设置 timeout:0。Rust 每次最多 10 分钟，其他每次最多 2 分钟。
7. 环境问题的人工/PTY 检查最多一次针对性修复重试。可选检查不足写 deferredChecks(required:false)，不能无限 progress。若只剩项目强制门禁的必要外部条件且已无独立工作，才按协议 blocked 并写 required:true 的 external blocker。
8. 正常代码/编译/测试失败自行修复；仅重跑受修复影响的检查。功能齐备且全量通过后不反复全量验证。

## 6. 变更范围与阶段完成条件

预计修改业务/测试文件已同步 `change-scope.json`：

- `src/db/catalog_drop.rs`
- `src/db/postgres.rs`
- `src/action.rs`
- `src/runtime.rs`
- `src/app.rs`
- `src/ui/mod.rs`
- `src/model/workspace.rs`
- `src/model/explorer.rs`
- `src/model/relation.rs`
- `tests/catalog_drop.rs`
- `tests/catalog_reducer.rs`
- `tests/keymap.rs`
- `tests/ui_render.rs`
- `tests/postgres_catalog_drop.rs`

不预计新增、删除或重命名业务文件。仅读取的 `src/db/mod.rs`、`src/db/catalog_mutation.rs`、`src/input/*`、CI 和历史设计文档不列入 scope。若实施确需额外文件，先按真实变更更新 scope 和理由，避免预列整个 src 目录。

### 实施进度（2026-10-20）

- Task 1–6 已实施，包括计划模型、PG SQLSTATE 分流和定向路由、二次确认 reducer、风险提示、同 database 跨 schema catalog invalidation/refresh 和 ignored PostgreSQL 集成测试。
- 实际验证结果在任务目录 validation.md 和 implementation.md；最终源的 catalog/reducer/UI/PG test target、Clippy、fmt、全量 all-targets/all-features 测试均已运行并记录。
- 手工终端交互和真实隔离 PostgreSQL fixture 仍为 `required:false` 补充检查；后者当前报告为 ignored，未提供该外部 URL。
- 实施与收尾复核完成；worktree 尚未提交或合并，后续由自动工作流按用户授权处理。
