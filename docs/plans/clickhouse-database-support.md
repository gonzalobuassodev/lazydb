# ClickHouse Support Implementation Plan

> **执行者：Luna。** 用户已选择自动工作流顺序实施；不询问执行方式，不启动子 Agent。按本计划逐项完成、复核和验证，普通实现问题自行修复。当前阶段只交付计划，尚未实施任何业务功能。

**Goal:** 为 LazyDB 增加与现有 TUI、CLI、MCP、LSP 入口一致的 ClickHouse 完整支持，并正确表达其非事务批次、异步 mutation 和非唯一排序键语义。

**Architecture:** 增加独立 `DatabaseConnection::ClickHouse`，使用现有 reqwest 的 HTTP(S) transport 和流式 `TabSeparatedWithNamesAndTypes` 动态解码。复用现有 Action → Command → Runtime、结果预算、catalog 请求/身份/代际合同；只对空结果元数据、原生进程 ID 和 ClickHouse 原生目录定义作必要共享扩展。SQL 使用已锁定 sqlparser 0.62 的 ClickHouseDialect，不引入通用驱动框架。

**Tech Stack:** Rust 2024 / MSRV 1.94；Tokio；reqwest 0.12 + rustls；sqlparser 0.62；serde、url、uuid、chrono；ratatui；ClickHouse 25.8 LTS 基线。

---

## 0. 任务约束、输入与交接方式

- 输入分析为本目录 `analysis.md`。本计划保留其 HTTP(S)、原生动态结果和能力诚实声明的决策，并将大单元拆为可执行任务。无需重新通读旧日志。
- 原工作区 `/Users/yelog/workspace/tui/lazydb`；目标 `main`；起点 `0e68ae20e9a455d7c0f9b46bcecff2e7d0283059`。plan 阶段再次确认 HEAD 等于起点、status 无输出。未提交依赖文件：**无**；本计划不依赖原工作区未提交行为。未来实施前若出现本地差异，按实际 diff 判定，不清空、stash 或自动搬运未知用户改动。
- plan 阶段 `checkpoint.json` 不存在，只读失败已记入 validation；不创建/改写 checkpoint 或 state。Luna 实施时优先读插件提供的 checkpoint 和当前 diff，忽略旧轮次“等待 resume”文字。
- 此次只在指定任务目录写 `plan.md`、`change-scope.json`、validation 补记和当前精确指定的回执；不额外写 docs/plans。后续任务标题可用“ClickHouse 完整支持”，分支建议 `feat/clickhouse-support`，由自动工作流在计划完成后命名/创建。
- 技能 writing-plans 已调用。技能的默认工作树、docs/plans 输出、频繁 commit 和执行方式提问服从用户本次约束；本计划不要求每步提交，不在当前阶段提交。实施/审查/纠偏/提交合并由 Luna 完成。
- 后续按 U1→U6 顺序推进。U4 拆为独立对象闭环，每完成一项再做下一项；不能用“尚未完成”作为 blocked。只有缺外部输入/权限或需用户决定的互斥要求才 blocked；环境受限验证按 §1 记录，不无限重试。

## 1. 要求来源与验证分级

| 标记 | 来源 | 性质和完成规则 |
|---|---|---|
| R | 用户“增加 ClickHouse 完整支持” | 必须覆盖连接/查询/类型/目录/DDL/搜索/监控及现有 CLI/MCP/LSP 横向入口。下文为工程验收解释，不代表用户逐条指定了 wire format、版本或测试工具 |
| G | 项目 `CONTRIBUTING.md:6-12,57-76` 与 `.github/workflows/ci.yml` | 修改后 fmt、clippy、all-targets/all-features 测试；遵守 reducer 无副作用、异步身份/代际、可靠行定位、未实现动作隐藏、终端输入安全、配置变动同步文档/测试。属于现有强制门禁 |
| A | 本计划为实现 R/G 选定的自动化证据 | 每项改动的定向自动测试；新增 ClickHouse 强制 live CI、mock TLS/取消/异常/精度等。属于此次实现验收方案，不能冒称项目原来已经要求这些新测试；缺 live 环境不得声称已完成数据库验收 |
| S | 补充建议 | 人工 TUI/PTY smoke、真实 Cloud endpoint、第二个新版本、内存/吞吐 benchmark、no-default-features check。不是用户/项目强制门禁，不因无法运行自动阻塞交付；记录剩余风险，由 Luna 收尾审查决定是否补证 |

G 最终命令（在实施 worktree、功能齐备后执行一次；后续只按相关变更重跑）：

```sh
cargo +1.94.0 fmt --check
cargo +1.94.0 clippy --locked --all-targets --all-features -- -D warnings
cargo +1.94.0 test --locked --all-targets --all-features
```

版本与 CI 一致；若本机只有 1.94.1，可先定向验证，但不可将其冒称已验证 1.94.0。工具链不可用时记录本机限制并在 CI 获得 G 证据，不擅自更改 MSRV。

A live 统一约定：`LAZYDB_TEST_CLICKHOUSE_URL` 由 fixture/CI 提供，`LAZYDB_REQUIRE_DATABASE_TESTS=1` 时缺失或连接失败必须 fail；普通本地未配置时打印 SKIP，验证记录单独列 skipped。不可只运行可跳过的全量测试就声称 live 通过。

S 检查最多一次有针对性的环境修复重试；若仍不可用，记录命令、退出结果和范围，继续独立工作，不反复 progress。上一阶段发现 Docker daemon 不可用只代表当时环境；实施时按需检测一次，不自动启动用户桌面应用。每次记录代码 commit + 相关 diff 状态、环境、命令/退出码/通过与 skipped 数；旧结果不复制成当前结果。

## 2. 交付合同与实现决策

### 2.1 能力与兼容性

1. 独立 ClickHouse 产品、`clickhouse` 序列化 kind、SQL/表式交互（沿用 Relational 类别）；旧七产品枚举顺序/显示/持久化兼容。
2. 应用 URI `clickhouse://` 为 HTTP 8123，`clickhouses://` 为验证 HTTPS 8443；显式 port 保留。默认 user/db=`default`；不抢占全局 `http(s)://`，不宣称这些应用 URI 是 JDBC/native 标准。password 用 header，不能放请求 URL/日志/Debug。
3. ClickHouse profile 的 SSL Disable→HTTP；其他 SSL 模式→不降级的验证 HTTPS，新表单明确显示 HTTP/HTTPS；`clickhouses` 与 Disable 冲突报错。Require 不关闭证书校验；不增加自签自动信任、自动认证重试或 native 端口转换。
4. DatabaseIsSchema：ExecutionTarget `(profile, db, Some(db))`，catalog 的合成 schema 与数据库同名，实际 SQL 只引用 `db.object`。schema picker 不允许独立编辑；所有请求使用捕获的 scope/target。
5. `catalog/relation_ddl/monitoring` 在相应完整路径通过 A 测试后开启；`cancellation=false`，直至服务端 query ID 注册、独立 `KILL QUERY ... SYNC` 请求和确认状态具备端到端覆盖。`manual_transactions=false`，`relation_edit=false` 且逐操作 insert/update/delete 关闭。不能只隐藏 UI 而留下后端可调用路径。
6. SQL console 的原生 INSERT、ALTER UPDATE/DELETE、DDL、GRANT 等仍支持；ClickHouse key 不能作为网格唯一行定位，实验事务/rollback 不承诺。用户可用现有确认和权限策略执行合法 native SQL。

### 2.2 Transport、结果、错误和取消

- 新建 `src/db/clickhouse/` 的 mod/transport/query/value/catalog/mutation/principal/monitor 模块，后文给出实际文件。stream 功能优先用 reqwest 已有 chunk API，只有需要 bytes_stream 时启用现有依赖 `stream` feature；无默认新增生产依赖。
- Query POST body 为原 SQL；default_format 固定 TSV names/types；参数编码、headers sensitive、redirect 禁止、连接/读取/取消有界超时。写入不自动 retry；只读 request 设置服务端 readonly 防线，用户 SETTINGS 不得覆盖。
- 字节状态机跨 chunk 解析 names/types/rows，校验列数、escape、结束状态和异常；Nullable 原始 `\N` 在反转义前判 NULL，Text `\\N` 不混淆；String/FixedString 可逆解码，非 UTF-8→Bytes，不 trim NUL。
- Int/UInt≤64、Bool、有限 Float/chrono 可安全表达的日期映射现有 CellValue；大整数、Decimal、含糊时区/越界时间、非有限 float、Array/Tuple/Map/Nested/JSON/Variant 等保留精确文本与原始 type_name，未知类型不静默置 NULL。
- accumulator 增加 metadata-only 初始化，确保空 SELECT 有列；共享 QueryBudget 跨 result set 保留连续前缀；满预算继续有界 drain 检测尾部错误。单 row/cell framing 上限独立于保留预算；若太大需中止，执行取消并明确 budget error/unknown，不缓存全响应。
- HTTP 200 不能判成功；识别 header/native code、旧式 exception 和新式 tagged tail，先验证 framing，避免将普通 cell 的 `Code:` 内容当异常；有界错误 body，终端安全化、脱敏。
- 全缓冲逐条请求，source range 分段，不以 AST.to_string 重写全部 SQL；批次统一预检、共享预算、首错停止，明确成功前缀/失败位置。partial 结果不能丢失，写结果不确定必须到达输出与 history/agent。
- 单次执行/buffer session 独立串行；SET/USE/临时表只承诺同次 buffer，不承诺跨 run 或 Cloud 负载均衡 session。internal metadata/monitor/cancel 不共用用户 session。
- SELECT 顶层文本 FORMAT 可安全规范化为网格格式并给出说明；不改 INSERT FORMAT 或字符串/注释中的 FORMAT；二进制导出/INFILE/OUTFILE/WATCH 明确拒绝，混合 raw payload batch 在无法可靠分界时执行前拒绝。WITH TOTALS/extremes 必须保留独立结果或明确该查询不支持网格执行，不能合并成普通数据行。
- Runtime 仅在服务端取消闭环实现后才开启 ClickHouse server cancellation：保存 owner/generation/query_id/cancel sender，CancelQuery 优先发信号，独立控制请求 `KILL QUERY WHERE query_id={id:String} SYNC`，超时后清理本地 task；零匹配/跨节点/无权/早于注册均不假装确认。此前 capability 保持 false，文案必须说明本地 request abort 不表示远端停止。disconnect/shutdown/page/count/agent timeout 覆盖相同清理合同；不能只在 Drop spawn。
- ACK 不等于异步 mutation 完成；取消不回滚已应用数据。执行结果扩展优先添加兼容的可选 diagnostics/certainty，成功前缀/unknown affected rows 可解释，不把统一 affected_rows=0 宣称真实修改数。

### 2.3 目录与结构化修改

- 主源 system.databases/tables/columns，补充 dictionaries/functions/data_skipping_indices/projections；SQL UDF 只显示可确认用户定义项；字段是否存在按 25.8 fixture 确认，不能使用当前文档中新版列而无 gate。
- Dictionary 为 schema-level 非 relation 对象，Projection 为 table child；扩展 CatalogKind/ObjectGroup/metadata 验证和 consumers，不能仅加 enum 而生成非法 CatalogEntry。Dictionary 不普遍开放 relation-grid，native 可预览形态按 adapter 显式处理。
- keyset cursor 绑定版本/target/scope/connection，稳定 name/kind/id 排序，SQL 层 LIMIT page+1，<=500；权限/epoch失败保留 stale tree。搜索同样 scope 下推且携带真实祖先；模糊匹配可忽略大小写，身份绑定必须区分 Foo/foo。
- system.columns 的 DEFAULT/MATERIALIZED/ALIAS 分开，PRIMARY KEY/sorting 表达式不进入可编辑网格的 unique key metadata。无外键/identity/autoincrement 等就 Unsupported，不能虚构。
- scoped preview 先校验对象，filter/sort、limit+lookahead/offset，count 只在 resolve_total 时发。DDL 保留 canonical SHOW CREATE 的 engine/keys/codec/TTL/settings/TO；包含外部来源密码时须安全省略敏感子句且标明不能原样重放。
- 原生目录模型使用 `ClickHouseObjectDefinition` 变体和 CH draft/options，包含 canonical baseline fingerprint、native engine/options 和可编辑字段，避免在每个 PG 定义中插空字段。必须保留未改属性；已有数据表不得 drop/create 来模拟改列。
- 支持普通数据库/表/列/视图/MV/索引/投影/check/字典的有定义操作；复杂 engine、安全无法表达的 external source 编辑 operation-level gated，native SQL 管理通路保留。不能因“可用 SQL”而省略计划内常用对象闭环。
- users/roles 只读取安全列，禁止 auth_params/hash 和直接显示 SHOW CREATE USER；生成认证省略的定义。权限/member 操作对现有可表达对象白名单适配，不照搬 PG privileges。账号创建秘密独立执行、预览/history/MCP 不包含；若无法保证，关闭具体表单而非整个角色读取。
- ProcessRow 原生字符串 ID 兼容扩展，ClickHouse完整 query_id 用于展示/复制/筛选/稳定选择，不能 hash/truncate 成 PID；旧驱动数字显示/行为不变。metrics 缺失保留缺失，不能伪造 PG commits/WAL 或连接总数。

## 3. 实施顺序与复核规则

依赖顺序：U1(任务1–3) → U2(4–7) → U3(8–10) → U4(11–14) → U5(15–16) → U6(17–19)。fixture 在任务2建立，随各单元增加，不等全部功能完毕才尝试 live。运行时 `Command` 实际定义在 `src/action.rs:1521`；`src/commands.rs` 是命令目录/可用性定义，不是运行时 Command enum，后者仅在动作可用性需要同步时修改。

每项执行节奏：先补能暴露业务错误的测试 → 单个测试确认预期失败 → 实现最小完整路径 → 同目标测试通过 → 按列出的“复核”检查调用方/失败路径 → validation 简记。本计划不要求给低影响文案逐项补测试，也不写仅镜像 match 分支的无意义测试。`cargo test --locked --test <name> <case> -- --exact` 可用于单个初始用例；下面列的是该任务最终定向命令。首次新增 target 尚未存在时先创建测试文件再运行，不能将“no test target”当有效业务失败。

## U1：从 profile 到可见结果

### 任务 1：产品注册、URI、目标与持久化

**修改：** `src/profile.rs`、`src/db/descriptor.rs`、`src/db/capabilities.rs`、`src/model/profile_manager.rs`、`src/model/execution_target.rs`、`src/model/explorer_actions.rs`、`src/ui/profiles.rs`、`src/ui/icons.rs`、`src/persistence/profiles.rs`、`src/cli.rs`、`src/lsp/server.rs`。必要枚举适配清单见 change-scope；不得将 ClickHouse 默认为 MySQL。
**新增测试：** `tests/clickhouse_profile.rs`。**扩展测试：** `tests/profile_url.rs`、`tests/profile_draft.rs`、`tests/profile_compatibility.rs`、`tests/driver_categories.rs`、`tests/execution_target.rs`、`tests/database_capabilities.rs`。

1. 写 URI 表驱动用例：HTTP/HTTPS 默认端口；显式 port/IPv6/percent-encoded database/user/password；空密码、默认库、TLS 冲突、重复/未知 option、禁止 query_id/session_id 注入；roundtrip 不含秘密。
2. 注册 kind/format/descriptor/UI 字段与图标；SSL切换不覆盖显式端口；禁止独立 schema；补 scope/target clone/apply/valid 分支。
3. 以 v6 当前 profile format 实际规则为准，验证 ClickHouse 保存/重载、旧格式 load_report、未知 kind 保留为 unavailable，不为新增 enum 无意义 bump 版本；必要处设置兼容 url_format 默认。
4. 更新所有 DatabaseKind/SqlDialect exhaustive match 的安全含义：未实现功能返回 Unsupported；无 panic/todo、无默认开启能力。

**验证 A：** `cargo test --locked --test clickhouse_profile --test profile_url --test profile_draft --test profile_compatibility --test driver_categories --test execution_target --test database_capabilities`。
**复核：** profile Debug/URL redaction；kind 切换保留用户输入；MySQL/MariaDB/Redis 的既有默认/字段不改变。
**验收 R：** 可创建并重开 ClickHouse profile，目标/SSL/端口可靠；未接通能力不显示可执行动作。

### 任务 2：测试 fixture、HTTP client 与 probe

**新增：** `src/db/clickhouse/mod.rs`、`src/db/clickhouse/transport.rs`、`tests/clickhouse_adapter.rs`、`tests/support/clickhouse.rs`、`tests/fixtures/clickhouse/init.sql`、`tests/fixtures/clickhouse/users.xml`、`docker-compose.clickhouse.yml`。**修改：** `src/db/mod.rs`、`tests/support/mod.rs`；`Cargo.toml`/`Cargo.lock` 仅实际 feature 解析变化需要时改。

1. 在 tests/support 增 ClickHouse URL helper，遵循现有 MariaDB REQUIRE_DATABASE_TESTS 的行为；建唯一测试库/清理 guard；密码不打印。固定 25.8 LTS 的已存在 patch/tag，获取后锁 digest，不能写虚构 digest；compose 绑定 loopback、healthcheck、独立账户，默认不部署外部源。
2. 为 transport 增可重复 mock（现有依赖和本地监听器），检查 auth header sensitive/无 secret URL、redirect 拒绝、错误体上限/timeout/no retry；不为 mock 引入生产依赖。
3. 实现端点解析和 client 构建，probe 发 `SELECT version(), currentDatabase(), currentUser()`，映射 ServerInfo；版本过低明确 Unsupported/Configuration。
4. 接 `DatabaseConnection::connect/connect_target/probe/kind/close`；关闭 adapter 会清理自己请求上下文，不把共享 clone 的另一 console 请求误杀。

**验证 A：** `cargo test --locked --test clickhouse_adapter`；fixture 可用后 `docker compose -f docker-compose.clickhouse.yml up -d --wait`，安全注入测试 URL 后用 `LAZYDB_REQUIRE_DATABASE_TESTS=1 cargo test --locked --test clickhouse_adapter -- --nocapture --test-threads=1`。环境变量必须实际设置，不能省略后把失败当业务回归。
**复核：** compose 不占生产库、不装载用户数据；plain HTTP 仅为显式选择；写请求无重试。
**验收 A/R：** adapter 真正连通、probe 返回真实产品/版本/库/用户，失败能准确分类；mock 和 live 结果分开记录。

### 任务 3：流式首查询、零行列与结果预算入口

**新增：** `src/db/clickhouse/query.rs`、`src/db/clickhouse/value.rs`。**修改：** `src/db/query.rs`、`src/db/clickhouse/mod.rs`、`src/db/clickhouse/transport.rs`、`src/db/mod.rs`；测试 `tests/clickhouse_adapter.rs`。

1. accumulator 补 metadata-only 入口，测试零行、零预算、连续多 result 的列/affected_rows，不虚构 row。
2. 实现 names/types 与行的字节流 decoder；从 API入口到 execute_with_budget 共享 budget，不 fetch_all；最小 UInt64/Text/Nullable/String 类型可用。
3. live 查询 `SELECT toUInt64(1) AS n, '' AS empty, CAST(NULL AS Nullable(String)) AS missing`；零行 `SELECT toUInt64(number) AS n FROM numbers(0)`；Memory 表 INSERT/readback。
4. 通过现有 reducer/runtime 查询路径证明测试连接→保存→懒执行可见结果；补 `tests/console_lazy_connection.rs`/`tests/connections_cli.rs` 必要断言。

**验证 A：** `cargo test --locked --lib db::query`；`cargo test --locked --test clickhouse_adapter --test console_lazy_connection --test connections_cli`；live 强制同任务2。
**复核：** 空 DDL response 不当空 SELECT，列名/类型准确；开启 UI 查询不依赖未实现 MANUAL worker。
**U1 验收：** profile→connect/probe→typed SELECT/空结果→保存重载完整，旧产品入口仍运行。完成后继续 U2，不以 U1 作为最终“完整支持”。

## U2：可靠 SQL、类型与取消

### 任务 4：完整动态值、framing 与尾部错误

**新增测试：** `tests/clickhouse_values.rs`、`tests/clickhouse_transport.rs`。**修改：** clickhouse transport/query/value；`src/db/query.rs`、`src/db/value.rs`、`src/security.rs` 仅有必要共享缺口时。

1. 固定 bytes fixture 对每一个可能 chunk 边界切分，覆盖 names/types/escape 跨块、无尾换行/异常尾、列数错、无效 header；测试 header 只一行、DDL 空 body 与零行 metadata。
2. 值测试覆盖 NULL/empty/字面 `\N`、NUL/tab/LF/CR/backslash、非 UTF-8 FixedString；UInt64 max、±128/256整数、Decimal scale、NaN/Inf、Date32 越界、DateTime64(9)/时区、Nullable/LowCardinality、Array/Tuple/Map/Nested/JSON/Variant/Geo。
3. 实现递归 type wrapper 和安全标量 decode；未映射复杂值用精确 Text/Unsupported；复制/agent 序列化值不能只比较屏幕预览。
4. 固定行缓冲硬上限和有界错误读取；保留预算耗尽后仍检测尾部错误，200/非2xx/native code分开分类；普通 cell `Code: 62...` 不得误报。

**验证 A：** `cargo test --locked --test clickhouse_values --test clickhouse_transport`；live 值集用强制模式执行 `--test clickhouse_values`。
**复核：** 不经 lossy UTF-8/f64 转 Decimal；budget 是保留字节预算而非无限 wire分配许可；旧输出 sanitizer 不破坏复制的原始值。
**验收：** 异型列仍可整表浏览，精度/字节不损失，超大/错误 response 不被显示为成功。

### 任务 5：方言、范围、全缓冲与读写授权

**新增：** `src/sql/clickhouse.rs`、`tests/clickhouse_sql.rs`、`tests/clickhouse_execution.rs`。**修改：** `src/sql/mod.rs`、`src/sql/dialect.rs`、`src/sql/scope.rs`、`src/sql/risk.rs`、`src/sql/statement_kind.rs`、`src/sql/transaction.rs`、`src/sql/catalog_change.rs`、`src/sql/highlight.rs`、`src/sql/format.rs`、`src/agent/policy.rs`、clickhouse query。

1. 接入 ClickHouseDialect 和独立 highlight dialect_ref；确认格式化不重写 raw payload。对 `ARRAY JOIN/PREWHERE/FINAL/LIMIT BY/SETTINGS/lambda` 建合法样本，对 parser 不支持的语法保守诊断/Unknown。
2. 新 source-slice splitter 复用 scanner，把 ClickHouse backticks/escape/注释/quoted semicolon 覆盖；不能简单 `split(';')`，不能 AST.to_string 代替原文。全批预检再发送。
3. 单执行 UUID session 逐语句执行，共享 budget，首错停止；SET/USE 只影响此 buffer，new run 还用原目标；两个 console不共享session。
4. 加 SHOW/DESCRIBE/EXPLAIN、INSERT/ALTER UPDATE/DELETE、RENAME/OPTIMIZE/SYSTEM/KILL/GRANT/REVOKE风险测试；MCP query 未知 fail closed。readonly SQL/URL SETTINGS覆盖尝试被拒绝。
5. 仅规范化已识别 SELECT 输出 FORMAT；INSERT FORMAT VALUES/有限文本单请求保留 raw payload，混合不能安全分段提前拒绝；INFILE/OUTFILE/WATCH/二进制 FORMAT 提前拒绝并说明。

**验证 A：** `cargo test --locked --test clickhouse_sql --test clickhouse_execution --test sql_scope --test sql_risk --test sql_execution --test sql_dialect_mapping --test sql_highlight --test sql_format --test agent_policy --test transaction_sql`。
**复核：** 所有新特例局限 ClickHouse；Unknown 不放宽 headless，TUI仍可明确确认；事务控制UI/reducer/backend均拒绝。
**验收：** 当前/选择/全缓冲执行范围准确，batch非原子且stop位置可知，读写保护不绕过。

### 任务 6：执行诊断、部分成功与 history

**修改：** `src/db/query.rs`、`src/db/mod.rs`、`src/runtime.rs`、`src/action.rs`、`src/commands.rs`、`src/model/sql_history.rs`、`src/persistence/sql_history.rs`、`src/agent/types.rs`、`src/agent/service.rs`、`src/app.rs`、`src/ui/execution_confirm.rs`、`src/ui/sql_preview.rs`、clickhouse query；测试 `tests/sql_history_errors.rs`、`tests/sql_history_model.rs`、`tests/agent_serialization.rs`、`tests/clickhouse_execution.rs`。

1. 先定义兼容执行诊断合同：query_id、statement ordinal、completed-prefix counts、outcome certainty、accepted async、affected_rows known/unknown；新字段有默认值，旧 JSON/历史格式兼容，避免无需要的持久化schema升级。
2. 测试 batch“INSERT成功→坏SQL失败→第三条未发送”，输出/历史保留成功前缀；网络断线/timeout 写入状态Unknown，错误不自动标Confirmed。
3. 解析可信 summary written_rows；缺失时明确unknown，不拿 read_rows当受影响行数。async mutation显示已接受、不承诺数据完成或rollback；用户明确SETTINGS保持原意。
4. 将diagnostics贯通 runtime → Action → 输出/通知/history，CLI/MCP保持安全结构化信息；history 不存密码/敏感请求header。

**验证 A：** `cargo test --locked --test clickhouse_execution --test sql_history_errors --test sql_history_model --test sql_history_recorder --test agent_serialization`。
**复核：** 其他adapter无diagnostics仍按原行为；失败与unknown不混淆；persist的当前schema如确实改变需迁移测试，不能暗改SQL schema。
**验收：** 用户能知道已执行哪些语句及哪些结果未确认，不因错误误以为前面的写被回滚。

### 任务 7：AUTO取消、timeout、disconnect和隔离

**新增测试：** `tests/clickhouse_cancellation.rs`。**修改：** clickhouse transport/query/mod；`src/runtime.rs`、`src/runtime/connections.rs`、`src/action.rs`、`src/commands.rs`、`src/app.rs`、`src/agent/service.rs`；扩展 `tests/connection_switch.rs`、`tests/relation_runtime.rs`。

1. 先补可控mock：取消早于dispatch、早于server注册、读到一半、执行完成竞态、cancel后新generation；每次query_id唯一，不复用。
2. 注册请求cancel context，独立请求发KILL SYNC，处理finished/waiting/zero-match/permission/timeout；取消通道不与用户session互锁。保留有限deadline，不无限轮询。
3. CancelQuery不直接abort ClickHouse request而丢失远端取消；后备abort须记录未确认。补page/count/derived/preview、disconnect/shutdown和agent timeout清理，不影响其他目标。
4. live长查询用bounded polling等待注册，再cancel并确认无自己的query_id；fixture failure paths由mock补齐。mutation取消之后不标rollback。

**验证 A：** `cargo test --locked --test clickhouse_cancellation --test connection_switch --test relation_runtime --test profile_transactions --test transaction_reducer`；强制live `--test clickhouse_cancellation -- --nocapture --test-threads=1`。
**复核：** 无共享query_id map跨profile；身份/代际先验证；Cloud跨节点零匹配不宣称完成；drop不是唯一取消机制。
**U2验收：** SQL、类型、预算、错误、部分写状态及取消形成真实闭环；cancellation能力此时才开启。

## U3：目录、搜索、数据与编辑器

### 任务 8：原生catalog identity/分页/metadata

**新增：** `src/db/clickhouse/catalog.rs`、`tests/clickhouse_catalog.rs`。**修改：** `src/db/catalog.rs`、`src/db/mod.rs`、`src/model/explorer.rs`、`src/model/explorer_actions.rs`、`src/ui/icons.rs`、`src/lsp/catalog.rs`；CatalogKind consumers见scope。

1. 为Dictionary schema object、Projection relation child补合法CatalogEntry构造/validation、ObjectGroup/图标，拒绝跨profile/非法祖先；不将它们粗暴归Table。
2. native查询库/表/视图/MV/字典/UDF、列/跳数index/projection/check；建canonical name与UUID辅助身份规则；临时/inner对象明确过滤或不可编辑。
3. metadata保留default_kind/type/comment/精度/非唯一key含义；对未知字段Unsupported、无权限错误与空结果区分。
4. 实现绑定scope/target/cursor version的keyset，SQL page_size+1；fixture两个库、Foo/foo、特殊标识符、501+对象；不得fetch_all再分页。

**验证 A：** `cargo test --locked --test clickhouse_catalog --test catalog_contract --test catalog_scope --test catalog_reducer --test explorer_state`，同target强制live。
**复核：** catalog scope不是用户任意SQL ACL；adapter负责元数据范围；无依据不暴露hidden账号/系统外部源秘密；旧CatalogKind语义不变。
**验收：** 目录能够逐页展开/刷新，各节点真实身份和native kind正确，过期页被拒绝。

### 任务 9：远程搜索、Omni、语义补全/LSP

**修改：** clickhouse catalog、`src/sql/semantic.rs`、`src/sql/identifier_match.rs`、`src/sql/completion.rs`、`src/sql/builtins.rs`、`src/model/omni.rs`、`src/lsp/catalog.rs`、`src/lsp/server.rs`、`src/agent/catalog.rs`；测试 `tests/clickhouse_catalog.rs`、`tests/clickhouse_sql.rs`、`tests/omni_navigation.rs`、`tests/omni_search.rs`、`tests/sql_semantic.rs`、`tests/sql_completion.rs`、`tests/lsp_catalog.rs`、`tests/lsp_service.rs`。

1. native搜索scope下推、rank→stable tie-break/limit/truncated；产出真实祖先，不污染loaded coverage；大小写模糊搜索与case-sensitive identity分开。
2. db.table映射DatabaseIsSchema；default namespace/alias/CTE/quoted identifier比较传入dialect，不全局改其他产品名称规则。Foo/foo两个对象不误判ambiguous/missing。
3. 加离线常用CH内建函数/type补全，catalog未完整时不得证明对象不存在；新对象类型的LSP/Omni定位与DDL action对齐。
4. 加用户切换目标时返回旧搜索/补全的代际拒绝，以及同名跨库连接隔离测试。

**验证 A：** `cargo test --locked --test clickhouse_catalog --test clickhouse_sql --test omni_navigation --test omni_search --test sql_semantic --test sql_completion --test lsp_catalog --test lsp_service`。
**复核：** 没有为补全发全量catalog查询；模糊匹配不用于rename/rebind；没有改动MyBatis业务逻辑。
**验收：** 搜索→定位→补全→执行所指相同原生对象，scope/generation一致。

### 任务 10：scoped relation preview、DDL和派生分页

**修改：** clickhouse catalog/query；`src/db/mod.rs`、`src/model/relation.rs`、`src/sql/derived_result.rs`、`src/sql/relation_filter.rs`、`src/model/relation_review.rs`、`src/ui/relation.rs`、`src/ui/query_bar.rs`；测试 `tests/clickhouse_catalog.rs`、`tests/clickhouse_sql.rs`、`tests/relation_tabs.rs`、`tests/relation_runtime.rs`。

1. 校验CatalogId/native对象/immutable scope后生成quoted db.object；WHERE/ORDER BY经CH方言验证，page.size+1判断has_next；resolve_total才count。
2. DDL读取保留原生细节并脱敏外部source；native UUID校验只在可证明身份时重绑；drop/recreate不同对象不自动接受。
3. ordinary view、self-storage MV、TO MV/Dictionary按可验证行为preview或具体原因，不把目标表静默当成原对象。
4. 派生分页仅安全SELECT；测试SETTINGS/FORMAT/WITH TOTALS/UNION，不能安全包装则保留原执行并禁用二次分页。totals/extremes不得作为普通row污染count。

**验证 A：** `cargo test --locked --test clickhouse_catalog --test clickhouse_sql --test relation_tabs --test relation_runtime --lib sql::derived_result`。
**复核：** 不自动FINAL、不每页count、无order不保证稳定顺序；row_versions=None，所有gridmutation仍禁用。
**U3验收：** Explorer/搜索/补全→数据/DDL→分页/刷新闭环正确，catalog/relation_ddl能力按实测开启。

## U4：原生目录修改（每种对象独立闭环）

### 任务 11：CH定义模型、计划与数据库/表创建

**新增：** `src/db/clickhouse/mutation.rs`、`tests/clickhouse_catalog_mutation.rs`、`tests/clickhouse_catalog_drop.rs`。**修改：** `src/db/catalog_mutation.rs`、`src/db/catalog_drop.rs`、`src/db/column_type.rs`、`src/db/mod.rs`、`src/model/catalog_editor.rs`、`src/ui/catalog_editor.rs`、`src/runtime.rs`、`src/app.rs`。

1. native definition/draft变体含canonical baseline/options；form仅CH显示engine/order/partition/TTL/settings；String默认类型，Nullable(T)不拼PG NOT NULL。
2. 建库native Atomic/default、MergeTree/Memory table创建，默认MergeTree + 显式ORDER BY tuple()；带分区/TTL/codec测试canonical reload。
3. drop plan正确quote/scope/破坏性确认，DDL非事务模式；baseline stale先拒绝，failure/unknown后刷新，不调用rollback。
4. 每种可展示动作完成create→load→drop；有权限失败及identity/epoch测试后打开对应profile_create/create/drop能力。

**验证 A：** `cargo test --locked --test clickhouse_catalog_mutation --test clickhouse_catalog_drop --test catalog_editor_state --test catalog_editor_reducer --test object_mutation_contract --test catalog_drop`，同新targets强制live。
**复核：** 不往PG DatabaseDefinition填假owner/encoding；未实现edit仍hidden。
**验收：** a创建真实CH库/表，DDL可读、drop审查准确，失败不伪装成功/rollback。

### 任务 12：有数据表的无损编辑与列闭环

**文件：** 任务11的native mutation/model/UI/runtime；测试 `tests/clickhouse_catalog_mutation.rs`、`tests/catalog_editor_state.rs`、`tests/catalog_editor_reducer.rs`。

1. fixture保留rows、engine参数、order/partition/primary/sample、TTL/settings/codec和不认识的子句，edit只改name/comment/一列；预览不能重造整个表。
2. 实现rename/comment、ADD/MODIFY/DROP列、DEFAULT/MATERIALIZED/ALIAS；兼容性/破坏性语法逐操作检查；影响key的非法修改不自动绕过server。
3. 重新读取definition做baseline比对；多条ALTER partial_applied后明确已执行步骤，刷新实际状态；不能以DROP/CREATE替代。
4. 先保持复杂engine不支持的edit为具体reason，再对可无损支持shape启用操作；数据和未改options前后相等。

**验证 A：** `cargo test --locked --test clickhouse_catalog_mutation --test catalog_editor_state --test catalog_editor_reducer --test object_mutation_contract`，强制live同target。
**复核：** 后台mutation ACK不是完成，秘密字段不参与可读baseline；column操作不是网格行操作。
**验收：** create→load→edit属性→reload→drop；现有数据、未改engine/options完整保留，stale/permission/partial路径正确。

### 任务 13：view与materialized view闭环

**文件：** clickhouse mutation/catalog、catalog native definition/draft/UI；测试 `tests/clickhouse_catalog_mutation.rs`、`tests/clickhouse_catalog_drop.rs`。

1. 普通View create/query change/rename/drop按server支持操作实现，query body保留原文，definition不丢security/未知子句。
2. MV区分TO target与self-storage，schema/engine只在适用形态出现，不默认POPULATE；建源表→建MV→insert→查目标验证效果。
3. MV不能安全ALTER的属性显示具体reason，保留SQL/DDL通路，不能自动drop/recreate；target依赖/权限失败可见。
4. 每种shape独立baseline、identity/refresh、drop操作测试后开能力。

**验证 A：** `cargo test --locked --test clickhouse_catalog_mutation --test clickhouse_catalog_drop --test catalog_editor_reducer`，live强制同新target。
**复核/验收：** View/MV不混淆，修改不会丢目标/engine或触发未请求回填；已advertise操作都能reload证实。

### 任务 14：index/projection/check/dictionary闭环

**文件：** clickhouse mutation/catalog；`src/db/catalog.rs`/catalog_mutation/drop；native draft/UI。**测试：** `tests/clickhouse_catalog_mutation.rs`、`tests/clickhouse_catalog_drop.rs`、`tests/catalog_contract.rs`。

1. 跳数index、projection、check按原生CREATE/ALTER ADD/DROP/可用编辑实现，create→load→改一可变属性（不支持直接edit时明确gated）→reload→drop；不伪装唯一约束。
2. Dictionary采用本地CLICKHOUSE source或无秘密fixture；create参数显式SOURCE/LAYOUT/LIFETIME/PRIMARY KEY，验证catalog与SHOW CREATE；secret-bearing外部source安全省略、不能回放删去秘密的定义。
3. 未改unknown字段完整保留；若编辑必须有损重建/必须未知秘密，关闭具体edit并给reason。安全可支持rename/comment等小操作优先，不因复杂shape缺口隐藏整个Dictionary读取。
4. 补object-kind构造、search/ancestors、drop SQL、confirmation和reconciliation，旧类型同样回归。

**验证 A：** `cargo test --locked --test clickhouse_catalog_mutation --test clickhouse_catalog_drop --test catalog_contract --test object_mutation_contract`，强制live同新targets。
**U4验收：** 常用对象的已展示create/edit/drop均有往返测试；复杂shape例外具体、无有损重建和秘密泄漏。不能将所有CH编辑统一Unsupported视为U4完成。

## U5：账号、权限与监控

### 任务 15：users/roles安全读取与原生权限

**新增：** `src/db/clickhouse/principal.rs`、`tests/clickhouse_principals.rs`。**修改：** `src/db/principal.rs`、`src/db/principal_drop.rs`、`src/db/mod.rs`、`src/model/principal.rs`、`src/model/catalog_editor.rs`、`src/ui/principal.rs`、`src/ui/principal_mutation_form.rs`、`src/ui/principal_mutation_confirm.rs`、clickhouse mutation。

1. 只选system.users/roles安全列，grants/role_grants单独读取，测试普通/受限用户、用户角色同名identity、排序、coverage；禁止auth_params/hash/原始SHOW CREATE USER。
2. 生成认证省略说明的DDL；schema/database/table/column/membership目标白名单quote和绑定；GRANT/REVOKE与member add/remove往返，拒绝stale/跨profile目标。
3. 角色create/edit/drop按CH语义完成；账号创建只有在SecretString独立执行/preview-history脱敏路径完整时advertise；安全不能表达的属性具体gated，不宣称读取覆盖等于修改覆盖。
4. 只读profile/生产策略保持生效，permission denied不是empty principals。

**验证 A：** `cargo test --locked --test clickhouse_principals --test principal_contract --test principal_overview --test principal_tabs --test principal_drop --test agent_security`，强制live `--test clickhouse_principals`。
**复核/验收：** safe list/definition/权限member循环有效，失败coverage准确，无密码/hash在任何输出/快照/history。

### 任务 16：metrics与完整query_id进程表

**新增：** `src/db/clickhouse/monitor.rs`、`tests/clickhouse_monitoring.rs`。**修改：** `src/db/monitor.rs`、`src/db/mod.rs`、`src/model/dashboard.rs`、`src/ui/dashboard.rs`、`src/app.rs`；现有 `src/db/postgres.rs`、`mysql.rs`、`mssql.rs`、`oracle.rs`、`sqlite.rs`、`redis/monitor.rs` 的ProcessRow literal只作兼容字段适配。

1. 增可选native_id或等价兼容typed ID；完整query_id用于选择/筛选/复制；现有numeric驱动展示不变，绝不hash冒充native PID。
2. system.events/metrics/asynchronous_metrics映射真实语义，缺失None；不把selected rows当select次数、不把process数量当总连接；counter/reset/uptime/node切换有gap。
3. system.processes LIMIT MAX_PROCESS_ROWS+1（当前 MAX_PROCESS_ROWS=2,000）、truncated/visibility，query/user/client/db/elapsed字段如实填，permission denied显示restricted/unknown；权限无法表明是否隐藏其他会话时使用unknown。
4. unit/mock覆盖first sample/NaN/reset/字段缺失、2001进程、ID重复前缀/selection稳定；live验证有限指标和长query可见。

**验证 A：** `cargo test --locked --test clickhouse_monitoring --test mariadb_monitoring --test ui_render --test app_flow`；`cargo test --locked --lib model::dashboard`；强制live仅新target，既有MariaDB未配置skip单列。
**复核/验收：** monitoring advertised后overview/charts/info/processes可用，不伪造事务指标，其他数据库PID交互不变。

## U6：全入口与最终验证

### 任务 17：CLI、MCP、LSP和项目权限闭环

**修改：** `src/agent/service.rs`、`src/agent/types.rs`、`src/agent/policy.rs`、`src/agent/catalog.rs`、`src/connections/cli.rs`、`src/cli.rs`、`src/lsp/server.rs`（复用既有能力，只有实际缺口时修改）。**新增测试：** `tests/clickhouse_agent.rs`。**扩展：** `tests/agent_service.rs`、`agent_policy.rs`、`agent_security.rs`、`agent_serialization.rs`、`agent_catalog.rs`、`agent_mcp_protocol.rs`、`connections_cli.rs`、`startup_profiles.rs`、`lsp_service.rs`。

1. project-visible profile选定→credential resolve→query结果/列类型/预算→schema search，CLI与MCP同一adapter；别的项目profile不可选。
2. query只读/execute显式授权，Deny/NonProduction/All+profile.read_only反例；原生SYSTEM/KILL/ALTER/SETTINGS不能绕过；Unknown保持拒绝。
3. MCP协议真实stdio往返，不只测serializer；取消/timeout不遗留未报告远端query；错误code/诊断partial/Unknown稳定且脱敏。
4. 离线 `--dialect clickhouse`/LSP name映射、补全/诊断可用，lazy connect与profile保存重载回归。

**验证 A：** `cargo test --locked --test clickhouse_agent --test agent_service --test agent_policy --test agent_security --test agent_serialization --test agent_catalog --test agent_mcp_protocol --test connections_cli --test startup_profiles --test lsp_service`，新live target强制执行。
**复核/验收：** 不需要TUI活动连接即可query/search，策略、scope、数据表示与TUI一致。

### 任务 18：文档与强制live CI

**新增：** `docs/clickhouse-test-database.md`。**修改：** `README.md`、`docs/database-capabilities.md`、`docs/configuration.md`、`docs/architecture.md`、`CONTRIBUTING.md`、`.github/workflows/ci.yml`、compose与fixture。若确实新增配置项再同改 `config/default.toml` 与对应配置测试，本计划默认不新增配置项。pages中尚未确认需要修改的具体页面不纳入范围；如实施定位到重复产品矩阵，先补最小具体文件清单再改。

1. 文档逐项写实际支持/限制：URI/TLS/8123/8443、默认库、版本/sys权限、类型表示、case/scope、async ACK/cancel、非事务buffer、MANUAL/grid不可用、复杂编辑shape、Cloud session/cancel限制。
2. CI固定25.8已验证image digest，fixture普通与受限账户；加env REQUIRE强制live测试，不因URL漏配return成功；TLS认证用重复fixture/mock证明，不要求真实Cloud凭据。
3. CI命令以本计划新targets实际实现为准，包含adapter、values、execution、cancellation、catalog、catalog_mutation/drop、principals、monitoring、agent；fixture清理只限本任务前缀对象/compose资源，不动用户库。
4. 文档矩阵和capability code逐项交叉，advertised操作必须有A往返证据；配置有变化时遵守G同步default/tests/docs。

**验证 A：** 先 `docker compose -f docker-compose.clickhouse.yml config --quiet`；设置测试URL后：

```sh
LAZYDB_REQUIRE_DATABASE_TESTS=1 cargo test --locked \
  --test clickhouse_adapter --test clickhouse_values \
  --test clickhouse_execution --test clickhouse_cancellation \
  --test clickhouse_catalog --test clickhouse_catalog_mutation \
  --test clickhouse_catalog_drop --test clickhouse_principals \
  --test clickhouse_monitoring --test clickhouse_agent \
  -- --nocapture --test-threads=1
```

**复核/验收：** CI未配置数据库时明确失败；没有标注新测试为历史强制规则，没有把计划能力写成已经通过。

### 任务 19：收尾审查、G全量与交接

**修改：** 仅前述发现的相关代码/测试/文档；本任务validation进度记录。**不新增不相关重构。**

1. Luna从实际diff检查：所有新增kind/dialect/object kind分支、通配分支遗漏、capability与operation-level一致性；secret与readonly、无损DDL、取消Unknown、byte预算是优先复核项。
2. 执行§1 G命令一次，随后执行任务18 A强制live；全量中其他驱动因无env跳过的测试明确列出，不能算integration通过。受影响共享模型的旧driver live按CI现有jobs取得证据。
3. 若G失败，定位修复后仅重跑相关target和受影响G检查；不为每个单元重复全量check+clippy+test。网络/daemon/PTY问题按验证分级与重试上限处理。
4. S：有环境时做profile→query→cancel→Explorer→DDL→mutation→monitor→disconnect键盘smoke；可选第二版/Cloud/no-default-features/内存压测。未做写明限制，无需人工等待确认。
5. 审查最终diff与change-scope：仅实际需要文件，额外必要consumer先更新范围；保留用户修改。总结每个R能力的代码/测试证据及具体gated例外，按后续工作流执行提交/合并，不在plan阶段操作。

**最终完成标准：** R各入口形成上述闭环；已advertise操作无stub且有A证据；G通过或明确提供环境受限及CI证据待补状态；S缺失仅记录不自动升级门禁；无未知写入被显示已回滚、无秘密泄漏、无有损DDL、无将排序key当唯一row identity。只有U1跑通不算完整交付。

## 4. 测试骨架与设计锚点

以下示例可直接作为任务1/3/4的初始业务断言；后续新增接口以当前模型为准。完整adapter实现由上述任务推进，不在计划中伪造大段未经编译的成品代码。

```rust
// tests/clickhouse_profile.rs
use lazydb::profile::{DatabaseKind, import_connection_url};

#[test]
fn clickhouse_http_url_defaults_to_http_port_and_default_database() {
    let imported = import_connection_url("clickhouse://localhost", Some("CH")).unwrap();
    assert_eq!(imported.profile.kind, DatabaseKind::ClickHouse);
    assert_eq!(imported.profile.port, Some(8123));
    assert_eq!(imported.profile.database.as_deref(), Some("default"));
}
```

```rust
// tests/clickhouse_adapter.rs: connect helper来自tests/support/clickhouse.rs。
// 必须让URL缺失在LAZYDB_REQUIRE_DATABASE_TESTS=1时panic，普通本地才显式SKIP。
#[tokio::test]
async fn zero_row_result_keeps_native_column_metadata() {
    let Some(database) = support::clickhouse::connect().await else { return };
    let outcome = database.execute(
        "SELECT toUInt64(number) AS n FROM numbers(0)"
    ).await.unwrap();
    assert_eq!(outcome.result_sets.len(), 1);
    let result = &outcome.result_sets[0];
    assert!(result.rows.is_empty());
    assert_eq!(result.columns[0].name, "n");
    assert_eq!(result.columns[0].type_name, "UInt64");
    database.close().await;
}
```

TSV必备fixture（实际bytes，反斜线语义不要经JSON/UTF-8二次损失）：第一行names、第二行types，Nullable raw `\N` 应为Null、String raw `\\N`应为Text；在每个byte边界分块后结果一致。异常fixture包含“预算已满后server抛错”、普通文本cell以Code开头、legacy异常和tagged异常；以framing区分，不靠全文contains。

任务11–14定义往返断言顺序固定：保存初始rows/SHOW CREATE→创建或载入→修改一属性→重新读取→断言目标属性变化且其他engine/keys/TTL/codec/settings/rows保留→清理任务对象。任何不支持shape要断言拒绝在发送前发生，不能仅assert错误文案。

## 5. 修改范围与允许偏差

机器清单为同目录 `change-scope.json`。列出预计实际改动文件，包含新模块/新tests/fixture和共享enum consumers。现有driver文件只允许本任务必要的exhaustive分支、definition/ProcessRow兼容适配；不重写其transport/business行为。测试文件也只为改变的合同增加用例/调整struct literal，不能批量改无关快照。

范围中少数现有文件（例如 persistence profiles、security、Cargo.lock、config/default.toml）是有明确可能的兼容/feature/配置依赖路径，实施查明无需修改则从最终范围移除。配置默认文件仅在任务18条件成立时修改。未能精确确认的配置测试使用最小 `tests` 目录候选会过宽，因此本次直接列现有 `tests/persistence.rs` 并在确实引入配置选项时重新定位补充，而非预先授权整个仓库。没有预期删除或rename，也没有未提交依赖文件。

## 6. 本阶段结果和下一步

plan阶段仅核对分析、代码路径、工作区和计划自身；未运行cargo/compose/live测试。当前不存在已实现的ClickHouse业务单元。Luna接手后先核对HEAD/diff/插件checkpoint，确定分支，然后从**任务1 profile URI/目标测试和任务2 HTTP probe fixture**开始，完成U1后连续推进U2–U6，不询问执行方式。
