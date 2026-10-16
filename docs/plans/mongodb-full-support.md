# MongoDB Full Support Implementation Plan

> 执行者：Luna。按本计划逐项实施、复核和记录证据；用户已选择自动工作流，不再询问执行方式，不启动子 Agent。本计划只读制定，当前不执行实现、提交或合并。

**Goal:** 让 LazyDB 在 TUI、CLI 和 MCP 中完成 MongoDB 的连接、发现、查询、类型保真编辑、管理、事务和恢复闭环。

**Architecture:** 使用官方异步 MongoDB Rust 驱动，新增 Document 交互模型和 Mongo 专用适配器、请求、标签及命令分类器。复用现有凭据、项目访问控制、JSON 编辑器、网格和 Action/Command 设施；SQL Catalog、SQL 风险解析和 Redis key 模型保持各自语义。Mongo 操作 worker 管理驱动 future、游标和会话，界面停止等待不会丢弃驱动 future 或被误报为写入未执行。

**Tech Stack:** Rust 1.94.0、Tokio、mongodb 3.9.1（crates.io 发布依赖及 Cargo.lock）、mongodb::bson、Serde、Ratatui、现有 rmcp、SQLite 历史存储、Docker Compose 测试服务。

---

## 0. 基线、授权及执行规则

本计划基于同目录 analysis.md。2026-10-15 plan 阶段实查 HEAD/main 都是 `0e68ae20e9a455d7c0f9b46bcecff2e7d0283059`，`git status --short --branch` 无变更条目。当前没有未提交行为依赖；future worktree 仅需起点提交，不需复制原工作区文件。checkpoint.json 仍不存在，未创建或修改插件状态。

本阶段只创建任务目录中的 plan.md、change-scope.json，追加 validation.md，最后写本轮专用回执。writing-plans 默认 worktree、docs/plans 文档、提交和询问执行方式均由用户本轮约束覆盖。后续 worktree/任务分支由插件调度；不得在 main 上执行本计划，不得擅自提交整个原工作区、stash 或清理用户修改。候选任务名 `MongoDB 原生客户端支持`，候选分支 `feat/mongodb-support`；实际命名交工作流。

后续开始实施时先读 checkpoint（若存在）、`git status --short` 和实际 diff，找到第一个未完成验收单元；不要重复全文调研，也不要把历史 next 提升成用户指令。每个 Task 依次执行“添加行为测试 → 定向失败 → 最小实现 → 同一测试通过 → 复核验收”，2–5 分钟的小动作作为检查点，较长任务拆在 Task 内推进。全局编译接线完成前的 expected fail 可以是缺新枚举/模块；接线完成后必须以行为断言失败为证据，不能把编译失败当功能测试通过。只有被后续阶段授权时才提交和合并，建议按端到端单元形成提交，不在本计划阶段运行 Git 写操作。

命名统一：Rust `MongoDb`（kind）、`MongoAdapter`、`MongoBrowserTab`、`MongoConsoleTab`；序列化驱动名固定 `mongodb`；URI 格式 `mongodb`/`mongodb-srv`，实际协议 `mongodb://`/`mongodb+srv://`。Mongo 专用模块目录采用 `mongodb`。新增功能不要求先把现有全体驱动重构为 trait/plugin。

## 1. 验证等级：不要扩大门禁

| 等级 | 来源 | 处理 |
| --- | --- | --- |
| U：用户范围与交付正确性 | 用户要求 MongoDB 完整支持、原工作区保护、任务专用产物、无子 Agent；分析中的产品能力矩阵由本计划落实 | 不以“已连接”代替完整支持；功能需要对应自动化证据，未知/未验如实报告。用户未要求人工签字、Atlas 账号、录屏或所有操作系统的人工验收 |
| P：仓库已有强制 CI | `.github/workflows/ci.yml` 的 Rust 1.94 fmt、clippy all-targets/all-features、test all-targets/all-features；macOS 构建及动态依赖检查；已有数据库/发行 CI | 功能齐备后执行适用命令一次；已有 CI 继续运行。未运行的外部 CI/服务不能冒称已通过。无关 Windows installer 手工检查不因此成为本任务新门禁 |
| F：本计划为功能新增的自动化验证 | Mongo URI/BSON/授权/reducer/持久化测试、隔离 Mongo CRUD/索引/事务/权限集成测试及新增 CI job | 这是工程验收方式，不是用户新增人工门禁。正常测试不得悄悄跳过数据库测试再称完整已验；缺环境记录未验范围并由 Luna 收尾审查安排 CI 或明确限制 |
| S：补充建议 | PTY/键鼠体验、小终端截图、真实 Atlas/SRV 云账号、TLS/X.509 完整环境矩阵、MongoDB 7 扩展兼容、压测及包体对比 | 默认不阻塞完成，不索要账号/人工批准。可以自动完成就记录；受限时最多一次有针对性的修复重试，之后记录限制交收尾审查，禁止无限 progress 重试 |

Mongo 标准 URI/SRV/TLS/X.509 是需要实现的功能；真实 Atlas 账号或人工操作不是强制验收条件。SRV 结构解析和 TLS 参数映射可自动测试；仅解析测试不可声称真实 DNS/证书握手已验。驱动声明兼容 4.4+ 不等于项目实测全部版本。核心 CI 采用 MongoDB 8；7 的扩展矩阵和 live SRV/TLS 环境是 S，不作为原项目已存在的 P。

统一记录：每条 validation.md 证据包含时间、命令、exit、commit/worktree 状态、相关文件、Rust/服务版本、是否跳过。只对代码或环境变化重跑相关项。单元内定向测试，最终全量一次；不要每轮 fmt+check+clippy+全量重复。不确定的写入、提交、取消状态不能为通过测试而降为普通失败。

## 2. 实施依赖与完成定义

| 单元 | Tasks | 用户可验收闭环 | 依赖 |
| --- | --- | --- | --- |
| A 连接 | 1–3 | 保存 Mongo profile → TUI/CLI 连接测试 → 正确展示服务器或错误 | 无 |
| B 浏览 | 4–5 | 展开库/集合 → Filter/Sort/Projection → 多页文档 → 完整 EJSON 复制/导出 | A |
| C 控制台 | 6–8 | JSON 查询/聚合 → 正确风险确认 → 结果/历史 → 重启恢复 | B |
| D 修改 | 9–10 | 新建/编辑/冲突/删除文档 → 集合、视图和索引管理 | C |
| E Agent | 11 | CLI/MCP 发现、读取和策略约束写入；原 SQL 协议兼容 | D |
| F 事务与管理 | 12–13 | begin→操作→commit/abort；用户角色管理与监控权限降级 | D；复用 E 授权 |
| G 交付 | 14–15 | 核心 CI、文档、完整验收映射和最终复核证据 | A–F |

实现须覆盖分析中的 11 类能力；不在本期引入 mongosh、SQL→Mongo 翻译、Atlas 控制平面、备份系统、GridFS 浏览器、CDC、CSFLE 密钥管理或需交互的企业云认证。遇到服务器不支持/无权限时明确诊断，不能用灰按钮掩盖整个计划未实现的功能。

## Task 1 — 测试环境、配置类型和驱动接线

**Files:** 新增 `docker-compose.mongodb.yml`、`scripts/mongodb/init-replica.js`、`scripts/mongodb/init-users.js`、`scripts/mongodb/wait-ready.sh`、`src/db/mongodb/mod.rs`、`src/db/mongodb/types.rs`、`src/db/mongodb/connection.rs`、`src/db/mongodb/worker.rs`、`src/profile/mongodb.rs`、`tests/mongodb_profile.rs`、`tests/mongodb_connection.rs`；修改 `Cargo.toml`、`Cargo.lock`、`src/lib.rs`、`src/profile.rs`、`src/db/mod.rs`、`src/db/descriptor.rs`、`src/db/capabilities.rs`、`src/sql/dialect.rs`、`tests/support/mod.rs`、`tests/driver_categories.rs`、`tests/database_capabilities.rs`、`tests/sql_dialect_mapping.rs`、`tests/redis_contract.rs`。现有 ConnectionProfile 构造器的机械补齐文件见 scope；仅新增 `mongodb: None` 等必要字段，不重写测试。

1. 在 mongodb_profile.rs 加注册契约：kind 序列化为 mongodb、属于 NonRelational、interaction 为 Document、try_for_database_kind 返回 None；默认 capabilities 不虚报 relation_ddl/relation_edit/SQL manual transactions。在 mongodb_connection.rs 添加真实 ping 及错误映射用例，当前无实现时应失败。
2. 编写 Compose 两服务：`mongo-rs`（MongoDB 8、单节点 rs0、绑定 localhost:27018）、`mongo-standalone`（localhost:27019）；公开测试账户仅限隔离 fixture。rs 成员广告 `localhost:27018`，让容器 mongod 同样监听 27018；单节点测试 URI 加 directConnection=true，避免宿主机无法解析容器名。容器 healthcheck 使用 mongosh；应用不依赖 mongosh。init-replica.js 等 primary 可写后再建测试用户，不能仅凭 ping 判断事务 ready。rs 启用认证时必须配置权限正确的节点 keyFile：在容器临时卷生成并限制权限，不把实际随机 key 写入仓库或日志；初始化可在受限 localhost bootstrap 完成后开启认证，脚本须明确初始化顺序。服务仅发布 127.0.0.1，普通 CI 不开放外网监听。
3. 使用 UUID 命名 `lazydb_mongo_test_<uuid>` 数据库；support helper 仅从 LAZYDB_TEST_MONGODB_URL 读取，不打印 URI。缺变量普通运行显式 SKIP；LAZYDB_REQUIRE_DATABASE_TESTS=1 时 panic。standalone 测试单独读取其变量，不能误用 rs 地址。清理仅删除 helper 创建的数据库/角色/用户。
4. 添加 `mongodb = "3.9.1"`，用 mongodb::bson，不添加第二个 bson crate，不打开 sync/native encryption/企业认证。正常 Cargo 解析生成 lock 后用 --locked；首次解锁命令 `cargo +1.94.0 check --all-targets`，只在依赖加入时运行一次，不手改 lock。
5. 定义 MongoConnectionOptions、MongoNamespace、MongoRequestKey、MongoCapabilities 基础类型；ConnectionProfile 增加带 serde default 的可选 Mongo options，ParsedConnectionUrl 同步携带该配置，旧构造器显式 None。DatabaseConnection 增加 MongoDb 分支，对 SQL 方法返回 Unsupported，不用 wildcard 掩盖未处理语义。
6. 注册第 8 个驱动、Document interaction，所有 “Redis 之外就是关系库” 调用按能力修正。SqlDialect 保持 SQL 枚举，不新增 Mongo dialect；所有 Mongo SQL 功能使用 try 路径关闭。

**验证:** `cargo +1.94.0 test --locked --test mongodb_profile --test driver_categories --test database_capabilities --test sql_dialect_mapping --test redis_contract`。预期新契约与 Redis 旧契约通过。

**服务准备（执行阶段 F）:** `docker compose -f docker-compose.mongodb.yml up -d`；`sh scripts/mongodb/wait-ready.sh`。脚本自行有期限轮询，无死循环；主机缺 docker 时转 CI 服务，不反复启动失败。

**复核/验收:** 发布 crate 可在 1.94 构建；驱动分类准确；测试服务隔离且 rs primary ready；这一步还不声称连接产品闭环完成。不要将 Mongo SQL unsupported 改为成功空结果。

## Task 2 — URI、凭据及配置迁移

**Files:** 修改 `src/profile/mongodb.rs`、`src/profile.rs`、`src/model/profile_manager.rs`、`src/persistence/profiles.rs`、`src/connections/cli.rs`、`src/security.rs`；测试 `tests/mongodb_profile.rs`、`tests/profile_url.rs`、`tests/profile_draft.rs`、`tests/profile_compatibility.rs`、`tests/persistence.rs`、`tests/credential_resolution.rs`。

1. 添加表驱动 URI 样本：单/多 seed、IPv6、转义用户名密码、空默认库、authSource、replicaSet、SRV 默认 TLS、重复 readPreferenceTags、X.509；断言 import→save→load→format 语义一致、password 只在 transient secret。
2. 解析独立 Mongo authority，不用 url::Url 把多 host 当一个 host。配置真值为有序种子/协议/非敏感 options，公共 host/port/user/database 为摘要；编辑简单字段同步更新 options，不允许两份真值分歧。ClientOptions::parse 是运行时最终校验，离线 SRV 导入不要求 DNS 成功。
3. query 参数按官方支持列表分 singleton/repeatable，校验大小写别名、冲突和值。敏感参数不能藏进 options：tlsCertificateKeyFilePassword、嵌套机制 secret/token 等本期拒绝持久化；加密私钥口令需求给出明确配置不支持诊断。SCRAM 密码走现有 CredentialPolicy，X.509 可存证书路径和 subject，验证 `$external` 规则。
4. 数据库未指定默认执行 test；在首次连接解析时固定有效 authSource（包含 Mongo 对默认库的原生规则），切库只改 execution target，不改变认证库。SRV 不人为补 27017。TLS 由 Mongo options 决定，表单 SSL 与 URI 冲突明确报错；不沿用 SQL Prefer 的模糊降级。
5. profile version 改 7，读取 v6 走当前 per-profile load report 路径，保留 2–5 迁移；不把 version match 改成 7 后丢掉 6。旧文件载入后 Mongo options 默认 None；未知 kind/config 仍 unavailable，修改别的 profile 不删除 unavailable 条目。
6. 测试所有凭据存储选择、URL display、CLI show/list、Debug/error/notifications 的脱敏；错误显示使用安全配置上下文和 Mongo error code，原始 URI 和含值 server message 不直接拼接。

**验证:** `cargo +1.94.0 test --locked --test mongodb_profile --test profile_url --test profile_draft --test profile_compatibility --test persistence --test credential_resolution`。预期退出 0，旧凭据存储及脱敏契约保持通过。

**复核/验收:** 不丢多 host/SRV/options；密码不进 TOML/输出；旧 v6 配置可读写；默认数据库切换不会变更 authSource。合法重复 tags 保留，敏感与矛盾配置 fail closed。

## Task 3 — 安全连接生命周期与 TUI/CLI 连接闭环（单元 A）

**Files:** 修改 `src/db/mongodb/connection.rs`、`src/db/mongodb/worker.rs`、`src/db/mod.rs`、`src/runtime.rs`、`src/runtime/connections.rs`、`src/model/profile_manager.rs`、`src/ui/profiles.rs`、`src/ui/icons.rs`、`src/model/execution_target.rs`、`src/connections/cli.rs`、`src/cli.rs`、`src/app.rs`；测试 `tests/mongodb_connection.rs`、`tests/mongodb_profile.rs`、`tests/connections_cli.rs`、`tests/profile_runtime.rs`、`tests/execution_target.rs`。

1. worker 测试用受控 future 和 drop guard：停止 UI 等待后 future 仍被轮询，完成结果回收；连接 generation 变旧不安装旧连接。分别覆盖连接超时、probe 超时、手动关闭、退出路径。
2. worker registry 默认最多 4 个 active driver 操作、32 个排队任务（常量可集中配置）；队列满明确 busy，不再 spawn 无限等待任务。worker 持有 owned Client/配置，UI 等待通道/JoinHandle 超时不 abort worker；完成后自动 unregister。driver Future 不跨应用锁运行。
3. ClientOptions 设置有界 serverSelectionTimeout/connectTimeout，操作在其支持范围设置服务端 maxTimeMS。建立 Client 后 ping 验证网络；buildInfo/用户信息权限不足只降级未知版本/用户，不能使 ping 成功的受限账户无法连接。
4. MongoAdapter::connect/probe/close 与 worker 接通。审计 runtime 中 connect_target 外层 10 秒 timeout、profile test/discovery/replace/shutdown 的 abort 和 timeout：Mongo 驱动操作留在 worker，外层只能停止等待。关闭期间拒绝新请求、清理 cursors/会话，在既定 driver 时限内 drain；进程最终强退无法确认的操作记 unknown，不能承诺强退前服务端必然停止。
5. profile manager 非关系库列表提供 MongoDB、URI 编辑和简单 host/port/authSource/TLS 字段；多 seed/SRV 高级配置以 URI 编辑并清楚显示，不悄悄折叠成单 host。CLI connections add/show/test 与启动 --url 复用此解析。目标 schema 必须 None，scope 为数据库筛选，Mongo 不出现 SQL schema picker。
6. 做真实连接成功/密码错/无认证/受限账户/standalone/不可达端口测试；验证 CLI 输出脱敏和错误分类。代码中 worker 类型及所有 SQL Unsupported 分支做完整枚举检查。

**验证:** `cargo +1.94.0 test --locked --test mongodb_profile --test connections_cli --test profile_runtime --test execution_target`；配置测试 URI 后 `LAZYDB_REQUIRE_DATABASE_TESTS=1 cargo +1.94.0 test --locked --test mongodb_connection -- --test-threads=1`。URI 通过环境传入，记录命令时不展开其内容。

**验收 A:** 用户可在 TUI/CLI 创建、保存、重开并测试 Mongo 连接；正确显示种类/服务器；失败不会泄密或污染另一连接；7 个旧驱动的连接契约不回归。

## Task 4 — BSON 保真、元数据与有界游标

**Files:** 新增 `src/db/mongodb/catalog.rs`、`src/db/mongodb/read.rs`、`src/db/mongodb/value.rs`、`tests/mongodb_values.rs`、`tests/mongodb_catalog.rs`、`tests/mongodb_cursor.rs`；修改 `src/db/mongodb/types.rs`、`src/db/mongodb/worker.rs`、`src/db/mongodb/mod.rs`、`tests/support/mod.rs`。

1. BSON 测试包含 ObjectId、int32/int64 同数值、Decimal128、非有限 double、DateTime、Timestamp、Binary subtype/UUID、Regex、MinKey/MaxKey、嵌套数组及 missing/null。Canonical EJSON 导出再导入断言类型/值保真；不能以普通 JSON numbers/f64 作为编辑中间值。
2. 原始结果持有 RawDocumentBuf 或等价原始 BSON 字节以检测重复键及不可无损编辑数据；生成 Document 投影仅用于显示。导出明确 EJSON 可表达边界；重复键/特殊文档允许只读查看原始 bytes，不静默 drop 重复键。网格字段并集有上限（默认 128），超出提示仍能查看完整文档。
3. 元数据按 Mongo namespace 返回 databases/collections/views/indexes/options，记录权限状态/continuation。listDatabases 使用 authorizedDatabases 能力；失败时只回落 configured/selected databases，不“猜”未授权库。不扫描所有文档做 schema 推断；采样注明 sampled 和样本量。
4. FindSpec 结构化保存 filter/sort/projection/skip/limit/batch；排序和 projection 保留 BSON key 顺序。页默认 100、最多 500；预算默认文档 10,000/原始和格式化字节各 16 MiB，先预估再分配输出，大文档超限返回明确 oversized/truncated 状态。
5. cursor 归 worker 所有，前端仅拿应用 opaque token；token 绑定完整 request identity 与 query revision。最大 16 个 live cursor、空闲 TTL 60 秒、缓存最多 3 页且受总字节预算；前页缓存失效则明确重新查询而非伪造稳定快照。清理 cursor 的异步 driver 工作也由 worker 执行。
6. 遍历 try_next 至页面/预算边界，超限关闭；不得 try_collect 全集合。count 为显式动作，estimated 与 exact 标识不同；分页不自动 count。真实测试插入 350 文档并设 batch_size=2，读取所有页证明不止 firstBatch，覆盖过滤改动/断线/过期 token 拒绝。

**验证:** `cargo +1.94.0 test --locked --test mongodb_values`；`LAZYDB_REQUIRE_DATABASE_TESTS=1 cargo +1.94.0 test --locked --test mongodb_catalog --test mongodb_cursor -- --test-threads=1`。

**复核/验收:** 文档原件保留、列缺失不作 null、数值不变型；游标受界且能跨批次；拒绝 profile/database/query 不匹配 token；元数据权限不足不影响授权库查询。

## Task 5 — Explorer、文档浏览与导入导出（单元 B）

**Files:** 新增 `src/model/mongodb_browser.rs`、`src/ui/mongodb_browser.rs`、`tests/mongodb_browser.rs`、`tests/mongodb_lifecycle.rs`；修改 `src/model/mod.rs`、`src/model/explorer.rs`、`src/model/explorer_actions.rs`、`src/model/explorer_add.rs`、`src/model/workspace.rs`、`src/model/tab.rs`、`src/model/navigation.rs`、`src/model/pane_navigation.rs`、`src/model/pane_resize.rs`、`src/model/omni.rs`、`src/action.rs`、`src/commands.rs`、`src/app.rs`、`src/runtime.rs`、`src/input/mod.rs`、`src/input/keymap.rs`、`src/input/mouse.rs`、`src/input/panes.rs`、`src/ui/mod.rs`、`src/ui/layout.rs`、`src/ui/icons.rs`、`src/ui/footer.rs`、`src/ui/shortcut_hints.rs`、`src/help.rs`、`src/clipboard.rs`、`tests/ui_render.rs`、`tests/mouse.rs`、`tests/keymap.rs`、`tests/omni_navigation.rs`。

1. 新增 MongoDatabase/Collection/View/Index explorer id 和 MongoBrowserTab；数据库→集合/视图→索引，不生成 CatalogKind::Table/假 schema。名称含点/Unicode 时保留原始 collection 名，不按 SQL qualified name 解析。
2. 定义 Mongo Load/Loaded/Failed actions 与 commands，所有结果携带 connection identity/tab/request/revision；reducer 检查后应用。关闭/切库/变更 Filter 使旧回复失效，选择项与滚动位置有稳定标识。
3. 浏览器包含 Filter、Sort、Projection JSON 输入、Data/EJSON 视图、分页/加载更多、刷新、显式 count 与元数据信息；JSON 错误保留输入并定位，不发送查询。复用 TextInput/JSON editor/DataGridState，不复用 SQL WHERE/ORDER BY 解析。
4. 接通键盘、鼠标、焦点、pane resize、Omni 集合搜索和 context help。文档/字段预览只渲染有界片段；复制和 EJSON 导出始终来自原件。导出显示是否只是已加载/截断的查询结果，避免冒称全集合备份。
5. 导入支持单文档或 EJSON 文档数组，预算内解析成写入草稿；此 Task 只生成草稿，Task 9 接保存。文件导出只在用户明确操作时写所选路径，文件存在走现有覆盖确认规则；自动化测试使用 TempDir。
6. Ratatui TestBackend 测试空集合、错误/加载状态、混合字段、窄窗口布局，reducer 测试 stale reply；鼠标/键盘命令等效且刷新不清空编辑中的过滤条件。

**验证:** `cargo +1.94.0 test --locked --test mongodb_browser --test mongodb_lifecycle --test explorer_state --test ui_render --test mouse --test keymap --test omni_navigation`。

**验收 B:** 从 Explorer 打开集合并分页、过滤、预览、复制、导出 EJSON；显示完整类型与预算状态。无 SQL 查询栏/SQL DDL，视图标只读。真实 cursor 证据复用 Task 4，不重复全套数据库测试。

## Task 6 — 有序命令解析、能力与统一授权

**Files:** 新增 `src/mongodb_command/mod.rs`、`src/mongodb_command/parser.rs`、`src/mongodb_command/policy.rs`、`src/mongodb_command/completion.rs`、`tests/mongodb_commands.rs`、`tests/mongodb_policy.rs`；修改 `src/lib.rs`、`src/db/mongodb/types.rs`、`src/security.rs`。

1. 先写纯函数测试：命令名首键、sort/compound index 键顺序、重复 JSON 键、第二个 JSON 文档、非法 EJSON、超深/超大输入。使用自定义有序 JSON AST/Serde visitor 检测重复键，随后转 BSON；不要把 sort 经默认 serde_json::Map 排序，也不要直接依靠 Document deserialize 检测已被覆盖的重复键。
2. ParsedMongoCommand 持有 typed variant、原始有序 BSON、database/所有 namespace、risk、history sensitivity。明确支持 `find/count/distinct/aggregate/explain`；`insert/update/delete/findAndModify`；`create/drop/renameCollection/collMod/createIndexes/dropIndexes`；`listCollections/listIndexes`；`ping/buildInfo/dbStats/collStats/serverStatus/currentOp`；Task 13 补 users/roles。每 variant 列出支持字段与类型，未知字段/命令 Unsupported。bulkWrite、mapReduce、eval 等不透传。
3. 为 command 和表达式采用语法位置相关的分类：$literal 的普通字符串/文档数据不当作操作符；$lookup/$unionWith/$facet 嵌套 pipeline 递归检查，$out/$merge 分类写入且收集输出 namespace；$where/$function/$accumulator 和未知可执行节点拒绝。update pipeline 的允许 stage 与 aggregate 分开。explain 检查内层，写命令 explain 不能走只读捷径。
4. authorize_mongo_request 统一校验 profile kind、read_only、数据库 scope、连接 generation、环境/WritePolicy、涉及的每个 namespace（包括 rename 源目标和聚合跨库引用）。拒绝用户 $db/lsid/txnNumber/autocommit/startTransaction 及手动 getMore/killCursors 注入；这些由 worker 管理。
5. 只读允许的 query 与变更 path 明确分离。TUI confirmation 按现有 ConfirmationPolicy 决定，profile.read_only 从不因确认被覆盖；production 写入和 destructive 操作展示原生目标与风险。MCP deny/non-production/all 复用现有 write_capability 判定，不另建冲突策略。
6. classify sensitivity 在编辑文本进入持久化之前可用：一旦识别/疑似 auth 管理命令切为 ephemeral 并禁止普通 autosave/history。未知命令可留内存提示，但不得在失败日志回显未经处理 payload。代码语法诊断不包含 secret 值。

**验证:** `cargo +1.94.0 test --locked --test mongodb_commands --test mongodb_policy`。

**复核/验收:** 不靠字符串前缀分类；恶意嵌套/跨库/会话注入被拒绝；字面量数据不误判；支持列表明确，未知不发往服务器；修改命令不能走读取接口。

## Task 7 — Mongo JSON 控制台与执行结果

**Files:** 新增 `src/model/mongodb_console.rs`、`src/ui/mongodb_console.rs`、`tests/mongodb_console.rs`；修改 `src/db/mongodb/read.rs`、`src/db/mongodb/worker.rs`、`src/db/mongodb/mod.rs`、`src/model/mod.rs`、`src/model/tab.rs`、`src/model/console_document.rs`、`src/model/pending_execution.rs`、`src/model/workspace.rs`、`src/action.rs`、`src/commands.rs`、`src/app.rs`、`src/runtime.rs`、`src/editor/mod.rs`、`src/ui/mod.rs`、`src/ui/execution_confirm.rs`、`src/help.rs`。

1. 选择独立 MongoConsoleTab，复用 ConsoleDocuments 的命名与开关管理，不把 Mongo outcome 塞进 ConsoleTab 的 SQL QueryOutcome。ConsoleDocument 加带默认 SQL 的 ConsoleLanguage；Mongo 与 SQL 类型绑定不可静默切换，换目标种类时提示新建正确语言控制台。
2. JSON 编辑支持离线创建、保存、格式化、基本集合/操作符补全、语法位置错误；执行一个完整文档，selection 如非完整命令直接报错。RunBuffer 不分割 SQL `;`；Mongo 不显示 SQL DerivedResult/WHERE/ORDER BY 功能。
3. `find/aggregate/listCollections/listIndexes` 用 typed API 或受管理 cursor；普通命令 run_command 不能把 firstBatch 当完整结果。禁止为方便统一返回而全量 collect。结果类型包含 documents/reply/counters、truncated、elapsed、certainty。
4. Begin→authorize→worker→reply 的全过程保持 operation id；读取停等可显示 Interrupted，写入停等显示 Pending/Unknown，后续 confirmed completion 可修正。generation 失效不能把结果写进新标签，但可更新对应操作历史。
5. 暂不执行未完成的 mutation/transaction variant，返回明确 Unsupported；Task 9/12 开通后补真实用例。控制台入口此时只读功能完整，不能将此作为全任务完成。

**验证:** `cargo +1.94.0 test --locked --test mongodb_console --test consoles_lifecycle --test console_manager_input --test editor_language`；使用 Task 4 服务证据，并新增 find/aggregate/explain 控制台执行集成用例到 mongodb_console.rs 的环境测试分组。

**复核/验收:** 输入分析中的 find/aggregate 示例可运行，JSON formatter 保留 sort 顺序，SQL formatter/LSP 不运行；超出首批仍能读；只读策略实际生效。

## Task 8 — 工作区与历史迁移（单元 C）

**Files:** 新增 `tests/mongodb_workspace.rs`、`tests/mongodb_history.rs`；修改 `src/persistence/workspace.rs`、`src/persistence/sql_history.rs`、`src/history.rs`、`src/model/sql_history.rs`、`src/model/sql_history_view.rs`、`src/model/console_document.rs`、`src/model/mongodb_console.rs`、`src/model/mongodb_browser.rs`、`src/app.rs`、`src/runtime.rs`、`src/ui/sql_history_modal.rs`、`src/ui/sql_preview.rs`、`tests/workspace_persistence.rs`、`tests/sql_history_model.rs`、`tests/sql_history_store.rs`、`tests/sql_history_recorder.rs`、`tests/sql_history_interaction.rs`。

1. 为 v3/v4/v5 workspace 和旧 history SQLite 建迁移测试，再新增 Mongo browser/console 快照读写。workspace version 6，明确保留 3/4/5 分支，避免 matches!(3|4|CURRENT) 无意排除 5。
2. 保存 namespace/Filter/Sort/Projection/语言/非敏感文本，不保存 cursor/session/运行中事务/connection pool/写入 baseline。控制台文件新增 `.mongodb.json`，旧 `.sql` 文件路径保持有效；路径检查和原子 temp rename 同样支持新后缀，禁止目录穿越。删除 Mongo 控制台删除对应文件，不能硬写 `<id>.sql`。
3. history schema 增加 language TEXT NOT NULL DEFAULT 'sql'，模型 ExecutionHistory 加语言字段；SQL 历史列名 sql 可暂留内部以缩小迁移，UI 按 language 高亮/格式化/重放。给所有 ExecutionHistory/ConsoleDocument 构造器补默认语言，scope 列出相应旧测试文件。
4. 敏感命令进入管理专用 ephemeral 编辑器，既不自动保存 console 文件也不写 history payload/result；保存状态拒绝绕过 sensitivity。历史可记录脱敏操作名/namespace/状态，但不能存密码/token/role auth 明细。日志/通知/错误同一策略。
5. 恢复仅恢复静态状态，不自动执行命令/重开事务；编辑基线必须重读；restart 将未确认写/commit 标 Unknown。confirmed terminal 不能被较弱的迟到 cancel 覆盖，沿用历史事实 reducer 思路。

**验证:** `cargo +1.94.0 test --locked --test mongodb_workspace --test mongodb_history --test workspace_persistence --test sql_history_model --test sql_history_store --test sql_history_recorder --test sql_history_interaction --test consoles_lifecycle`。

**验收 C:** 重启保留 Mongo 文本、语言和目标，旧 SQL workspace/history 无损可用；不存在自动重放、密码落盘和陈旧 cursor 恢复。历史呈现正确的已确认/未知事实。

## Task 9 — 文档 CRUD、冲突与批量结果

**Files:** 新增 `src/db/mongodb/mutation.rs`、`src/model/mongodb_edit.rs`、`src/ui/mongodb_edit.rs`、`tests/mongodb_mutation.rs`、`tests/mongodb_conflicts.rs`；修改 `src/db/mongodb/mod.rs`、`src/db/mongodb/worker.rs`、`src/model/mod.rs`、`src/model/mongodb_browser.rs`、`src/model/mongodb_console.rs`、`src/model/confirmation.rs`、`src/model/workspace.rs`、`src/mongodb_command/parser.rs`、`src/mongodb_command/policy.rs`、`src/app.rs`、`src/runtime.rs`、`src/action.rs`、`src/ui/mod.rs`、`src/help.rs`。

1. 先写一条文档 insert→read→edit→外部修改→conflict→reload→delete 的真实测试；包括非 ObjectId 的 _id、projection 不含 _id、missing/null、同数值不同 BSON type、数组/嵌套、$ 开头字符串与 NaN/特殊字段拒绝路径。
2. MongoEditDraft 保存原始完整文档/原 _id/namespace/request identity/revision。可视编辑从服务器取未投影完整文档；_id 不可改；EJSON import 的类型由 BSON 保留，不把数值全部转 double。关闭/切目标存在草稿时提供保存/丢弃/取消，失败保留草稿。
3. 使用服务端单次原子条件写：编辑已存在字段比较值及 BSON type、missing 比较 $exists，保守限定可验证字段路径；$literal 包裹被比较值，防止表达式注入。全量 replace/delete 使用经真实测试验证的完整 baseline 比较（例如 $expr 对 $$ROOT 与 literal 文档的值及类型约束），不允许 find 后无条件 replace。Mongo 比较可能把不同数值类型视相等，必须加类型约束；不能安全表达的特殊文档/路径/NaN 转只读并提示显式命令，不写伪 CAS。
4. plan 固定更新/删除对象与 baseline，执行前再次验证 profile.read_only、scope、generation、revision 和 confirmation。matched=0 是 conflict，matched=1 modified=0 是无变化；delete=0 同样不当作成功删除。
5. 支持 insert/update（含允许的 update pipeline）/replace/delete/findAndModify typed variants。多文档命令显示多文档风险；ordered/unordered 部分失败返回逐操作索引和 acknowledged/inserted/matched/modified/deleted/upserted。未知网络结果与 unacknowledged 写不能伪造精确计数；本期客户端编辑要求 acknowledged write concern。
6. write timeout 不应用层重放，保留 Unknown 并刷新核对；受控 worker 测试故障和晚到 confirmed。可视编辑 CAS 与用户显式发出的宽 filter 命令语义分开，后者展示风险而非偷偷附 baseline。

**验证:** `cargo +1.94.0 test --locked --test mongodb_browser --test mongodb_policy`；`LAZYDB_REQUIRE_DATABASE_TESTS=1 cargo +1.94.0 test --locked --test mongodb_mutation --test mongodb_conflicts -- --test-threads=1`。

**复核/验收:** 真实文档修改成功且类型不变；外部改动不静默丢失；_id/projection 保护有效；批量部分失败和未知结果准确；只读账户/策略在执行端拒绝。

## Task 10 — 集合、视图与索引管理（单元 D）

**Files:** 新增 `src/db/mongodb/management.rs`、`tests/mongodb_management.rs`；修改 `src/db/mongodb/mod.rs`、`src/db/mongodb/catalog.rs`、`src/db/mongodb/mutation.rs`、`src/mongodb_command/parser.rs`、`src/mongodb_command/policy.rs`、`src/model/mongodb_edit.rs`、`src/model/explorer_add.rs`、`src/model/explorer_actions.rs`、`src/app.rs`、`src/runtime.rs`、`src/ui/mongodb_edit.rs`、`src/help.rs`。

1. 定义 CreateCollection/CreateView/Rename/Drop/CollMod/CreateIndex/DropIndex 计划，展示原 namespace、目标、原生配置和 destructive 提示；由 Explorer 创建/编辑动作打开原生 JSON 草稿，不调用 SQL DDL generator。
2. 支持 collection options/validator/collation/time-series 等驱动/服务器允许的配置，服务器版本不支持返回明确错误。修改 validator 使用 collMod，不伪造 schema 字段编辑；空库只有第一次写/建集合才存在。
3. 复合 index keys 保持顺序；unique/TTL/sparse/partialFilterExpression/collation 和查看定义；阻止删除 _id_ 或 dropIndexes='*' 意外绕过保护（批量删除显示明确范围）。视图定义支持 viewOn/pipeline，浏览数据只读。
4. rename 明确 admin command db 与源目标 namespace scope，禁止越权跨库；成功后使原 cursor/namespace/editor 基线失效、更新 Explorer，不把旧编辑送入新集合。
5. 真实测试 create→insert→unique 冲突→list indexes→collMod validator→view→rename→drop，验证 drop 后旧 cursor/草稿不能继续写。TTL 测试检查索引定义，不等待不确定的后台过期时间。

**验证:** `LAZYDB_REQUIRE_DATABASE_TESTS=1 cargo +1.94.0 test --locked --test mongodb_management -- --test-threads=1`；`cargo +1.94.0 test --locked --test mongodb_browser --test mongodb_policy --test explorer_add`。

**验收 D:** 用户能完成文档及集合生命周期，创建复合/唯一/TTL 索引与原生 validator/view，危险操作确认和命名空间校验有效，视图不显示可保存数据编辑。

## Task 11 — Mongo 专用 Agent CLI/MCP（单元 E）

**Files:** 新增 `src/agent/mongodb.rs`、`tests/mongodb_agent.rs`、`tests/mongodb_mcp.rs`；修改 `src/agent/mod.rs`、`src/agent/service.rs`、`src/agent/policy.rs`、`src/agent/types.rs`、`src/agent/mcp.rs`、`src/agent/cli.rs`、`src/agent/catalog.rs`、`src/agent/context.rs`、`src/cli.rs`、`tests/agent_mcp_protocol.rs`、`tests/agent_mcp.rs`、`tests/agent_serialization.rs`、`tests/agent_cli.rs`、`tests/agent_security.rs`、`tests/agent_context.rs`、`tests/agent_policy.rs`。

1. MCP 名称固定 `mongo_query`、`mongo_execute_change`、`mongo_list_collections`、`mongo_describe_collection`。输入明确 connection/database/command（有序 JSON string）或 collection/limit；不得使用 JSON object schema 的无序 map 破坏命令/索引顺序。result 含 kind/namespace/documents 的 Canonical EJSON、counts、truncated/elapsed/certainty，不暴露 server cursor/session。
2. CLI 固定 `lazydb agent mongo query`、`execute`、`list-collections`、`describe-collection`；query/execute 的 --command 与 --file 互斥，后者必须在 canonicalized project root 内，复用旧 SQL 文件边界规则。CLI 传入 password 的建议用环境/secret store，不写示例真实 URI。
3. AgentService 在选择可见连接后检查 Mongo kind 和 scope，再用 Task 6 parser/policy；只读接口拒绝所有变更。写接口强制 deny/non-production/all、production 和 read_only 组合。显式数据库不能通过未公开 profile 或原始 URI 绕过项目可见性。
4. 暴露类型化 collection/index/validator 详情；Mongo search_schema/describe_object 按 kind 分派返回明确 Mongo 对象，不硬填 SQL CatalogKind。旧 SQL QueryInput.sql/结果形状不变，调用 Mongo 上的旧 SQL query 返回明确 unsupported 并指向新工具。
5. get_context/capabilities 增加向后兼容的 Mongo 能力信息，annotations 准确：读取 read_only_hint=true，写入 destructive_hint=true；说明 hints 不覆盖授权。需要敏感用户管理写操作时只使用明确管理路径，结果和日志脱敏。
6. 协议测试检查 tools/list 新工具和旧工具、JSON schema/错误代码、默认拒写及 malicious nested pipeline；真实服务测试 query→拒写→允许写→读取核验，按 fixture 清理。MCP/CLI 同一 payload 的风险结果一致。

**验证:** `cargo +1.94.0 test --locked --test mongodb_agent --test mongodb_mcp --test agent_mcp_protocol --test agent_security --test agent_serialization --test agent_cli --test agent_context --test agent_policy`；真实分组另用 `LAZYDB_REQUIRE_DATABASE_TESTS=1 cargo +1.94.0 test --locked --test mongodb_agent --test mongodb_mcp -- --test-threads=1`。

**验收 E:** Agent 实际发现并读写 Mongo，默认拒写/production/read_only 策略一致，旧 SQL API 无破坏；EJSON 不变型，不因结果预算继续无限读 cursor。

## Task 12 — Mongo 会话事务及退出闭环

**Files:** 新增 `src/db/mongodb/transaction.rs`、`src/model/mongodb_transaction.rs`、`src/ui/mongodb_transaction_review.rs`、`tests/mongodb_transactions.rs`、`tests/mongodb_cancellation.rs`；修改 `src/db/mongodb/mod.rs`、`src/db/mongodb/worker.rs`、`src/model/mod.rs`、`src/model/mongodb_console.rs`、`src/model/workspace.rs`、`src/model/transaction.rs`、`src/model/transaction_review.rs`、`src/runtime.rs`、`src/runtime/transaction.rs`、`src/app.rs`、`src/action.rs`、`src/commands.rs`、`src/ui/mod.rs`、`src/help.rs`、`tests/quit_transaction_review.rs`。

1. 基于 hello/topology/session 信息生成 Mongo runtime capabilities，standalone 不允许 manual transaction。SQL DatabaseCapabilities.manual_transactions 不直接让 Mongo 进入 SQL BEGIN worker；Document 路由使用专用能力。
2. MongoTransactionWorker 独占 ClientSession，串行 Execute/Commit/Abort/Shutdown，每次 typed CRUD/aggregate 附同一 session；不 parallel 操作 session，不自动执行事务不允许的 DDL/用户管理命令。session 中游标同 worker 所有，commit/abort 后全部失效。
3. 测试状态 Idle/Starting/Active/Committing/Aborting/Failed/OutcomeUnknown/Committed/Aborted。TransientTransactionError 提示整笔事务需用户重新操作，人工事务不自动重放。UnknownTransactionCommitResult 在有限次数（如 2 次、总时限内）重试同一 commit，不重放业务命令；耗尽仍 unknown。确认 abort 失败不能显示 RolledBack。
4. TUI 事务快捷键/命令根据 tab kind 分派；review 显示 JSON 命令及目标，不按 SqlDialect 渲染。脏草稿和活动事务的关闭/切库/退出先形成明确 deferred intent，选择 Cancel 可完整留在原状态。
5. 读取取消只是停止等待；worker 继续轮询。写入/commit 超时保留 pending/unknown，late confirmed 可更新原操作，不能覆盖别的 transaction generation。检查 shutdown registry 和 cursor cleanup 的 abort 调用，不误用 SQL worker.force_close。
6. 真实 rs 测试 begin→insert→事务外不可见→commit 可见、abort 后不可见、关闭确认、standalone 拒绝。commit error label/断网未知结果用可控 backend/transport fixture 验证，无需为每次单元测试引入真实网络故障；不能将模拟未知结果说成真实 failover 测试。

**验证:** `cargo +1.94.0 test --locked --test mongodb_cancellation --test quit_transaction_review --test transaction_reducer`；`LAZYDB_REQUIRE_DATABASE_TESTS=1 cargo +1.94.0 test --locked --test mongodb_transactions -- --test-threads=1`（同时配置 standalone URI）。

**复核/验收:** Mongo session 事务事实正确，standalone 不虚报支持；未知提交结果保留；退出/停等不直接 abort driver 操作，也不自动重放写入。

## Task 13 — 监控与用户/角色管理（单元 F）

**Files:** 新增 `src/db/mongodb/monitor.rs`、`src/db/mongodb/principal.rs`、`src/ui/mongodb_dashboard.rs`、`src/ui/mongodb_principal.rs`、`tests/mongodb_monitoring.rs`、`tests/mongodb_principals.rs`；修改 `src/db/mongodb/mod.rs`、`src/db/mongodb/types.rs`、`src/db/mod.rs`、`src/db/monitor.rs`、`src/mongodb_command/parser.rs`、`src/mongodb_command/policy.rs`、`src/model/dashboard.rs`、`src/model/explorer.rs`、`src/model/workspace.rs`、`src/app.rs`、`src/runtime.rs`、`src/ui/mod.rs`、`src/action.rs`、`src/help.rs`。

1. 定义 Mongo monitor snapshot：serverStatus/dbStats/collStats/currentOp 的采样时间、部分错误、缺权限字段。Mongo 自己的 JSON 详情/指标投影，不给 SQL process/thread 列填虚值；currentOp 若为游标形式由 worker 分页。
2. dashboard polling 有界、同类请求去重，后台权限失败不重试风暴；一个指标无权限不使整个面板失败，查询功能继续可用。killOp 为非必需增强，本计划不依赖其获得“硬取消”。
3. usersInfo/rolesInfo 显示原生权限/继承角色，用户角色按 database identity 区分。createUser/updateUser/dropUser/createRole/updateRole/dropRole/grantRolesToUser/revokeRolesFromUser 及角色权限增撤接 typed management plan；显式校验目标数据库/跨库 role 引用、权限和生产确认。
4. 用户密码输入在 SecretString/ephemeral editor 路径，不能进普通 ConsoleDocument、history、workspace、通知或完整 server error。不要自动删除最后管理员；客户端若无法证实安全条件，危险 drop 用户/角色展示明确目标和影响，仍需既有确认策略与服务端授权，不凭空引入全局“人工审计签字”门禁。
5. 测试隔离 root 管理 fixture 创建随机前缀测试 role/user，验证只读用户可读不可写、管理账户增撤角色实际生效，最后仅清理测试对象；restricted 用户 serverStatus/currentOp 拒绝只局部降级。

**验证:** `cargo +1.94.0 test --locked --test mongodb_policy --test mongodb_browser`；`LAZYDB_REQUIRE_DATABASE_TESTS=1 cargo +1.94.0 test --locked --test mongodb_monitoring --test mongodb_principals -- --test-threads=1`。

**验收 F:** rs 事务、监控和常规权限管理有可用入口与真实证据；无权限降级清楚；密码在任何持久化/错误渠道不可见。

## Task 14 — 文档、CI 及能力声明

**Files:** 新增 `docs/mongodb.md`；修改 `.github/workflows/ci.yml`、`README.md`、`docs/keybindings.md`、`docs/omni-bar.md`、`tests/docs.rs`、`src/cli.rs`、测试脚本及 Compose（Task 1 文件）。本计划不预设修改 pages：实施前若发现独立驱动列表或文档导航确需同步，先具体补充 scope 后修改。

1. 文档包含连接/默认库与 authSource、SRV/TLS/X.509、EJSON、Filter/Sort/Projection、控制台语法、CRUD/索引/事务、用户角色、MCP/CLI 示例、未知写结果与取消语义。区分不支持 JS、未实测的托管兼容产品、企业认证、特殊 BSON 编辑边界。
2. README 支持数据库名单、驱动数、功能介绍及 CLI capabilities 同步；不能显示“完整 Mongo”但隐藏事务/编辑未完成。MCP docs 描述新增工具和 write policy。
3. 新增 Mongo CI job 使用 Compose 或显式初始化 service：MongoDB 8 rs + standalone，init 完成再设 LAZYDB_REQUIRE_DATABASE_TESTS=1 执行全部 Mongo integration targets。日志不得 dump 环境凭据，测试超时有上限，失败保留非敏感服务诊断；CI 缺变量应失败而非 skip。
4. 保留现有数据库/发行 job，不借 Mongo 任务重构整个 workflow。MongoDB 7 扩展矩阵、TLS 专用证书/DNS fixture 可加入非阻塞补充 job，环境成本不合适时记录未验，不要求用户提供云账号。
5. 更新 docs contract tests 和帮助索引，核对所有文档命令/flags 与实际 CLI schema 一致，避免 SQL 参数继续用于 Mongo 示例。

**验证:** `cargo +1.94.0 test --locked --test docs --test commands --test driver_categories --test agent_mcp_protocol`；`docker compose -f docker-compose.mongodb.yml config --quiet`；`sh scripts/mongodb/wait-ready.sh`。CI 命令按 Task 15 Mongo targets 集中执行，不能省略 --require 对应环境变量。

**复核/验收:** 版本、能力、文档与实现一致；真实 Mongo CI 缺服务不会假绿；已有 CI 未被弱化。

## Task 15 — 最终自动验证与 Luna 收尾复核（单元 G）

**Files:** 对已实施范围修复必要问题；证据写任务目录 validation.md，进度摘要交插件 checkpoint，不修改插件 state。不要把补充检查失败转成反复 resume。

1. 从实际 diff 对照 change-scope.json：未列出的必要文件先更新范围并解释；删除/重命名若新增必须同时列两端。本计划未预设删除或重命名。只读参考的 SQL/Redis 模块若未改动不因读过就列入最终提交。
2. 核对全部 A–F 验收，并检查 URI/凭据、BSON、scope、partial/unknown write、cursor generation、session、workspace/history 迁移。没有实现的功能须完成；不可用外部服务是证据限制，与代码未完成区分。
3. **F 定向数据库合并运行（功能齐备后一次）:**

```sh
LAZYDB_REQUIRE_DATABASE_TESTS=1 cargo +1.94.0 test --locked --test mongodb_connection --test mongodb_catalog --test mongodb_cursor --test mongodb_console --test mongodb_mutation --test mongodb_conflicts --test mongodb_management --test mongodb_agent --test mongodb_mcp --test mongodb_transactions --test mongodb_monitoring --test mongodb_principals -- --test-threads=1
```

预期退出 0，无 required 环境 skip；rs 和 standalone URI 已在环境配置。若此前相同代码/环境的某 targets 已验证，仅补新增/变化 targets，CI 可以作为完整合并证据，不机械重复。

4. **P 项目现有门禁:**

```sh
cargo +1.94.0 fmt --all -- --check
cargo +1.94.0 clippy --all-targets --all-features -- -D warnings
cargo +1.94.0 test --all-targets --all-features
```

预期均退出 0；先相关测试再执行这些全量命令。macOS 按 CI 另运行 `cargo +1.94.0 build --locked` 与 `sh scripts/release/check-macos-dependencies.sh target/debug/lazydb`，记录环境；其他既有服务/发行 CI 由当前工作流验证，未运行项标明。

5. **F 可选驱动配置兼容:** `cargo +1.94.0 check --locked --no-default-features`，预期 Oracle feature 关闭时 Mongo 仍可编译。不重复正常配置 check，all-targets clippy 已编译。
6. **S 补充:** 可用 PTY 下检查 100×30/80×24 的键鼠、文档预览、确认、取消、退出；真实 TLS/SRV/X.509 测试账户/环境已可用则运行，不能为此向用户索要凭据或变成强制人工验收。失败最多针对性修复重试一次，之后记录命令/错误/覆盖替代证据并交 Luna 评估。静态 parser 测试不能标为真实 TLS/SRV 握手通过。
7. Luna 最终复核给出：实际改动、通过的 gates、未验及原因、剩余兼容限制。必要的普通错误自行修正再仅重跑相关项；普通未完成需求继续完成，不标 blocked。提交/合并只在相应后续阶段执行，不要求用户代为实现。

**完成判定:** A–F 有实现和对应自动化证据、G 的 P 门禁通过或准确报告环境限制且经收尾处理；无明文 secret、越权、截断误报、静默覆盖、未知结果伪报；原工作区用户内容和 index 未被改变；最终描述不夸大未实测平台/服务。

## 3. 构造器接线与范围完整性

ConnectionProfile 新字段会影响现有 literal，已实查需要机械接线：`src/db/mssql.rs`、`src/model/execution_target.rs`、`src/model/profile_manager.rs`、`src/persistence/profiles.rs`、`src/profile.rs`，以及 `tests/agent_context.rs`、`tests/agent_mcp.rs`、`tests/agent_policy.rs`、`tests/agent_selection.rs`、`tests/agent_service.rs`、`tests/catalog_editor_reducer.rs`、`tests/connection_switch.rs`、`tests/execution_target.rs`、`tests/global_workspace.rs`、`tests/keymap.rs`、`tests/profile_draft.rs`、`tests/profile_reducer.rs`、`tests/profile_runtime.rs`、`tests/profile_transactions.rs`、`tests/redis_browser_tabs.rs`、`tests/redis_loading_lifecycle.rs`、`tests/relation_runtime.rs`、`tests/relation_tabs.rs`、`tests/sqlserver_adapter.rs`。只补必要默认，不“顺便”改测试目的。

ConsoleDocument/ExecutionHistory 新字段也需补 `src/model/console_document.rs`、`src/model/sql_history.rs`、`src/persistence/sql_history.rs`、`src/runtime.rs`、`src/app.rs` 及现有 sql_history tests。WorkspaceTab 新变体的接线按编译器补齐已有 match，优先明确 Unsupported/None，不用大范围 wildcard 静默吞行为。无法预知细节仅以最小具体目录 `src/input`、`src/ui` 和 `tests` 等退路扩 scope 不可取；当前 scope 尽量列实际文件，后续确有新路径先具体增补。

本计划采用 analysis 的总体设计并提前测试环境；参数上限是工程默认，可在保持有界与证据前提下调整。任何本计划新定义的 API/字段名是实现契约草案而非现有代码事实；Luna 可为贴合现有惯例调整名称并同步测试与文档，不要求重新授权。

## 4. 参考与交接

相关源码定位与方案比较见 analysis.md；官方库版本与取消限制已有 analyze 调研证据，本轮不重复外部查询。实施 Mongo 3.x API 时按 @writing-plans 的小步验证，并使用 Context7/发布版本源码核对具体 API，不照搬旧版双参数示例。当前阶段未编译或连接数据库，不能引用计划命令为实际通过。

下一阶段第一个未完成单元为 A：从 Task 1 配置/驱动契约测试与隔离服务开始，完成 Tasks 1–3 后记录连接闭环，再继续 B。用户已选自动继续，禁止再询问“子 Agent 还是新会话”；本任务无子 Agent。本 plan 阶段完成后将控制权交插件，不越阶段开始实现。
