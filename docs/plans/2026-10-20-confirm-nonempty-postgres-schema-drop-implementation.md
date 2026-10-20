# PostgreSQL 非空 Schema 二次确认删除 Implementation Plan

**执行者：** Luna；由已选择的自动工作流依次执行，不启动子 Agent，不再询问执行方式。writing-plans 技能的执行方式询问、额外技能、默认文档目录和逐步提交建议服从本任务明确协议；本阶段只生成计划，后续提交/合并由工作流对应阶段完成。

**Goal:** PostgreSQL schema 首次确认执行 RESTRICT，存在依赖时再次明确确认，随后通过 CASCADE 删除并正确更新界面。

**Architecture:** 保留默认受限删除，以 PostgreSQL SQLSTATE `2BP01` 作为二次确认的触发条件。adapter 负责构建受约束的 CASCADE 计划，runtime 保留结构化错误分类，App 复用现有确认框并隔离迟到结果。级联成功后清理目标子树，对同数据库其他目录和关系页按潜在依赖变化失效，不建立依赖图。

**Tech Stack:** Rust 1.94.0、Tokio、sqlx PostgreSQL、Ratatui、现有 Action/Command/reducer 架构。

---

## 0. 基线、授权和范围

- 分析输入：同目录 `analysis.md`；已完整读取。基线 HEAD：`773a2a381ce92cd8cd788804156a32abd7f01ecb`，目标 `main`。
- 本轮重新执行 `git status --short; git rev-parse HEAD`：status 为空、HEAD 与基线相同。不存在未提交依赖；新 worktree 可直接以该基线开始。
- checkpoint.json 当前仍不存在；不要自行创建它，不修改 state.json。历史分析回执保持原样。
- 实施开始时先检查实际 diff/HEAD，保护用户新改动。不要为清理整个工作区 commit/stash/reset；若出现本地新行为依赖，把确切文件加入 scope 并说明如何纳入任务 worktree。
- 建议任务名 `confirm-nonempty-postgres-schema-drop`；计划完成后由工作流命名任务分支并准备 worktree。
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
4. 专用 action 建议 `CatalogDropCascadeRequired { original_plan, cascade_plan }`。runtime 仅在真实 PostgreSQL + Schema + Restrict + `code.as_deref()==Some("2BP01")` 时生成它，不读英文 message 决定行为。
5. runtime 生成升级计划后，App 匹配 original_plan/连接/epoch/busy，分配新的 `next_profile_request_id()`，通过受限 rebind 方法更新升级计划 request_id 后显示二次弹窗。rebind 只改变 request_id 并再次 validate，不允许改对象、SQL、连接或 epoch。新 ID 避免与第一次执行任务的去重 key 冲突。
6. 重用 `CatalogDropConfirm`、`ToggleCatalogDropFocus`、`ActivateCatalogDrop`、Cancel、busy 和既有鼠标 hit target。二次确认永远重置 Cancel；初次 Drop 的焦点不能沿用。
7. 在 App 保存最小 pending drop plan request（connection/request_id/epoch 足以，或完整 request），初次/维护库重规划时登记，Ready/Failed 必须匹配并消费，连接失效后清除或拒绝。避免初次迟到 PlanReady 覆盖二次弹窗。执行 Failed 和 CascadeRequired 与成功一样隔离 expected plan/连接/epoch，不重构其他异步请求。
8. PG schema 实际执行通过 entry.database 构造 `ExecutionTarget`，复用 `resolve_catalog_mutation_connection` 的定向连接和 `close_if_owned`。连接身份必须仍在线；不要把新建目标池当作忽略断连/只读的许可。Restrict 和 Cascade 共用同一目标规则，不修改 console target/search_path。
9. 级联成功先按 plan 验证结果，再移除 schema 子树并处理 removed_set。清除同 profile/目标数据库的潜在过期目录、补全/搜索数据和 pending load，递增 epoch；保留数据库/其他 schema 的导航锚点，复用刷新请求加载 schema 列表和仍存在 schema 的组/已加载对象目标。不能只 `update(RefreshCatalog)`：它当前只刷新选中目标。
10. 对 schema 内关系页调用已有删除失效方法；其他同数据库关系页标记 native identity stale，后续访问重新解析。保留未保存编辑内容，禁止自动提交/回滚或把所有其他关系直接标记“已删除”。如已有 relation 方法不足，仅扩展该现有模型。

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
3. 加目标计算测试：profile/database/schema 对应选中 entry；数据库 B 下同名 schema 不可落到数据库 A 的任意活跃池；缺失目标、断连身份应失败。
4. 在普通执行分支保留 profile/readonly/validate，PG schema 单独走定向连接并确保 owned 池无论成功失败都 close。其他后端和维护库路径保持现状。
5. Restrict result 返回 2BP01 时通过 PG 专用入口生成升级计划，发送专用 action 后结束任务；其他错误发送现有 Failed。Cascade 不再升级。
6. Run `cargo +1.94.0 test --locked --lib catalog_drop`，timeout 600000ms，确保新分类/目标测试实际运行。

**复核:** 不丢弃 SQLSTATE 后再解析字符串；不会在等待用户时持锁/事务；没有第二次自动 execute；身份与目标数据库同时校验。

### Task 3：App 确认状态、过期结果与键鼠一致性

**Modify:** `src/app.rs`

**Test:** `tests/catalog_reducer.rs`、`tests/keymap.rs`

1. 新增 pending plan 状态，初始化 App 构造，初次请求和维护库重规划登记，Ready/PlanFailed 匹配后消费；更新测试让直接注入 Ready 的 fixture 先安装对应 pending 请求或经真实 Request 入口取得 request，避免绕开生产规则。
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

### Task 5：子树清理、跨 schema 失效和恢复

**Modify:** `src/app.rs`、`src/model/workspace.rs`、`src/model/explorer.rs`、`src/model/relation.rs`

**Test:** `tests/catalog_reducer.rs`

上述模型文件为预计需要的最小辅助方法位置；已有 API 足够时不强行修改该文件。新增方法须明确只作用于目标 profile/database。

1. 测试布置目标 schema 的多关系、其他 schema 的依赖 view 及另一 database 的同名对象、打开关系标签、补全索引、pending catalog 请求。
2. 模拟 Cascade Succeeded：目标子树删除且选回原数据库；removed_set 中关系页失效；同库其他关系页 stale、相关缓存和旧请求不再可用于编辑/补全；其他库/profile 保持不变。
3. 复用现有树删除和 relation invalidate 方法；提供数据库范围失效辅助方法以清理目录/cache/pending，推进 epoch，并基于保留的 navigation anchor 发必要刷新命令。避免保留旧对象直到用户偶然手动刷新；不要只失效 schema 列表而留下其子树旧对象。
4. 测试迟到的级联前 CatalogPageLoaded 不恢复旧对象；刷新后外部依赖 view 消失；未被实际删除的关系可重新解析并正常使用。
5. 测试失败/取消不触发成功清理；保留未提交关系编辑内容，不自动 save/commit/rollback。若存在 schema 内活动编辑，沿现有删除保护拒绝或提示，不能静默丢失。
6. Run `cargo +1.94.0 test --locked --test catalog_reducer catalog_drop`，timeout 600000ms。此时新增测试结果替代相关旧证据，不重跑无关套件。

**验收:** A7 完整；不建立完整数据库依赖图，保守失效仅限同数据库。

### Task 6：真实 PostgreSQL 补充回归用例

**Modify/Test:** `tests/postgres_catalog_drop.rs`

1. 新增 `#[ignore = "requires an explicitly isolated PostgreSQL test instance"]` 的 schema 集成测试，使用专用 `LAZYDB_TEST_POSTGRES_ADMIN_URL`；未配置时明确缺失条件，不能默默报告测试通过。
2. 使用随机唯一 schema（包含双引号的名称也要覆盖），创建 table/view/sequence/type；在第二个随机 schema 创建依赖 view。通过真实 adapter 计划执行，不仅 raw_sql 自测 PG 语法。
3. 验证：空 schema Restrict 成功；非空返回 SQLSTATE 2BP01、对象仍在；不执行升级代表取消时对象仍在；显式 Cascade plan 执行后目标及跨 schema view 消失，第二 schema 自身保留。
4. 需要另建 database 时使用 PgConnectOptions 切换 database，禁止将完整 URL 字符串直接附加 `/database`。每个 fixture 有明确清理过程，失败时尽可能清理；不打印 URL/凭据。
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

本阶段完成条件：计划包含逐项文件/步骤/复核/验证/验收，scope JSON 有效，原工作区/index 无业务变更，实际计划验证追加到 validation.md，最后写本轮指定的 plan completed 回执。后续由自动工作流进入 Luna 实施，不要求用户 resume 或选择执行方式。
