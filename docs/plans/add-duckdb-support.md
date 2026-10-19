# DuckDB 完整支持 Implementation Plan

> **执行交接：Luna。** 使用 writing-plans 形成的逐项计划；用户已选择自动工作流，不再询问执行方式，不启动子 Agent。GPT-6.1 Sol 仅负责 analyze/plan，实际实现、审查、纠偏、提交和合并由 Luna 执行。技能模板中的 Claude、executing-plans、频繁提交和执行方式选择不覆盖本任务的明确约束。

**Goal:** 为 LazyDB 增加原生 DuckDB 支持，使现有客户端适用于 DuckDB 的连接、SQL、目录/DDL、数据和对象变更、事务/取消及 CLI/MCP/LSP 功能形成可验证闭环。

**Architecture:** 使用官方 duckdb-rs bundled 引擎，实现独立适配器。复用现有数据库合同，显式区分数据库实例、console 会话和事务租约；同步调用在阻塞任务运行，取消通过 operation 专属 interrupt handle 完成。DuckDB 使用原生方言、元数据和绑定值，不复用 SQLite 文件驱动或重建算法。

**Tech Stack:** Rust 1.94、Tokio、duckdb-rs、sqlparser 0.62 DuckDbDialect、现有 ratatui/CLI/MCP/LSP。

---

## 一、基线和本阶段交付

输入为同目录 `analysis.md`（已完整阅读）。目标 `main`，起点 `0e68ae20e9a455d7c0f9b46bcecff2e7d0283059`。plan 阶段再次执行 Git 只读检查，HEAD 等于起点、工作区和 index 干净；`checkpoint.json` 仍不存在。不存在未提交依赖，不需要复制、清理或提交原工作区内容。

本阶段只生成本任务目录中的 plan.md、change-scope.json、追加 validation.md 和当前 token 的 plan 回执；不创建 docs/plans 副本，不实施业务代码，不改 checkpoint.json/state.json、不创建分支/worktree、不改 index、不 stash/commit/push。下一阶段由工作流自动命名分支；建议语义名称为 DuckDB 完整支持，但不替 Luna 决定最终任务名称。

实施前 Luna 读取当时实际 checkpoint 和 diff，发现已有改动则从真实代码提炼剩余项，不因本计划的起始状态覆盖用户内容。新 worktree 从指定起点创建；若实施时发现新增的本地依赖，应先将精确路径和所需行为补入 scope 与计划，再处理迁移，不能假定未提交内容自动出现。

## 二、需求范围、计划决策和验证等级

### 2.1 用户需求与完成定义（U）

用户要求当前数据库客户端完整支持 DuckDB。完成包含：文件/内存配置、可靠执行和类型显示、Explorer/搜索/metadata/Data/DDL、原生适用的网格及目录编辑、事务/取消/退出、CLI/MCP/LSP、持久化与既有驱动兼容。U 表示用户目标，不表示用户指定了某一条测试命令。

不适用项：DuckDB 无原生 users/roles、trigger、stored procedure、materialized view 时明确标注不支持，不能造空数据冒充支持；server dashboard 不适用。不扩建 MotherDuck/DuckLake 专属管理、驱动桥接或扩展商城。JSON/Parquet/CSV 原生读写 SQL 属于 DuckDB 使用路径，使用临时文件验证；扩展由用户自行配置，不自动安装外部扩展。

### 2.2 项目现存强制门禁（P）

来源 `.github/workflows/ci.yml:81-88`，实施完成后的有效代码版本应运行：

```sh
cargo +1.94.0 fmt --all -- --check
cargo +1.94.0 clippy --all-targets --all-features -- -D warnings
cargo +1.94.0 test --all-targets --all-features
cargo +1.94.0 build --locked
sh scripts/release/check-macos-dependencies.sh target/debug/lazydb
```

最后两项为现有 macOS CI 约定，在 macOS 实施环境适用。若精确工具链未安装，可先用当前 1.94.1 做定向检查，但必须记录与 CI 1.94.0 不同，不能以其冒充 P 已通过。项目 existing database jobs 仍由原 CI 运行；远端数据库缺失或原有测试跳过需如实记录，不为 DuckDB 新增外部数据库依赖。

`.github/workflows/release.yml:77-118` 的五平台 release build、macOS 依赖和 artifact smoke 是**已有发布流程门禁**，不是当前 plan 阶段运行要求。此次不授权发布、打 tag 或为取得验证而触发发布 workflow。

### 2.3 本计划的自动化验收（A）

各单元的定向测试是为 U 设计的自动化实现验收，不谎称用户亲自强制这些命令。每个 advertised DuckDB 能力有真实 native 测试；DuckDB 测试使用 tempfile/in-memory，不以缺少服务 URL 或 CLI 静默 return。失败时修实现或有依据地调整错误测试，不能删用例、放宽断言或把核心能力标记 Unsupported 来规避需求。

定向测试名称在下文列为最终目标。第一次补测试可以因类型/方法尚不存在而编译失败；记录失败原因，随后实现并转绿。不可把依赖下载失败当成行为测试已触发预期失败。每个小项优先“测试→运行→最小实现→同一测试转绿→复核”，每个端到端单元完成后连续推进下一个。

### 2.4 补充建议验证（S）与环境限制

人工/PTY TUI smoke、真实大库性能、包体积观察和本机以外平台构建属于补充证据，不自动升级为本地必需门禁。可以用自动化 reducer/render/runtime 测试覆盖主要交互。PTY 受限最多一次针对性修复重试，随后记录限制交 Luna 收尾审查；不能无限 progress。跨平台结果由能获得的常规 CI/build 证据补充，不伪称本机已验证 Windows/Linux。普通编译错误不属于环境阻塞，应持续修复。

validation.md 每次追加代码 SHA + diff 状态、命令、退出码、环境、涉及文件和结论；旧 analyze 记录保留。没有代码或环境变化不重跑同一检查，单元内定向测试，功能完整后一次 P 全量；全量后新修改只重跑受影响项，必要时补最终门禁，不机械循环。

## 三、固定实施约定及分析结论的细化

### 3.1 依赖和模块

以分析核对的已发布 `duckdb = "=1.10506.0"` 为起点，`default-features = false`，features 为 bundled/chrono/json/parquet。先在实施阶段实际 resolve 和编译，锁定 Cargo.lock。若可用镜像/版本不同，核对对应 tag 的 prepare/interrupt/value 行为后选稳定版本并记录，无需把普通依赖选择退回用户。不能按上游 main 的不同版本 API 编码，不新增自写全量 C API 封装。

采用固定模块布局，避免单文件适配器膨胀：

```text
src/db/duckdb/mod.rs          adapter、probe/close、capabilities 和 facade
src/db/duckdb/session.rs      实例所有权、会话租约、operation guard
src/db/duckdb/query.rs        原文 batch、结果 metadata、预算、错误
src/db/duckdb/value.rs        logical type 解码、规范文本、typed bind
src/db/duckdb/catalog.rs      发现/分页/搜索/preview/DDL
src/db/duckdb/transaction.rs  TransactionBackend、确认状态和清理
src/db/duckdb/mutation.rs     行 mutation、目录 mutation/drop
src/runtime/duckdb.rs        profile generation 实例与 console 会话路由
```

项目以 `src/db/mod.rs` 声明 `pub mod duckdb`；不再同时创建冲突的 src/db/duckdb.rs。只在确需驱动专有 SQL 辅助时新增 `src/sql/duckdb.rs` 并在 mod.rs 声明。

### 3.2 Profile 与 target：文件路径和 catalog 分开

新增 `ConnectionProfile.duckdb_path: Option<PathBuf>`，`#[serde(default, skip_serializing_if = "Option::is_none")]`；ParsedConnectionUrl 对应 path/memory 属性也独立命名。旧 profile 转换赋 None。新增字段会影响现有 struct literals，scope 列入全部已定位构造点；只补字段，不借机重写旧驱动。

规范 URL 为 `duckdb:/absolute/path.duckdb`、`duckdb:relative/path.duckdb`、`duckdb::memory:`；接受三斜线绝对路径形式，拒绝非空 host/远程伪 URL。参数统一为 `read_only=true|false`、`schema=<name>`，重复冲突/未知项拒绝；内存加 read_only 拒绝并解释。使用既有 URL encode/decode 库，不能手动去掉 Windows 盘符。导出统一规范形式，测试中文/空格/%/#/?/Windows drive 的无损往返。

`database` 是 native catalog，不是文件路径。实现一个共享的“默认 DuckDB catalog”函数供 URL/draft/ExecutionTarget 使用，文件名规则用实际引擎测试固定；`:memory:` 对外配置保留模式，执行 target 为 memory/main。连接 probe 的 `ServerInfo.database` 是权威值：若与预期不同，先经 Action/runtime/app 把 resolved target 和默认 scope 同步，再派发排队查询，不允许查询以旧 target key 启动。Selected scope 中用户显式选择不因归一化被扩展；无法安全映射时明确配置错误。

Test Connection 对不存在文件不得创建；在 profile draft/CLI 的测试入口预检，正式连接可创建。只读缺失文件无副作用。项目相对路径相对 project root，global/TUI 使用保存时的绝对路径；`sqlite_path` 语义保持。

### 3.3 实例、会话与取消

- 实例 key 是 profile UUID + connection generation；owner 保持引擎根连接，派生连接用 try_clone。不同 schema/catalog target 共享实例，不能 open 新内存库。相同文件冲突返回可理解配置/锁错误，不冒充网络故障。
- 会话 key 是实例 + console/tab UUID。普通查询、衍生分页、WHERE/ORDER BY、count、MANUAL 执行必须使用同一会话；进入 MANUAL 租用该会话而非另开连接。每个会话 operation 串行；其它 console 独立。relation tab 也有稳定会话以保证其事务和预览一致。
- root 负责目录系统查询；临时对象的 metadata/preview 应从拥有它的会话读取，不能声称所有 clone 都看得到。旧 catalog 路由无 console id 的接点需补最小 session context（action/runtime）并测试，不修改全部旧驱动签名。
- operation guard 带 query generation + interrupt handle；future drop 可中断 native 调用，但归还租约只能发生在阻塞任务真正退出之后。待执行 operation 在开始前检查取消标记。迟到 guard drop 不可打断下一代查询。保持现有普通取消历史 Unknown，除非有新确认结果；MANUAL 只有执行结束且 rollback 成功才能报告已回滚。
- 断开、关闭 tab、profile 修改、退出路径释放对应会话；根 owner 不抢先销毁正在使用的句柄；rollback/close 失败走 quarantine。避免在 holding runtime map mutex 时 await native 操作。

### 3.4 执行、类型和风险

逐 `scan_statements` 的原始 text range prepare 一条，不能 prepare 整个 buffer，因为上游会执行前置语句并只返回最后结果。Dollar quote、嵌套注释、字符串分号和列表语法先修扫描器；AST 仅用于分析，不 reserialize 后执行。零行列 metadata 通过新增 accumulator `start_result(columns)` 初始化；必须在 DuckDB statement 实际执行后读取列元信息，严禁“probe 查询”将 DML 执行第二次。

CellValue 不先扩建一套类型系统。现有可表达标量用已有变体；decimal、128 位、interval 和嵌套类型保留 ColumnMeta.native name 与规范文本，typed mutation 按 metadata 转为原生值。不能经 f64/i64 中转，不能 Debug 格式冒充可靠值。逻辑类型树解码应处理嵌套 UHUGEINT；库接口无法安全读取时优先在结果读取/原生转换层使用已验证的无损路径，不能重执行用户语句或拼 SQL 将用户结果再次求值。仅个别 native 类型确实不可读取时 Unsupported，并保留原因和类型。

SQL 风险分类对 DuckDB 增补原生语法：COPY TO/ATTACH/DETACH/INSTALL/LOAD/CREATE SECRET/SET/USE 保守处理；SELECT 的 nextval、setval 等已知副作用、`EXPLAIN ANALYZE` 内部写语句不得绕过 MCP deny。未知函数/扩展的分类不能笼统承诺安全，文档说明现有客户端政策不是文件系统 sandbox。TUI 仍可确认后原文执行引擎支持、parser 不认识的语法；不得为了通过安全测试彻底禁用 DuckDB 查询。

### 3.5 目录编辑边界（对分析的具体化）

复核 `CatalogDraft`/`CatalogObjectDefinition`：已有 schema/table/index/constraint/view/sequence，**没有 Type 或 Macro 表单/definition**。本计划不创建新的全局 Type/Macro 编辑器。Type/Macro 原生 SQL 新建/replace/drop + Explorer 浏览/搜索/DDL/补全刷新是本次范围；菜单说明“通过 SQL 管理”，不能显示为已实现结构化表单。它们的专门表单属于后续扩展，不阻碍当前客户端同等能力支持。

结构化创建至少 schema/table/view/sequence/index；创建表里的列/default/NULL/PK/unique/check/FK 由现有 TableDraft 承载。编辑实现 DuckDB 支持且现有 draft 能表达的 table rename、column rename/add/drop/type/default/nullable/comment，view query replace/comment；schema/index/sequence 实际不支持 ALTER 的字段 disabled 并附原因。约束或依赖阻止 ALTER 时回传 native 错误、保留 baseline，不 DROP/CREATE 或 SQLite rebuild 绕过。对每项声称可编辑操作验证 create→load→edit→reload；只支持 create/drop 的对象验证 create→load→drop，不能强行执行引擎没有的 ALTER。

## 四、任务与依赖

依赖链为 U1 → U2 → U3 → U4 → U5 → U6 → U7。共享门面/枚举 exhaustive match 在 U1 补齐，尚未实现能力先明确 gated，最终不能留下 U 范围的永久 stub。

每个任务步骤按小项执行，可分多次 edit/test，但不能停在横向“加完类型”就称端到端单元完成。用户要求仅当前 plan 阶段，本节命令全是**后续执行命令，不是本阶段已运行结果**。

### U1：配置 → 连接 → 查询 → 关闭重开

**修改文件：** Cargo.toml、Cargo.lock、src/profile.rs、src/persistence/profiles.rs、src/db/mod.rs、src/db/descriptor.rs、src/db/capabilities.rs、src/db/query.rs、src/model/profile_manager.rs、src/model/execution_target.rs、src/ui/profiles.rs、src/ui/icons.rs、src/connections/cli.rs、src/runtime.rs、src/app.rs、src/action.rs。旧 struct literals 的精确兼容文件见 change-scope.json。

**新增文件：** src/db/duckdb/mod.rs、session.rs、query.rs、value.rs；src/runtime/duckdb.rs；tests/duckdb_profile.rs、tests/duckdb_adapter.rs。

1. 写 profile 参数化测试：上述四种 URL、path/memory、read_only/schema、无密码 SSL 字段、未知参数、旧 profile 加载和序列化；先运行 `cargo test --test duckdb_profile`，预期新 enum/字段缺失失败。
2. 新增字段和 URL parser/exporter，历史 DTO 赋 None；更新所有已定位 struct literals，只作兼容补字段。更新 descriptor 数组、DRIVER_NAMES/DRIVER_LIST、图标各模式、关系型分类及现有 capabilities 测试中的枚举数量。
3. 在 Cargo.toml 固定依赖并 resolve Cargo.lock；运行最小 adapter 测试，确认 bundled 构建和 stable API。记录 native 版本；此后测试加 --locked，禁止每轮重建锁文件。
4. 写并实现 file root/clone 生命周期，probe/version/current_database、read_only native flag、清晰 file/config/SQL error 映射和 terminal sanitize。适配器所有分发完整，不适用接口返回明确 Unsupported。
5. 写 scan-based executor 和 start_result metadata，支持多结果和批次共享预算；预算耗尽继续执行剩余语句，执行失败后不运行后续语句。拆分前置语句结果不能丢，DDL/DML affected_rows 和 RETURNING 不重复执行。
6. 打通草稿与连接测试、CLI path 归一化、保存/reload；resolved catalog 回传目标后继续 queued SQL。runtime/duckdb 实例映射纳入 profile 修改/断开清理，console 会话路由先用于普通 query。
7. 用以下完整回归测试骨架建立最小纵向证据（可直接落 tests/duckdb_adapter.rs，取项目现有接口）：

```rust
use lazydb::{db::{DatabaseConnection, value::CellValue}, profile::import_connection_url};

#[tokio::test]
async fn duckdb_file_survives_close_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("analytics.duckdb");
    let url = format!("duckdb:{}", path.display());
    let imported = import_connection_url(&url, Some("DuckDB test")).unwrap();
    let db = DatabaseConnection::connect(&imported.profile, None).await.unwrap();
    db.execute("CREATE TABLE records (id BIGINT PRIMARY KEY, note VARCHAR)")
        .await.unwrap();
    db.execute("INSERT INTO records VALUES (1, 'saved')").await.unwrap();
    db.close().await;
    let reopened = DatabaseConnection::connect(&imported.profile, None).await.unwrap();
    let result = reopened.execute("SELECT note FROM records WHERE id = 1").await.unwrap();
    assert_eq!(result.result_sets.len(), 1);
    assert_eq!(result.result_sets[0].rows, vec![vec![CellValue::Text("saved".into())]]);
    reopened.close().await;
}
```

8. 补实际验证：相同 instance 切 schema 不丢内存表、不同 profile 隔离；空结果仍有列；中间 SELECT 两份结果；插入一次；文件锁/访问模式冲突；只读缺失不创建；Test Connection 缺失不创建但保存后正式连接能创建。

**复核：** root owner 与 session 不混淆；map 锁不跨 native await；用户名密码不会进入 DuckDB URL；不通过 SQLite driver 读取 DuckDB。

**A 命令：**

```sh
cargo test --locked --test duckdb_profile --test duckdb_adapter --test profile_compatibility --test driver_categories --test execution_target --test profile_draft --test connections_cli
cargo test --locked --lib db::query::tests
```

**验收：** 真正新建文件、执行 SQL、关闭并重开保留数据；内存配置和目标同步正确；旧配置不受影响；暂 gate 的能力有明确下一单元归属。

### U2：DuckDB 方言、原始语句和无损结果

**修改文件：** src/sql/dialect.rs、scope.rs、highlight.rs、format.rs、builtins.rs、completion.rs、semantic.rs、analysis.rs、derived_result.rs、relation_filter.rs、type_name.rs、risk.rs、catalog_change.rs、statement_kind.rs、transaction.rs、mod.rs；src/cli.rs、src/lsp/server.rs、src/lsp/catalog.rs、src/app.rs、src/ui/catalog_editor.rs；src/db/duckdb/query.rs、value.rs、src/db/column_type.rs、src/model/cell_editor.rs。

**新增文件：** src/sql/duckdb.rs（集中驱动专有规则）；tests/duckdb_sql.rs、tests/duckdb_values.rs。

1. 补 `SqlDialect::DuckDb` 和全入口映射。格式化、高亮各自的 parser 都使用 DuckDbDialect；LSP 添加显式 duckdb 选项，不能 fallback 到 SQLite。先写 sql_dialect_mapping、CLI parser 断言运行定位缺口。
2. 将 scanner 的 dollar quote 分支加 DuckDB，确认 bracket 为 list 而非标识符；逐例测试 dollar-tag、转义和普通单引号、嵌套注释、字符串分号。保持原文 ranges，不影响其它驱动扫描。
3. 在真实引擎执行 EXCLUDE/REPLACE、QUALIFY、GROUP BY ALL、FROM-first、list/struct、macro。parser 无法识别时保守风险+原文执行；format 不可破坏不认识语句。测试 COUNT/分页/WHERE/ORDER BY 对可安全封装 SELECT 成功，不可安全派生的原文直接执行。
4. logical type 解码实现 Null/bool/int/uint/float/text/bytes/date/time/timestamp；从 native 类型保留 timezone、precision/scale、单位和特殊边界。大整数/decimal/interval/嵌套值用规范文本，native 类型存在 ColumnMeta。
5. 用固定期望值测试 i64/u64/128 位边界、DECIMAL(38,9)、大嵌套 UHUGEINT、ENUM、UUID、JSON、LIST/ARRAY/STRUCT/MAP/UNION、BLOB、空值、NaN/Inf、日期无穷和 chrono 越界；不支持项专门断言类型与原因，不能整类弃用。
6. 测试 TUI cell preview/copy 与 agent JSON 的规范文本一致；在现有 CellValue 足够时不加新变体。确需新变体才更新 value.rs/serializer/preview 的全部 match，并记录 scope 扩展。
7. builtin completion 增加 DuckDB 函数/类型/DDL，catalog qualifier/case 语义与 engine 一致。风险区分原生会话/文件/扩展操作；本单元完成 parser 和 classifier 的正反例，MCP 接口在 U7 验证。

**复核：** 不重执行结果 SQL 做 CAST；不将 decimal/128 位转 f64；空集 metadata 仍存在；未知语法不会被误认为安全派生或只读。

**A 命令：**

```sh
cargo test --locked --test duckdb_sql --test duckdb_values --test sql_dialect_mapping --test sql_scope --test sql_format --test sql_highlight --test sql_risk --test sql_completion --test sql_semantic
```

**验收：** DuckDB 语法能编辑和原文运行，批次边界正确；支持类型结果可核对且无精度损失；真实缺失类型有清晰边界。

### U3：目录、搜索、Data/DDL 和 ATTACH 刷新

**新增文件：** src/db/duckdb/catalog.rs；tests/duckdb_catalog.rs、tests/duckdb_relation.rs。

**修改文件：** src/db/duckdb/mod.rs、session.rs、src/db/mod.rs、src/db/catalog.rs（仅必要 metadata 接点）、src/runtime.rs、src/runtime/duckdb.rs、src/action.rs、src/app.rs、src/model/execution_target.rs、src/model/explorer.rs、src/sql/semantic.rs、src/sql/completion.rs、src/sql/catalog_change.rs、src/lsp/catalog.rs。

1. 写 fixtures：主库和 attached tempfile、不同 schema 同名表、quoted catalog/schema/table、temp table、view、sequence、enum、scalar/table macro、index/PK/unique/check/FK。
2. 实现 duckdb_databases/schemas/tables/views/columns/indexes/constraints/sequences/types/functions 原生读取，绑定 lookup 值并分别 quote identifiers。过滤 internal 不过滤用户 temp；Function ID 区分 overload/native kind，builtin function 不全塞 Explorer。
3. 实现完整 CatalogRequestKey 验证、scope、keyset cursor、page size <=500 和 search；生成 >500 对象证明翻页无重复遗漏，cursor 跨 target/scope/epoch/profile 被拒绝。校验不支持 groups 按项目错误码返回，不让 LSP误判连接坏了。
4. preview 在拥有对象的 session 使用三段引用，验证 WHERE/ORDER BY、LIMIT lookahead、count 和零行列名；临时表依赖 owner session，跨 console 不泄露。
5. DDL 从原生 sql + 索引/comment/约束依赖组装，provenance 正确；保留 default/generated/precision/enum/宏定义，按稳定顺序输出，不在 UI 重造 DDL。
6. ATTACH/DETACH/CREATE TYPE/MACRO 等成功 SQL 触发目录 invalidation；默认 Selected 不突然包含新 attached database，All 才显示；DETACH 后旧 tab 保留快照、拒绝写。native error 或 unknown SQL 同样按保守刷新规则，不靠唯一 rename AST。
7. 将 discovered catalog/schema 与目标选择器、runtime map 统一；用 runtime 测试覆盖 file→attached schema→主库切换不新 open 内存库、跨 profile 无误路由。

**复核：** CatalogId 是 native catalog/schema/name，不是文件 path；schema scope 不等于安全权限；读 metadata 不对 MANUAL 会话再 BEGIN，不窥探 secrets。

**A 命令：**

```sh
cargo test --locked --test duckdb_catalog --test duckdb_relation --test catalog_contract --test catalog_scope --test catalog_reducer --test lsp_catalog --test connection_switch
```

**验收：** Explorer 可以发现并搜索真实用户对象，Data/DDL 完整且限域，ATTACH/DETACH 和 SQL DDL 后补全/目标一致。

### U4：MANUAL 事务、取消和断开清理

**新增文件：** src/db/duckdb/transaction.rs；tests/duckdb_transactions.rs、tests/duckdb_cancellation.rs。

**修改文件：** src/db/duckdb/session.rs、query.rs、mod.rs、src/db/mod.rs、src/db/transaction.rs（仅需要的生命周期接点）、src/runtime.rs、src/runtime/duckdb.rs、src/runtime/transaction.rs、src/model/transaction.rs、src/sql/transaction.rs、src/action.rs、src/app.rs。

1. 编写真实事务测试：MANUAL insert 另一 console 不可见、COMMIT 后可见、ROLLBACK 后消失、DDL rollback、错误后退出与恢复。由另一 clone 读验证隔离，不能只断言 UI state。
2. 实现 DuckDbTransactionBackend；begin 深度最多 1，成功 native control 后更新状态；不能调用上游固定 true 的 is_autocommit。嵌套/savepoint 不支持时明确拒绝。SQL 内部 COMMIT/ROLLBACK 通过既有路由同步，不让 depth 假保持 1。
3. MANUAL 租用原 tab 会话；加入“先 CREATE TEMP/SET，再进入 MANUAL”测试。分页/count/derived query、数据网格 mutation 使用该事务连接；其它 console 的临时对象不继承。
4. operation guard 独立持有 interrupt handle；阻塞任务持租约直到结束，执行结束与 guard disarm/generation 更新必须串行同步。覆盖调用开始前取消、执行中取消和已结束后旧取消，不使用只改 AtomicBool 无法中断引擎的假取消。
5. 写可控通知/barrier 测试：确认查询启动后 cancel 大 range 聚合，超时只是测试上限；下一条 SELECT 正常；MANUAL native 调用结束→ROLLBACK→结果确认。取消写语句时另一连接验证无残留未提交值。
6. 覆盖 close tab、改 profile、disconnect、timeout 和 runtime shutdown。执行无法确认/回滚失败进入 quarantine、Unknown history，错误提示不硬编码 SQLite。普通取消沿用 Unknown，不能无证据改成确认回滚。
7. 更新 quit/review/reducer 测试覆盖 DuckDB，保障退出不被后台 native 操作无限阻塞。文件锁在最后会话释放后可重开。

**复核：** interrupt 不持执行 mutex；drop 外层 future 不等于 native 已结束；不能早释放租约；unknown commit outcome 不宣称成功或失败已知。

**A 命令：**

```sh
cargo test --locked --test duckdb_transactions --test duckdb_cancellation --test sqlite_transactions --test quit_transaction_review --test transaction_reducer --test transaction_sql --test connection_switch
```

**验收：** 两 console 隔离、原会话保留、真实 commit/rollback、取消后清理和复用均有 native 证据，无后台继续执行或迟到中断。

### U5：网格新增、修改、删除与冲突

**新增文件：** src/db/duckdb/mutation.rs；tests/duckdb_relation_mutations.rs。

**修改文件：** src/db/duckdb/value.rs、catalog.rs、transaction.rs、mod.rs；src/db/mutation.rs、src/db/column_type.rs、src/db/mod.rs；src/model/cell_editor.rs、src/model/relation_edit.rs、src/model/relation_review.rs；src/ui/relation.rs、src/app.rs、src/runtime.rs。

1. 新增实际请求→mutation→SELECT 验证，无主键表 `INSERT RETURNING`、有/复合主键插入、DEFAULT VALUES、sequence/default、NULL 和 generated 限制。必须返回 engine 最终行，不拿草稿拼结果。
2. 注册 Returning 策略；binder 根据 metadata 转 native types，所有值绑定，DEFAULT 是受控 SQL token；标识符 quote，拒绝未知列/越界/metadata 变化/错误 profile 或 scope。
3. update/delete 以完整 PK 顺序定位并检查原值 `IS NOT DISTINCT FROM`；0 行返回冲突、多于 1 行视为错误；更改 PK 后用 RETURNING 返回新定位值。
4. 批量删除在同一事务，后续冲突/约束失败不得部分提交；AUTO 和 MANUAL 均测试。默认值、bytes/date/decimal/unsigned 往返精确；display 规范文本不能直接当 SQL 表达式。
5. 按列声明不支持绑定的复杂类型，禁止仅凭 Text 允许所有类型编辑；允许其它安全列操作时不无故禁整表。只读、非 LIVE 快照、缺 PK update/delete 有明确 disabled 原因。
6. reducer/runtime 验证 draft→review→apply→refetch；MANUAL 未提交状态可 rollback，取消不留下部分写；接口确认前不提前显示“已保存”。

**复核：** 所有请求都复查 scope/identity；不能按 rowid 伪造主键，不以 DELETE+INSERT 代替 UPDATE 绕过引擎约束。

**A 命令：**

```sh
cargo test --locked --test duckdb_relation_mutations --test relation_runtime --test object_mutation_contract
```

**验收：** 新增/修改/删除能通过 TUI 既有动作路由完成，权威行和冲突/回滚正确；能力文案与实现一致。

### U6：结构化对象编辑、DROP 和目录同步

**新增文件：** tests/duckdb_catalog_mutation.rs。

**修改文件：** src/db/duckdb/mutation.rs、catalog.rs、mod.rs；src/db/mod.rs、src/db/catalog_mutation.rs、src/db/catalog_drop.rs、src/db/column_type.rs；src/model/catalog_editor.rs、src/model/explorer_actions.rs、src/model/explorer_add.rs；src/ui/catalog_editor.rs、src/ui/icons.rs、src/app.rs、src/runtime.rs、src/runtime/duckdb.rs、src/sql/catalog_change.rs。

1. 给 schema/table/view/sequence/index 列出 create/edit/drop availability，隐藏 PostgreSQL 专有 owner/tablespace/cache 等字段。Type/Macro 标明 SQL 管理，不新增不支持的 draft variant。
2. 实现 create schema/table/view/sequence/index 的 plan、definition 和执行；TableDraft 覆盖 default/NULL/PK/unique/check/FK。生成 SQL 经过 target/scope 校验，不根据 UI 显示字符串直接执行。
3. 写 schema/table/view/sequence/index 的 create→definition 测试；编辑支持字段（table rename/列变更/default/nullability/comment、view query/comment 等）→reload。native unsupported 字段 test availability 和原因，不使用 DROP+CREATE 或 SQLite rebuild。
4. 执行前重新加载 baseline fingerprint；外部修改、rename、stale epoch、只读/权限/范围外、依赖和约束失败均拒绝或保留清晰错误，失败后不抹掉原对象数据/DDL。
5. DROP 使用既有 plan/confirmation/qualified identity，支持 native table/view/schema/sequence/type/macro/index（按实际 kind 验证）。拒绝将数据库节点映射成删文件或 DROP DATABASE；ATTACH/DETACH 仍是原生 SQL。
6. 补 SQL Type/Macro 全链路测试：CREATE→Explorer/DDL→CREATE OR REPLACE（仅 engine 支持的 macro）→refresh→DROP；用户 type 的 engine 无 ALTER 就不宣称可编辑。index/constraints 不重复当可删条目。
7. rename/mutation 成功后刷新 Explorer selection、completion、已开 relation tab identity 与请求 generation；stale response 丢弃。dirty tab 的写阻断直到身份重新验证。

**复核：** 每项 advertised 编辑都有 round trip，菜单开启不等于完成；表变更不会丢失未修改依赖/comment；无不可恢复自动迁移。

**A 命令：**

```sh
cargo test --locked --test duckdb_catalog_mutation --test catalog_drop --test catalog_mutation --test catalog_editor_reducer --test catalog_editor_state --test explorer_add
```

**验收：** 既有结构化编辑器对 DuckDB 适用对象可用，Type/Macro SQL 管理透明，DROP 和目录/补全刷新有端到端证据。

### U7：CLI/MCP/LSP、文档与收尾门禁

**新增文件：** tests/duckdb_agent.rs、tests/duckdb_cli.rs、tests/duckdb_lsp.rs；docs/duckdb.md。

**修改文件：** src/agent/policy.rs、src/agent/service.rs、src/agent/types.rs、src/agent/catalog.rs；src/connections/cli.rs、src/cli.rs、src/lsp/server.rs、src/lsp/catalog.rs、src/lsp/mod.rs；README.md；docs/configuration.md、docs/database-capabilities.md、docs/coding-agent-access.md、docs/sql-language-server.md；.github/workflows/ci.yml。本阶段文件清单确认 pages 仅有安装脚本，无数据库列表，不修改 pages。

1. AgentService 使用临时 file profile 验证 connections/context、query、schema search、preview、write；对默认 deny、只读 profile、Production policy、明确允许写分别断言。native read_only 也实测，不能只断言 authorize 函数。
2. MCP deny 覆盖次级副作用：nextval/setval、EXPLAIN ANALYZE 内写语句、COPY TO、ATTACH/LOAD/INSTALL 和不可安全分类语句；读查询/常规 metadata 仍可用。结果预算、target catalog/schema、复杂类型 JSON 和错误 sanitize 与 TUI 一致，wire version 不任意变化。
3. headless 每次独立连接，文档明确 `:memory:` 不跨请求持久；不能复用 TUI 隐式会话。cancel/error 路径也释放实例；文件数据库供多次 agent 调用持久化。
4. CLI 用隔离 app root/config 运行创建/列表/测试/导入导出与 capabilities JSON；确认 driver 列表包含 duckdb，profile 路径和只读信息不串，缺文件 test 无副作用。
5. LSP `--dialect duckdb` 与绑定 DuckDB profile 均使用正确方言，catalog 补全 columns/macros、diagnostics、scoped warm 和 stale page 丢弃。用已存在协议 harness，不依赖 Neovim 人工启动。
6. docs/duckdb.md 写最短示例、文件/内存 URL、read-only/锁、catalog/schema/ATTACH/session/temp 语义、数据类型/结构化编辑限制、取消与 agent 边界；更新其它文档矩阵和 README。不编造 users/roles/dashboard，不顺带改历史 plans。
7. 新 native tests 默认纳入 cargo all-targets，CI 仅在 needed 时补 C++ 工具链/合理资源限制；不以不编译 DuckDB 降低开销。可增加普通 PR Windows build/test job验证 bundled（明确为本任务新增 A 的 CI 证据，不冒称原有门禁）。现有 release workflow 已覆盖五平台，除非必要不改发版逻辑。
8. 先执行 U7 定向测试，随后统一执行 P。检查 docs tests 与 CLI snapshots 的 driver 数量更新。以 `git diff --check` 和实际 diff 对照 change-scope，删除仅由本任务生成且无必要的调试代码，绝不删除未知用户文件。

**A 命令：**

```sh
cargo test --locked --test duckdb_agent --test duckdb_cli --test duckdb_lsp --test agent_policy --test agent_serialization --test agent_mcp --test lsp_protocol --test connections_cli --test docs
git diff --check
```

**P 命令：** 执行第二节列出的现有 CI 门禁，每项记录真实结果。不得将 cargo test 中因缺服务 skip 的远端 adapter 视为已做 live 兼容验证。

**S 检查：** 条件允许时一次 PTY 覆盖创建连接、F2、SQL、Data/DDL、网格 mutation、MANUAL 和取消；无法运行记录原因而不是要求用户手工完成。普通 CI/build 可用时记录 Linux/macOS/Windows 编译，完整 release 平台构建留给发布流程；不为取得证据发布。

**验收：** TUI/headless/LSP 使用同一 DuckDB 合同，策略不退化，文档能力真实，P 完成或有明确环境限制和 Luna 审查结论。

## 五、改动范围和兼容性审查

change-scope.json 是预计改动范围，不是要求每个文件都改。覆盖：固定新增模块/测试、必要模型/分发/路由/文档/CI、已确认 ConnectionProfile struct literals。后续编译发现更多构造点或 exhaustive matches，Luna 先按实际最小路径补 scope，不能笼统授权修改整个 src/tests。

现有测试需要补 `duckdb_path: None` 的点已搜索定位：agent_context/mcp/policy/selection/service、catalog_editor_reducer、connection_switch、execution_target、global_workspace、keymap、profile_draft/reducer/runtime/transactions、redis_browser_tabs/loading_lifecycle、relation_runtime/tabs、sqlserver_adapter；源文件另有 db/mssql.rs 的内测构造和 profile/manager/target。不是所有参考测试都会修改，只有新增行为断言/枚举数量或编译兼容必要时修改。

原驱动无需行为变化；必须保护 SQLite 连接内存语义、原有 SQL 风险分类和序列化、PostgreSQL 主键版本、Oracle feature、Redis 非 SQL 路由。共用 helper 修改后定向回归其受影响测试。没有计划删除或重命名文件；如实施出现重命名，scope 必须同时列两端。

## 六、Luna 收尾复核清单与交接

1. U1–U7 每项以实际 diff、native 测试和 capability 矩阵确认；未完成业务不是 blocked，持续实施到完成，不要求 resume。
2. 对六个关键风险逐项核对证据：实例/会话/租约；native cancel 与归还竞态；precision/复杂值；catalog/path/target/scope；prepare/autocommit API；bundled 链接。
3. 按 U/P/A/S 区分证据来源，不把人工检查、跨平台或新增 benchmark 变成用户强制门禁。环境受限记录一次修复重试及结论，不无限重跑。
4. validation.md 每项标明实际代码版本/工作区；未运行写未运行，不复用 analyze 的静态证据当当前通过。
5. branch/commit/merge 按后续工作流授权由 Luna 处理，本计划本身不执行这些操作；不触发 release。

**下一阶段第一个可执行动作：** 核实当时 checkpoint/diff，在工作流任务分支/worktree 开始 U1；先补 `tests/duckdb_profile.rs` 的 URL/兼容测试并运行定位缺口，再锁定 bundled 依赖、实现实例与会话基础和 file close/reopen 测试。普通实现取舍自行完成，持续推进到 U7。

**本 plan 阶段完成标准：** plan.md 完整，change-scope.json 为合法 JSON 并覆盖所有预计变更，本轮静态复核记录已追加；业务 diff/index 无变化，最后写入用户本轮指定 plan token 的 completed 回执。
