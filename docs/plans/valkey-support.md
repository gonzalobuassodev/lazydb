# Valkey Support Implementation Plan

> **执行者：Luna。** 按本自动工作流的任务检查点逐项实施，不启动子 Agent。分析与计划由 GPT-6.1 Sol 完成，其余实施、审查、纠偏、提交和集成由 Luna 完成。

**Goal:** 让 Valkey 成为可独立配置、持久化和识别的数据库，同时安全复用现有 Redis 键值浏览、变更和监控能力。

**Architecture:** 增加 `DatabaseKind::Valkey` 与应用级 valkey/valkeys URL；保留 `DatabaseConnection::Redis(RedisAdapter)` 作为共享协议族实现。Adapter 保存 configured kind，产品分派使用统一的 Redis-compatible 判断；INFO 实际身份独立于配置身份，不修改 profile UUID、目标 DB 或 generation。

**Tech Stack:** Rust 2024 / Rust 1.94、现有 redis 1.5.0 + Tokio/rustls、ratatui、serde/TOML、Docker Compose、GitHub Actions。原则上不新增依赖或修改 Cargo.lock。

---

## 0. 基线、权限和执行规则

- 工作空间：`/data/workspace/lazydb`；目标分支 main；起点 `80e118ad92ad101f14981cfc2a249cd50036e17f`。
- plan 阶段重新读取 HEAD 与 status：HEAD 仍等于起点，工作区干净。checkpoint.json 仍不存在。不依赖任何未提交业务文件，无须复制本地新行为。
- 本计划与 analysis.md、validation.md 仅保存于 `.git/opencode-tasks/ses_e9800e761ffeLRkMepTDhU8Il7/`；本阶段不建分支/worktree、不改业务代码、不改 index、不 stash、不改 state/checkpoint。
- 建议任务分支名：`feat/valkey-support`，仅供后续工作流自动命名使用；本阶段未创建。后续实施环境若创建独立 worktree，必须从指定起点开始，核对实际基线，不默认带入原工作区未提交文件。
- 本轮完成前写入 `change-scope.json`，最后仅写 `plan-c3552472-395e-4419-8e33-16ee6053037d.json`，token 为 `c3552472-395e-4419-8e33-16ee6053037d`、stage 为 plan；不得覆盖任何历史回执。
- 实施前确认 `CONTRIBUTING.md` 与实际 diff。按下面顺序持续完成三个闭环；不能把单个枚举、驱动列表或“复核完毕”当完整业务单元。
- 每个闭环结束记录当前代码版本/dirty 状态、命令、退出码、是否实际运行 live 服务到 validation.md。仅对相关改动重跑定向检查，最后一次统一全量验证。
- 分析环境 PATH 未找到 cargo/rustc。实施者先查现有 Rust 工具位置及项目工具链；最多一次有针对性的环境修复重试。无法运行时如实记录，不无限尝试或声称通过；不得以旧分析结果代替当前代码验证。

### 验证等级与门禁来源

1. **用户明确要求：** 增加 Valkey 支持；本阶段只制定计划、写指定任务产物、不改业务代码或 index。用户没有要求人工/PTY/截图验收，也没有指定 Valkey 版本矩阵或 live TLS 基础设施。
2. **项目既有强制门禁：** CONTRIBUTING.md 的 fmt、clippy `-D warnings`、all-targets/all-features test；CI 使用 Rust 1.94 与 locked 依赖。项目安全/架构约束包括秘密不泄漏、异步 owner/generation、终端文本清理、配置变更同步文档/默认文件等。Task 10 给出对应命令。
3. **本方案的功能自动化验收：** Tasks 1—8 的契约、reducer、INFO 与真实 Valkey 测试用于证明所实现功能，属于实施计划新增覆盖，不冒充用户另行指定或项目早已存在的门禁。新增 CI live 步骤使未来 CI 明确检查该功能。环境不能运行时，记录具体未覆盖项并由 Luna 收尾审查决定补充 CI 证据；不得把没有执行说成通过。
4. **补充建议、非必需门禁：** 人工 TUI/PTY、浏览器截图、live TLS（需现成可信测试 CA）、更多 Valkey 版本/平台矩阵、性能压测。无环境时可记录限制或不做，不自动转成阻塞；至少保留确定性 TLS scheme/配置映射测试。最多一次有针对性修复重试，之后收尾决定，不无限 progress。

以下单元的“验收”均指功能自动化契约，不要求额外人工签字。用户已选择自动工作流，按计划自动交接 Luna，不再询问执行方式。无预计删除或重命名，无未提交依赖；`change-scope.json` 列明全部预计修改/新增路径，条件性文件也包含在内。只有参考和原样运行的文件不列入；如果实施发现额外必要变更，应先说明理由并更新范围，不进行无关重构。

## 1. 确定的行为契约

| 项目 | 契约 |
| --- | --- |
| 持久化 kind | Redis 保持 redis；Valkey 使用 valkey；不做 serde alias、不批量改写旧 profile |
| 应用 URL | valkey:// → Valkey + Disable；valkeys:// → Valkey + Require；redis/rediss 始终推断 Redis |
| URL 格式 | 增加 Valkey、ValkeyTls，serde 依现有 kebab-case；只与 Valkey 兼容，不混入 Redis formats |
| 传输 URL | Adapter 仍生成 redis/rediss；不得将 valkey scheme 原样传给 redis::Client |
| 默认值 | localhost、6379、DB 0、无 schema、CatalogScope::All；DB 为规范化 u32，不限制到 15 |
| 类型切换 | Redis ↔ Valkey 保留 host/自定义 port/user/DB/只读/凭据以及 TLS，重新选择对应 plain/TLS format |
| SQL | KeyValue interaction；try_for_database_kind 返回 None；无 SQL catalog/principals/transaction UI |
| 身份 | DatabaseConnection::kind() 与 ServerInfo.kind 返回 configured kind；INFO 不重写配置身份 |
| 版本 | 观测 Valkey 时只用非空 valkey_version，缺失时 unknown；Redis 用 redis_version |
| 模式 | 非空 server_mode 优先，redis_mode 回退；未知字段保留、缺失容错 |
| 写入安全 | 复用现有 Lua 冲突/TTL 语义、只读保护、请求身份与 generation 检查，不扩大命令白名单 |
| 范围限制 | 单端点基础键值能力；不新增 Cluster/Sentinel、模块/专属类型、任意命令或数据迁移 |

## 2. 单元一：配置到键值工作区纵向闭环

**验收条件：** Valkey 可发现、新建、保存、重载和连接；成功事件触发逻辑 DB 发现，而非 SQL catalog；指定 DB 打开共享 RedisBrowser；profile/connection identity 保持 Valkey，Redis/MongoDB 行为不变。

### Task 1. 写契约与 URL 红测试

**Create:** `tests/valkey_contract.rs`。
**Modify/Test:** `tests/profile_url.rs`、`tests/driver_categories.rs`、`tests/connections_cli.rs`、`tests/database_capabilities.rs`、`tests/sql_dialect_mapping.rs`。

1. 在 valkey_contract.rs 写下最小契约（当前因缺少类型而编译失败是预期红灯）：

```rust
use lazydb::{
    db::capabilities::{InteractionModel, interaction_model},
    profile::{DatabaseKind, SslMode, parse_connection_url},
    sql::SqlDialect,
};

#[test]
fn valkey_is_a_key_value_database_with_stable_defaults() {
    let parsed = parse_connection_url("valkey://localhost").unwrap();
    assert_eq!(parsed.kind, DatabaseKind::Valkey);
    assert_eq!(parsed.port, Some(6379));
    assert_eq!(parsed.database.as_deref(), Some("0"));
    assert_eq!(parsed.ssl_mode, SslMode::Disable);
    assert_eq!(interaction_model(parsed.kind), InteractionModel::KeyValue);
    assert_eq!(SqlDialect::try_for_database_kind(parsed.kind), None);
}
```

2. 参照 profile_url.rs:198-241 的真实 API 增加 plain/TLS、ACL 百分号编码、密码不导出、前导零规范化、u32 边界、负数/非整数/溢出、未知 query 参数测试。明确 redis/rediss 解析仍为 Redis；Valkey formats 不接受 Redis kind，反之亦然。
3. 更新精确 registry 断言：11 个驱动；非关系型次序 Redis、Valkey、MongoDB；CLI JSON/文本包含 valkey。关系型数量仍为 8。
4. 运行红灯：`cargo +1.94.0 test --locked --test valkey_contract --test profile_url --test driver_categories`。记录实际错误，不把缺少编译器记为测试红灯。

### Task 2. 实现最小类型注册和全量编译分派

**Modify:** `src/profile.rs`、`src/db/descriptor.rs`、`src/db/capabilities.rs`、`src/db/mod.rs`、`src/db/redis/mod.rs`、`src/model/profile_manager.rs`、`src/model/execution_target.rs`、`src/sql/dialect.rs`、`src/db/column_type.rs`、`src/model/cell_editor.rs`、`src/model/relation_review.rs`、`src/ui/catalog_editor.rs`、`src/model/explorer_actions.rs`、`src/ui/icons.rs`、`src/ui/profiles.rs`、`src/app.rs`。

1. 增加 Valkey 类型和 formats；给 DatabaseKind 添加：

```rust
pub const fn is_redis_compatible(self) -> bool {
    matches!(self, Self::Redis | Self::Valkey)
}
```

2. 保持枚举 match 穷举：相同行为用 Redis | Valkey；条件正/反判断使用 helper，MongoDB 不进入该分支。注意 capabilities 的通配 fallback，不依赖编译器捕获此遗漏。
3. 将 profile.rs 的 scope、parse_server_url、TLS、默认端口、format/compatible_formats、URL 输出全部扩展。产品 URL 与 TLS 是一套完整状态，不能只加 parser scheme。
4. DRIVERS 插入 Valkey，DRIVER_NAMES 与 DRIVER_LIST 同步，更新 CLI 所依赖的精确输出断言。
5. RedisAdapter 保存 configured_kind；构造时先拒绝非 Redis-compatible profile，再访问 host/网络。公开 kind()，DatabaseConnection::Redis(adapter).kind() 委托它；connect 将两种 kind 都映射到现有 Redis variant。probe.kind 先改为 configured kind，实际版本解析留 Task 6。
6. 为剩余穷举分支添加 Valkey，使用不误导的类型名、URL 提示和 VK 图标 fallback，不新增 Cargo 依赖。
7. 运行 Task 1 定向测试以及 `--test database_capabilities --test sql_dialect_mapping --test connections_cli`；若多个 test target 命名以实际仓库为准，上述现有 target 已经路径核对。

### Task 3. 表单、类型切换与持久化闭环

**Modify:** `src/model/profile_manager.rs`、必要时 `src/persistence/profiles.rs`。
**Modify/Test:** `tests/profile_draft.rs`、`tests/profile_compatibility.rs`、`tests/profile_runtime.rs`。

1. 先增加 failing tests：ProfileDraft::new(Valkey) 默认值；validate 后 kind/format/scope；保存/加载 kind 不变；从保存 profile 编辑 URL 也不改变 kind；凭据既有策略和只读保留。
2. 用 `ProfileManagerState::select_driver` 测 Redis ↔ Valkey，分别从 TLS 与 plain 起步。断言 TLS 保留、format 同步、URL 重解析不降级、不泄漏密码、自定义端口不重置。
3. 扩展 profile_manager.rs 的 Redis 默认值、REDIS_FIELDS、验证、scope、SSL 控件、默认端口；set_kind 当前无条件 default_for，需对两种协议族之间的切换保留 TLS 对应 format。其他跨类型切换保持原行为。
4. 检查所有 ConnectionUrlFormat 处理位置，不能假定只搜 DatabaseKind 足够。apply_parsed_url 保持 URL 唯一推断，不添加上下文魔法。
5. 持久化兼容归一化应使用新增 formats；不预先升级 schema 版本。测试新 Valkey round-trip、现有 Redis round-trip、未知类型 unavailable 行为。
6. 运行：`cargo +1.94.0 test --locked --test profile_draft --test profile_compatibility --test profile_runtime --test profile_url --test driver_categories`。

### Task 4. App/Runtime/Explorer/Browser 行为与所有权

**Create/Test:** `tests/valkey_workspace.rs`。
**Modify:** `src/app.rs`、`src/runtime.rs`、`src/model/explorer.rs`、`src/ui/mod.rs`；必要时扩展既有 Redis reducer 测试。
**Regression:** `tests/redis_explorer.rs`、`tests/redis_browser_tabs.rs`、`tests/redis_loading_lifecycle.rs`、`tests/redis_unsaved_changes.rs`。

1. 按 redis_browser_tabs.rs 现有 ConnectionSucceeded/ExecutionTarget 测试 helper 组织用例，不直接伪造 ActiveConnection 来绕过 App::update。
2. 红测试：Valkey profile connect success → DiscoverRedisDatabases 存在、LoadCatalogPage 不存在；DB rows 排序 0/2/10；OpenRedisDatabase(profile, 2) → 指定 RedisBrowser target；不出现 SQL principals。
3. 增加跨 Redis/Valkey 两 profile 用例：切换 Explorer 或当前 tab 后结果仍归原 owner；旧 generation 的扫描/预览/变更结果被忽略；只读 Valkey 不能发 ExecuteRedisMutation；工作区恢复后 browser 正确关联 Valkey profile。
4. 审计 app.rs 中 `== Redis`、`!= Redis`、`Some(Redis | MongoDb)` 等全部分支；尤其 13819-13889 连接后分派、23625 principals 排除。同步 Runtime 连接后初始化、Explorer 投影及搜索/可见性逻辑。
5. ui/mod.rs 的 RedisBrowser 图标从 `tab.target.profile_id` 查 profile.kind，而不是固定 Redis或查当前选中连接。配置缺失时安全降级，不能错误使用其他 profile。
6. 运行：`cargo +1.94.0 test --locked --test valkey_workspace --test valkey_contract --test redis_explorer --test redis_browser_tabs --test redis_loading_lifecycle --test redis_unsaved_changes`。
7. 执行 `rg -n 'DatabaseKind::Redis|ConnectionUrlFormat::Redis' src tests`，为保留的每个独占 Redis 分支核对原因；不能机械替换测试中本来测试 Redis 的断言。
8. 首个闭环自检并记录实际证据；只有完整满足本单元验收才提交逻辑提交 `feat(valkey): add profiles and shared key-value workspace`。按实际文件选择性暂存，禁止 git add .；不得包含任务目录或用户改动。

## 3. 单元二：实际身份与 Dashboard 闭环

**验收条件：** Valkey 显示自身实际版本和模式；INFO 权限失败时连接仍可用；configured kind 始终不变；Redis 的既有字段和指标不回归。

### Task 5. INFO 身份解析红测试

**Create:** `src/db/redis/server_info.rs`（共享解析，名称可依邻近模块调整但避免双份逻辑）。
**Modify/Test:** `src/db/redis/monitor.rs` 内测试；新解析模块内测试。

1. 先用以下输入表驱动测试预期的品牌、版本、模式，不以只 contains 某个字符串作为完整验证：

| 输入关键字段 | 预期 |
| --- | --- |
| redis_version:7.4.2 / redis_mode:standalone | Redis / 7.4.2 / standalone |
| redis_version:7.2.4 / server_name:valkey / valkey_version:8.1.3 / server_mode:standalone | Valkey / 8.1.3 / standalone |
| server_name:valkey / redis_version:7.2.4 | Valkey / unknown，不得 7.2.4 |
| valkey_version:8.1.3，无 server_name | Valkey / 8.1.3 |
| server_name:valkey / valkey_version:空白 | Valkey / unknown |
| 无 INFO 或未知字段/CRLF | 可解析的字段保留；缺失项 None/unknown，无 panic |

2. 额外断言 server_mode 优先于 redis_mode，未知 field 仍在原 monitor.fields 中；既有数值/缺失/零值测试保持通过。

### Task 6. probe、monitor 与 UI 使用共享身份

**Modify:** `src/db/redis/mod.rs`、`src/db/redis/monitor.rs`、`src/ui/redis_dashboard.rs`、必要时 `src/model/dashboard.rs`、`src/ui/mod.rs`。

1. 用一个共享解析实现返回观测身份，probe 与 monitor 消费相同逻辑；不强行删除旧 redis_version/redis_mode 字段以免引入无关公共 API 破坏，新增明确的实际身份字段或 accessor。
2. probe 的 INFO 请求保留 `.ok()` 级别可降级特性，PING 成功不因 INFO NOPERM 被推翻；ServerInfo.kind 用 adapter.kind()。已配置 Valkey但缺少实际版本时应返回 unknown，不误用兼容版本。
3. Dashboard 标题用绑定 profile.kind，摘要用观测到的品牌/实际版本；尚无 INFO 时用配置品牌与 waiting 状态。配置为 Redis但 endpoint 是 Valkey时，摘要可显示 Valkey，不能自动改配置。
4. 为 Dashboard 渲染增加确定性 ratatui 单元测试，覆盖 Valkey 摘要与标题、Redis 摘要、无 INFO、不活跃 profile 的 tab；内部私有 render 可在对应模块测试，不为测试无必要公开 API。
5. 用户可见诊断采用实际品牌或中性 key-value/Redis-compatible 文字；discovery partial 警告不能把 Valkey误写成只支持 Redis。技术命名 Redis* 保留。
6. 运行：`cargo +1.94.0 test --locked --lib db::redis`；`cargo +1.94.0 test --locked --lib ui::redis_dashboard`；加跑单元一的 Valkey contract/workspace。测试 filter 零匹配不算通过，确认测试计数。
7. 验收并选择性提交 `fix(valkey): report actual server identity and dashboard version`。

## 4. 单元三：真实服务、安全边界与交付证据

**验收条件：** 真实 Valkey 产品入口、读写/TTL/冲突/权限和共享 Redis 回归均有当前证据；CI显式运行live测试而不静默跳过；文档与实际验证版本一致。

### Task 7. 本地 fixture 与 live 产品入口

**Create:** `docker-compose.valkey.yml`、`tests/valkey_adapter.rs`。

1. fixture 参照 docker-compose.redis.yml：官方 `valkey/valkey:8.1-alpine`、宿主 `127.0.0.1:6380:6379`、独立 data volume、valkey-server --appendonly yes、valkey-cli ping healthcheck；不共享 Redis data volume。实施时记录拉取镜像 digest；tag 不可用时选择明确可用的 8.x 版本，同步 CI/文档，不改用 latest。
2. 新 live tests 使用 `#[ignore = "requires an isolated Valkey server"]` 和 `LAZYDB_TEST_VALKEY_URL`；被显式运行却缺 URL时 expect 失败，不返回成功。无条件运行的静态 Valkey 测试放 contract/workspace，避免被一起忽略。
3. 产品连接必须 import valkey URL → DatabaseConnection::connect(profile, transient_password.as_ref())。seed/cleanup 的 redis::Client 使用单独的 redis/rediss 传输 URL；仅替换 scheme，保留端口、DB 和编码凭据，不打印带密码 URL。
4. 运行 fixture 检查和启动：

```bash
docker compose -f docker-compose.valkey.yml config --quiet
docker compose -f docker-compose.valkey.yml up -d --wait
LAZYDB_TEST_VALKEY_URL='valkey://127.0.0.1:6380/0' cargo +1.94.0 test --locked --test valkey_adapter -- --ignored --nocapture --test-threads=1
```

5. Live 用例覆盖 connect/kind/probe 实际版本、SCAN、DB0/DB2 隔离、CONFIG/INFO discovery、六种基础类型与分页及二进制值、INFO 指标、创建/定向编辑/TTL保留与过期设置/expected冲突/删除。优先通过产品连接的共享 adapter 操作，不仅用 raw redis::Client 自测。
6. key 使用 UUID 前缀；只清理本测试 key，禁止 FLUSHDB/FLUSHALL。故障时尽可能清理，service teardown 为最后隔离保证。

### Task 8. 认证、ACL 与传输边界

**Modify/Test:** `tests/valkey_adapter.rs`；必要时追加 `tests/fixtures/valkey/` 配置。

1. 在隔离 fixture 创建唯一 ACL 用户并连接：正确密码成功，错误密码为 Authentication；配置用户名/密码含需编码字符，秘密不进入 Debug/导出 URL。
2. 禁止 CONFIG 时 discovery 返回 Partial 并保留当前 DB；禁 INFO 时 probe unknown但 PING与允许的读写仍可用。cleanup 删除测试 ACL 用户，禁止改 default 用户权限。
3. TLS URL/表单到内部 rediss 的映射必须有确定性单测（提取私有纯构造 helper 而非公开额外API）。如果本地有可信测试 CA，则加 isolated TLS live测试；无此环境时记录 live TLS 未覆盖，不宣称实测成功。新增自定义 CA 设置不在本任务范围。
4. 只读保护按产品层 reducer/runtime测试覆盖；不要误把 RedisAdapter 自身是否拦截任意 mutation 当成本来存在的产品权限边界。
5. 显式运行共享 Lua 回归：

```bash
LAZYDB_TEST_REDIS_URL='redis://127.0.0.1:6380/0' cargo +1.94.0 test --locked --test redis_mutation -- --ignored --nocapture --test-threads=1
```

6. 同样在独立 Redis fixture 上运行原 Redis mutation 检查，避免修复 Valkey时破坏 Redis。各服务使用各自端口；不更改开发者已有服务。

### Task 9. CI 与文档

**Modify:** `.github/workflows/ci.yml`、`README.md`、`docs/architecture.md`、`docs/redis-browser.md`、`CONTRIBUTING.md`。

1. CI databases job 增加独立 Valkey service 及 6380 端口/healthcheck，设置 LAZYDB_TEST_VALKEY_URL，显式运行 Task 7 live target和共享mutation回归。保留现有Redis服务与步骤，不把它替换成Valkey。
2. CI命令加已有风格的 `timeout 10m` 和 `--test-threads=1`；live测试没有URL应失败。检查脚本不能误将未执行 ignored tests 记成成功。
3. README 增加 Valkey 支持矩阵及限制，说明 valkey/valkeys 是 LazyDB URL；复制 redis endpoint后可选择 Valkey，使用现有凭据存储；列出实际验证版本，不宣称任意新类型、Cluster/Sentinel。
4. 架构说明共享 Redis variant 内保存 configured kind，browser/dashboard复用；redis-browser文档说明适用于 Redis与Valkey，内部名不改。CONTRIBUTING给出 fixture、显式live命令与只清理隔离测试资源的方法。
5. 不新增用户配置默认项，因此无需机械修改 config/default.toml；若实施另有配置新增则必须同步它和配置测试。

### Task 10. 收尾验证、审查与提交

1. 先审实际 diff：按 `git diff --check`、全部 Redis/format判断、硬编码品牌、异步owner、只读/TTL/冲突、凭据日志逐项核对；禁止只依赖穷举编译。
2. 完整功能齐备后统一运行项目强制检查：

```bash
cargo +1.94.0 fmt --all -- --check
cargo +1.94.0 clippy --locked --all-targets --all-features -- -D warnings
cargo +1.94.0 test --locked --all-targets --all-features
```

3. 全量测试不替代 ignored live命令。记录 Rust版本、平台、实际镜像digest、HEAD/dirty diff、每个命令退出码及关键测试计数。若代码/环境后续变更，只重跑受影响检查；最后证据必须对应最终代码。
4. 环境受限检查区分强制验证与补充证据；PTY/screenshots为补充，不要为它反复卡住。强制检查未能执行时在收尾清楚列出缺口和CI安排，不伪称整体通过。
5. 选择性提交 `test(valkey): verify live compatibility and document support`（若体量需要可拆测试/文档提交），不提交任务元数据；按实际启用的 fixture做 teardown，不停止用户原服务、不宽泛删除卷。
6. Luna完成审查/纠偏及工作流规定的提交合并，本 plan阶段不执行这些动作，也不替用户操作远端。

## 5. 完成清单与交接

- [x] 单元一：11 driver registry、Valkey URL/表单/持久化、shared adapter kind、DB发现/browser、非SQL分派、会话所有权，定向测试通过。
- [x] 单元二：INFO实际版本与模式、configured/observed身份独立、Dashboard/图标/诊断正确，Redis回归通过。
- [x] 单元三：真实Valkey读写/ACL/TTL/冲突/DB隔离、共享Lua回归、CI显式live、文档完成；全量 all-target/all-feature tests 由 CI run 40101801030 在 Ubuntu/macOS 通过。

三个实施单元已完成，验证与提交/集成记录见任务目录中的 implementation.md 和 validation.md。功能提交为 `07a5c8d`，CLI capabilities 测试期望修正为 `f309e5e`；已 fast-forward 集成到本地 main，未推送 main。此计划文件作为额外仓库文档单独纳入 integrate 阶段提交。
