# PostgreSQL 角色与数据库规划分发修复 Implementation Plan

> 执行交接：由本自动工作流的 Luna 在实施阶段逐项执行；不启动子 Agent，不再次询问执行方式。本文件为计划，不代表修改或验证已经完成。

**Goal:** 修复普通角色及登录角色在 Review SQL 阶段被误判为不支持，并完整恢复同一分发遗漏影响的角色编辑和数据库新增、编辑。

**Architecture:** 把 Database、Role、LoginRole 的现有分发移到版本规划入口，静态方法只传默认版本，真实适配器仍传服务器实际版本。复用已有 SQL planner 和执行机制，在同文件测试模块用惰性连接覆盖公开 DatabaseConnection 调用路径。

**Tech Stack:** Rust 2024、Tokio、sqlx PostgreSQL，CI Rust 1.94.0；不新增依赖或公共 API。

---

## 1. 基线、范围与执行约束

- 分析依据：同目录 `analysis.md`；历史验证依据：`validation.md`。先前历史测试仅说明静态入口原有行为，不能计作修复验证。
- 原工作区 `/Users/yelog/workspace/tui/lazydb`；目标 `main`；起点 `7091bbc746d040f8441224d134179e224f82c868`。
- plan 阶段重新确认 HEAD 等于起点，`git status --short --branch` 输出 `## main...origin/main [ahead 1]`，无未提交修改。没有需要复制进新 worktree 的未提交依赖文件。
- checkpoint.json 读取结果为 File not found；模型不创建或修改该文件，不修改 state.json。
- 当前阶段只写本任务目录。实施阶段由自动工作流命名任务分支、准备从起点创建的 worktree，再执行代码变更；不在原 main 工作区修改业务代码，不 stash，不清空原工作区，不改动无关用户内容。
- **唯一预计业务改动文件：`src/db/postgres.rs`**，包含生产分发和已有 `#[cfg(test)] mod tests` 内的测试；无新增/删除/重命名业务文件。`change-scope.json` 列出该文件。
- 只读参考：`src/db/mod.rs`、`src/runtime.rs`、`src/app.rs`、`src/db/catalog_mutation.rs`、`src/model/catalog_editor.rs`、`tests/catalog_mutation.rs`、`tests/postgres_adapter.rs`、`tests/postgres_principal_mutations.rs`、`.github/workflows/ci.yml`、Cargo 配置。无需修改这些文件。
- 报告与回执保存在原任务目录，属于工作流元数据，不列入业务 change-scope，不提交这些 `.git` 元数据。
- 如果实施中发现必须超出文件范围，先将新文件准确加入 change-scope.json 并说明必要性；不得借修复开展其他数据库兼容性或权限功能重构。

## 2. 需求和验证级别

| 类别 | 具体内容 | 完成条件 |
| --- | --- | --- |
| 用户需求 | 修复截图中新增普通角色、登录角色的 UnsupportedOperation | 真实连接规划入口成功生成对应 LOGIN / NOLOGIN SQL |
| 同根因完整修复 | 角色编辑、数据库创建和编辑也走同一完整分发 | 六种有效请求均进入已有 planner，保持关键计划属性 |
| 必需定向验证 | 连接入口新增回归先红后绿、原 catalog_mutation 套件、真实版本传递保护 | 实际执行通过；修复前失败必须确认为目标 UnsupportedOperation |
| 项目 CI 门禁 | ci.yml 的 fmt、clippy、all-targets/all-features 测试 | 按下文运行、记录退出状态和适用环境，不能把跳过说成通过 |
| 项目 PostgreSQL CI 门禁 | postgres_adapter 与强制模式 postgres_principal_mutations | 有真实数据库和权限时运行；环境不足明确记录 CI 所需条件 |
| 补充建议，required:false | 人工/PTY 打开两类新增表单点击 Review SQL | 可用时确认进入预览，不要求 Apply；不可用不阻塞自动回归闭环 |

不把人工演示、新建数据库服务器、跨平台现场测试或手工创建真实角色引入额外强制门禁。已有 PostgreSQL CI 测试可能写入测试数据库，应只用配置的测试连接运行，绝不打印连接凭据。

## 3. 可验收单元：统一分发并覆盖真实入口

这是一个完整修复单元，按以下步骤推进，不拆成需要用户反复恢复的多个半成品任务。

### 步骤 A：确认实施位置和测试夹具

**文件：** 修改 `src/db/postgres.rs`；只读参考上述文件。

1. 在工作流准备好的任务 worktree 执行 `git rev-parse HEAD`、`git status --short --branch`、`git diff --stat`；核对实际基线和是否已有实现差异，避免覆盖其他内容。
2. 用 Codegraph 查看三个规划入口及其调用关系；必要时按 staleness 提示读取具体文件。
3. 复用 `src/db/postgres.rs` 测试模块约 8242 行的 PgPool 惰性构造方式。不要为测试添加 production constructor。
4. 复用 `tests/catalog_mutation.rs:166` 的 RoleDefinition 和 `245` 的 DatabaseDefinition 字段形状，夹具只放在当前文件测试模块，不跨测试 crate 抽取公用框架。

测试连接构造的完整建议实现（放在 tests 模块，可按本地风格调整名字）：

```rust
fn catalog_dispatch_connection(
    profile_id: Uuid,
    server_version_num: i32,
) -> crate::db::DatabaseConnection {
    crate::db::DatabaseConnection::Postgres(super::PostgresAdapter {
        pool: sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/test")
            .unwrap(),
        connection_id: profile_id,
        catalog_scope: crate::profile::CatalogScope::for_profile(
            crate::profile::DatabaseKind::Postgres,
            "test",
            None,
        ),
        server_version_num,
    })
}
```

调用此 helper 的测试用 `#[tokio::test]`，只执行同步规划，不调用 connect/execute/probe 等网络方法。

**复核：** 规划入口是 `connection.plan_catalog_mutation(...)`，不能又只测静态入口。惰性连接不会访问数据库；无真实密码和用户数据进入夹具。

### 步骤 B：添加有意义的失败回归测试

**文件：** `src/db/postgres.rs` 的 tests 模块。

按下面用例分成少量测试，测试名字均含 `catalog_mutation_dispatch`，用循环覆盖版本和角色种类即可，不复制六套近乎相同的构造逻辑。每个请求使用同一个固定 profile UUID、有效 request_id 和 catalog_epoch，并通过 `CatalogMutationRequest::new` 创建。

| 用例/建议测试名 | 输入及基线 | 必需断言 |
| --- | --- | --- |
| `catalog_mutation_dispatch_creates_roles` | 分别 RoleDraft::new(false/true)，object_type 为 Role/LoginRole，Profile anchor，name=test1，current_database=test | CREATE ROLE 与 NOLOGIN/LOGIN 正确；object_type 与请求一致；Transactional；maintenance 目标数据库为 test、schema=None；创建 fingerprint=None |
| 同一创建测试中的密码场景 | 登录角色设置固定测试密码 | 公开 plan.sql() 含 `PASSWORD '<REDACTED>'`；SQL 和 Debug 都不包含原密码；不得调用私密执行语句生成来打印密码 |
| `catalog_mutation_dispatch_edits_roles` | 两类角色；RoleDefinition 名字 alice，login 与类型对应，默认属性，membership=old_group，valid_until=infinity，comment=None，fingerprint=sha256:role；从定义建草稿，改名 bob、membership=new_group | ALTER ROLE 改名、GRANT new_group、REVOKE old_group；无空密码修改；fingerprint=sha256:role；Transactional、maintenance 目标和类型不变 |
| `catalog_mutation_dispatch_creates_database` | DatabaseDraft::new，name=app，owner=postgres，Profile anchor，current_database=test | CREATE DATABASE app；Autocommit；maintenance=test；刷新 Databases；fingerprint=None |
| `catalog_mutation_dispatch_edits_database` | DatabaseDefinition 与既有测试同字段，name=app，owner=postgres，fingerprint=sha256:app；Catalog(Database,[app]) anchor；草稿 connection_limit=12，current_database=test | ALTER DATABASE app CONNECTION LIMIT 12；Autocommit；maintenance=test；fingerprint=sha256:app；selection 为 app |
| `catalog_mutation_dispatch_preserves_validation` | 空角色名；以及当前数据库 app 改名 new_app 的请求 | 空名为 InvalidDraft；改名为 InvalidPlan 且原因表明当前数据库不可改名，不能只断言任意错误 |

角色编辑可沿用原 `__role__` Catalog anchor：`CatalogId::new(profile_id, CatalogKind::Database, ["__role__", "alice"])`，与已有测试一致，不额外引入 principal 夹具。

有效创建/编辑用例遍历 `server_version_num` 的 `120_000 / 150_000 / 180_000`。相同 request/draft/baseline 克隆后分别走：

1. `DatabaseConnection::plan_catalog_mutation`（测试主入口）；
2. `PostgresAdapter::plan_catalog_mutation_for_version(..., version)`；
3. `PostgresAdapter::plan_catalog_mutation(...)`（默认版本）。

对这些不依赖版本的对象比较三个入口的 SQL、类型、执行模式、目标、fingerprint、selection/refresh 等字段，或比较完整 plan（若其 Eq 正好表达相同语义）。同时保留表中具体 SQL 和属性断言，避免三个入口同时错误但彼此一致的假通过。

**执行并记录失败：**

```text
cargo +1.94.0 test --locked --lib catalog_mutation_dispatch -- --nocapture
```

工具 timeout=600000ms。预期 exit 非零，至少有效角色与数据库请求因 UnsupportedOperation 失败；出现字段不全、锚点无效、runtime 缺失等失败先修正夹具，不能当作 bug 已复现。

**验收：** 失败证据落入 validation.md，注明 worktree、起点、仅新增测试的 diff 状态和失败原因。

### 步骤 C：移动既有分发，保持版本传递

**文件：** `src/db/postgres.rs:1477` 与 `1872` 附近。

将静态入口改为完整的以下函数：

```rust
pub fn plan_catalog_mutation(
    request: CatalogMutationRequest,
    draft: crate::model::catalog_editor::CatalogDraft,
    baseline: Option<CatalogObjectDefinition>,
) -> Result<CatalogMutationPlan, CatalogMutationError> {
    Self::plan_catalog_mutation_for_version(request, draft, baseline, 120_000)
}
```

在现有 `plan_catalog_mutation_for_version` 函数体开头、Sequence 分发之前原样插入：

```rust
if request.object_type == CatalogObjectType::Catalog(CatalogKind::Database) {
    return Self::plan_database_mutation(request, draft, baseline);
}
if matches!(
    request.object_type,
    CatalogObjectType::LoginRole | CatalogObjectType::Role
) {
    return Self::plan_role_mutation(request, draft, baseline);
}
```

`plan_catalog_mutation_for_adapter` 保持原有实现：

```rust
Self::plan_catalog_mutation_for_version(request, draft, baseline, self.server_version_num)
```

**复核：** 不修改角色/数据库 SQL 生成器，不改执行模式、秘密处理、权限策略、能力声明及 public 签名；不出现两个入口重复持有上述分发；不把 adapter 改成调用默认 120000 静态方法。

**定向验证：**

```text
cargo +1.94.0 test --locked --lib catalog_mutation_dispatch -- --nocapture
cargo +1.94.0 test --locked --test catalog_mutation -- --nocapture
```

每条 timeout=600000ms，预期 exit 0。第二条应覆盖原有 role/database 以及 `postgres_view_options_are_version_gated_and_render_exact_syntax`；记录真实测试计数，不预设新增后总数。

### 步骤 D：保护适配器实际版本传递

**文件：** `src/db/postgres.rs` tests 模块，增加 `catalog_mutation_dispatch_preserves_server_version`。

沿用 `tests/catalog_mutation.rs:1258` 的 ViewDraft 字段形状构造一个 SELECT 1 视图草稿，schema=public，owner=owner，`security_invoker=ViewOption::available(Some(true))`，其他可选视图选项不指定。anchor=Schema([test,public])，object_type=View。

在真正的 `DatabaseConnection` 上分别设置 adapter 版本 140000 和 150000：

- 140000 返回版本选项不受支持的现有错误，核对现有 plan_view_mutation 的具体 variant/reason 后精确断言。
- 150000 成功，SQL 包含 `security_invoker=true`。

此测试直接防止误把实际连接入口接到默认 120000 方法。只需这一个跨版本行为，不新增视图功能或改写现有集成测试。

运行 `cargo +1.94.0 test --locked --lib catalog_mutation_dispatch -- --nocapture`，timeout=600000ms。此处因为新增测试而重跑有新依据；通过后不再无变化重复运行相同定向命令。

**单元验收：** 六种有效对象/操作从真实入口得到正确计划、两种错误输入保留业务约束、数据库无需访问、140000/150000 版本区别在实际适配器入口保留。

## 4. 完整功能验证与结果记录

### 项目强制检查

本机 plan 阶段 `rustup toolchain list` 已确认安装 1.94.0。实施阶段优先全部用该版本，避免 stable=1.94.1 与旧缓存产物混用。不依赖分析阶段 rustc 手工链接的临时复现程序。

功能齐备后按依赖顺序运行一次：

| 命令 | 工具有限 timeout | 预期与用途 |
| --- | --- | --- |
| `cargo +1.94.0 fmt --all -- --check` | 120000ms | exit 0，CI 格式门禁；失败仅格式化本次文件后复核 diff |
| `cargo +1.94.0 clippy --all-targets --all-features -- -D warnings` | 600000ms | exit 0，CI lint 门禁 |
| `cargo +1.94.0 test --all-targets --all-features` | 600000ms | exit 0，CI 全量测试；输出含环境跳过时逐项说明 |
| `cargo +1.94.0 test --locked --test postgres_adapter -- --nocapture --test-threads=1` | 600000ms | 现有 PostgreSQL 集成验证，含真实连接行为 |
| `env LAZYDB_REQUIRE_DATABASE_TESTS=1 cargo +1.94.0 test --locked --test postgres_principal_mutations -- --nocapture --test-threads=1` | 600000ms | ci.yml 的角色权限强制集成检查，不允许缺 URL 静默返回 |

如果全量命令已经以完全相同的数据库环境、线程设置和强制条件实际执行了某项集成验证，可引用该原始输出避免机械重复；否则按单独命令执行以满足 ci.yml 约定。不要用仅存在测试名称或 exit 0 的门控跳过冒充真实数据库通过。不要重复 check+clippy+全量测试，只在相关代码或环境变化、失败纠正后针对性重跑。

对任何 Rust 编译/检查超时，先只读检查 Cargo 锁、相关进程和最近 diff，确定原因后最多一次针对性的环境修复重试；不要 timeout:0，不要直接重复挂起命令。环境问题不影响其他独立测试的继续推进。

如确实只剩项目必需的外部数据库/权限且无独立工作，记录具体缺失条件和恢复方式，由后续收尾审查决定补证据或外部阻塞。普通编译、测试逻辑错误自行修复；不交还用户选择普通实现细节。

### 补充检查（非门禁）

`deferredChecks` 可记录 `{"name":"TUI/PTY Review SQL","required":false}`：在可用终端中使用现有测试连接打开普通/登录角色新增表单，输入 test1，点击 Review SQL，确认可进入预览。没有 PTY 或环境不支持时记录限制即可；不手工 Apply，不要求截图，不因此保持 progress 无限重试。若遇明确环境问题，最多一次针对性重试。

### validation.md 写入要求

在原任务目录的 validation.md 追加简短实施记录，保留历史原文，不重写历史成功时间或措辞制造新验证。每个新结果写明：完整命令、退出状态、相关源码版本/工作区 diff 状态、工具链、环境门控是否启用、是否实际访问测试数据库、通过/失败/跳过及原因。严禁输出连接密码或完整 URL。

## 5. 复核与交付标准

后续由 Luna 审查：

1. `git diff --check` exit 0；`git diff --name-only` 只有 `src/db/postgres.rs`；`git diff` 中无无关格式化、生成文件和依赖变动。
2. 静态入口和 adapter 均委托同一版本入口，默认/真实版本来源明确，无重复 profile 对象分发。
3. 回归测试真的经过 DatabaseConnection，断言业务 SQL 与计划属性，不仅是方法之间的相等；无网络依赖的测试不要求 CREATEROLE。
4. 密码脱敏、当前数据库不可改名、数据库 Autocommit 与角色 Transactional 保持原语义；角色/数据库原有 planner 不改动。
5. 必需检查均有可追溯结果，项目外部环境不足与可选 TUI 检查明确区分；不把历史静态入口测试当成当前修复的证据。
6. 计划产物和 change-scope 已齐备；原 main 工作区、Git index、用户未提交内容未被实现阶段误操作。

实施、审查完成后由自动工作流按既定授权处理任务提交/合并；推荐提交标题 `fix(postgres): unify catalog mutation planning dispatch`。本 plan 阶段不执行 commit/push/merge，不要求用户再选执行方式。

**最终验收标准：** 新增普通角色和登录角色不再报目标 UnsupportedOperation；同一根因影响的编辑和数据库操作可达；真实服务器版本传递有回归保护；修改保持单文件、无 API 或依赖扩展；验证与限制如实记录。
