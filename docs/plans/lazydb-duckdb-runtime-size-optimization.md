# LazyDB 客户端体积优化 Implementation Plan

> 执行交接：Luna。用户已经选择自动工作流，本计划不要求重新选择执行方式、不启动子 Agent。仅当前 plan 阶段准备文件、步骤和验证；业务实现、审查、纠偏及提交合并由后续 Luna 完成。技能模板中的 Claude/executing-plans/频繁提交等不覆盖用户当前分工和权限。

**Goal:** 默认安装包不编译、不携带、不强制链接 DuckDB 引擎；使用 DuckDB 时按需加载用户提供的可信兼容运行库，保留现有本地文件、会话、事务、取消及数据类型能力；同时精简更新解压依赖、优化压缩、提供安全显式旧版本清理和可靠体积报告。

**Architecture:** 保留 duckdb-rs=1.10506.0 高层接口和现有适配器，用本地固定版本 libduckdb-sys 兼容包生成类型精确的运行时函数表，完整初始化后进程级持有原生库。更新解包按编译平台裁剪；旧版本清理在安装互斥协议和可验证ownership边界内进行，默认只预览。

**Tech Stack:** Rust1.94.0、Tokio、libloading、syn/quote build-time生成、DuckDB1.5.6 C ABI、现有SQLx/Tiberius/Redis/Mongo/reqwest及CLI/MCP/LSP、Python/shell/PowerShell发布环境。

---

## 1. 基线、授权与交付约束

完整读取同任务目录analysis.md。当前HEAD=80e118ad92ad101f14981cfc2a249cd50036e17f，main工作区和index均干净；checkpoint.json已存在，本次读取stage=plan、round=4、codeChanged=false，仅只读参考，不修改它。任务文档目录在.git下，不是业务提交内容。没有未提交业务依赖需要复制到worktree。机器本地被忽略的.cargo/config.toml不作为产品行为或补丁；后续明确CARGO_TARGET_DIR，不能提交个人绝对路径。已有临时调查库/源码仅为历史证据，不复制为生产补丁或CI唯一fixture。

后续每次恢复先看真实checkpoint/status/diff和单元结果，不重复全读日志。普通编译错误、未知实现细节和单元未做完不算阻塞。没有新增事实不要重跑同一套全量验证。只在worktree按工作流实施，不stash、清空或覆盖原工作区，不自行tag/push/发布。若实际发现新依赖文件或改名，先更新change-scope.json，改名列两端。

用户client-first理念解释为默认包不强制携带重量级数据库运行时，不意味着删除SQLite历史存储、废弃本地DuckDB或必须改远程。运行库仍可在客户端进程中执行；文档坦诚这一点。远程Quack、独立helper、通用driver插件、自动下载引擎不在本任务实现范围。系统SQLite、TLS迁移、剪贴板fork不实施，保留分析中的兼容成本说明。

本次根据用户要求补齐任务文档。历史“本轮只写回执”阻塞源于对历史消息的错误引用，当前有效指令不存在该限制。计划正文和精确范围已分别保存到plan.md、change-scope.json；validation.md记录本次实际文档检查。历史回执保留，不将其中的错误权限判断继续作为执行前提。

## 2. U/P/A/S及超时约定

U：用户要求默认客户端不自带DuckDB引擎，完整计划并实施；阶段权限/工作区保护；Rust原生编译和验证每条最多600秒，其他每条最多120秒，不用timeout0。
P：已有ci.yml fmt/clippy/test、macOS build+system-only依赖检查、distribution contracts和既有真实driver jobs；release五native目标构建及artifact验证。保留这些功能门禁，可拆预编译和用例组满足600秒限制，不把旧180分钟job或15分钟Mongo整组当本次命令授权。
A：新增loader无库/有库、ABI/并发/失败重试/句柄生命周期、无引擎启动依赖和archive无payload、profile兼容、事务/取消/复杂类型、ZIP路径安全、prune所有权/锁/并发/幂等及最终同环境体积对照。五平台行为需nativeCI证据，不可都标成optional。
S：PTY人工TUI、大库长期性能观察等补充证据；required:false。环境受限只做一次针对修复重试，然后交收尾审查决定记录限制或补充，不无限progress。

验证统一采用scripts/ci/run-with-timeout.py，接口固定为python3 scripts/ci/run-with-timeout.py SECONDS -- PROGRAM ARG...，Rust用600，其他120，超时返回124并杀掉本次进程组/Windows任务树，不能杀无关Cargo。先测试wrapper的正常退出/非零透传/超时和子进程清理；CI不省略timeout。若大冷编译超时，先查锁、进程、最近代码变动及缓存，拆预编译/增量组，不立即重复同一挂起命令。

验证记录写任务validation.md：SHA+dirty状态、target、toolchain/SDK、runtime路径来源与版本、命令、退出码、live/skipped。旧6386593实验32.21%等仍是旧证据，不冒充最终80e118a或本实现通过。

## 3. 固定设计合同

### 3.1 Runtime crate与生成

根Cargo增加[patch.crates-io] libduckdb-sys={path="crates/libduckdb-sys-runtime"}，直接依赖同版本sys以调用新增加载API；duckdb依赖仅chrono，去掉bundled/json/parquet（后二者也强制bundled）。新增crate package名libduckdb-sys，version1.10506.0，保留lib名libduckdb_sys和MIT来源，links=duckdb仅作Cargo唯一性标识，build.rs绝不输出rustc-link-lib或构建C++。不复制duckdb.tar.gz、C++源或上游自动下载器。

该crate保持duckdb-rs本次依赖会请求的兼容feature default/pkg-config/vcpkg为空，不声明能重新引入bundled的可执行feature。生成器对unsupported link attributes、variadic、非预期foreign item显式报编译错误；不能默默drop ABI。build.rs通过syn AST读取upstream/bindgen_bundled_version.rs，将types/constants原样生成到OUT_DIR，foreign fn生成unsafe Rust代理和unsafe extern C typed指针，不靠regex解析签名、不对任意指针套通用transmute、不改变pub API或结构体repr。原始pinned文件有546个C函数，完整表全量校验，生成快照/函数计数可复现。arrow_c_data/string/error从同pinned来源带许可证和checksum，duckdb-rs继续处理Row/Arrow和复杂值，无全量手写C API重写。

libloading作为必要小依赖纳入锁，不升级其他库。成功Library和所有typed pointer保留在进程级OnceLock，不卸载；加载失败不填OnceLock。初始化Mutex串行化；先读version，再验证全部symbols，最后一次发布。支持v1.5.6或1.5.6精确版本，拒绝其他版本，错误返回stable code。版本函数是用户明确信任库的一次原生调用；不声称能sandbox恶意库。

失败后修复可重试；成功后显式路径canonical到另一库返回restart_required，不切换allocator/free/Arrow回调。无显式新配置时复用已加载实例。并发中失败候选不能污染成功表；loader返回前Config::default不得执行。代理未初始化是内部不变量panic，不作为缺库用户错误路径；所有应用生产原生构造之前统一ensure_loaded并测试。

### 3.2 发现路径与诊断

入口固定LAZYDB_DUCKDB_LIB_DIR，非空值必须是绝对目录，文件名macOS libduckdb.dylib/Linux libduckdb.so/Windows duckdb.dll。显式配置失败立即报错，不回落系统库。无显式配置时先AppPaths.data_dir/drivers/duckdb/1.5.6，再固定可信绝对系统目录（macOS两Homebrew目录；Linux/usr/lib、/usr/lib64及受支持架构multiarch目录；Windows只显式/应用数据目录）。不扫描CWD、普通PATH、不依赖LD_LIBRARY_PATH或机器绝对rpath。目录/最终库canonical，系统包管理器symlink可解析；配置路径与实际库路径用于诊断但不能带凭据。Windows加载依赖使用安全LoadLibraryEx语义，只查本库目录和系统依赖，不默认当前目录。

默认version/capabilities/doctor不加载库；supported drivers仍有duckdb，缺库不是unsupported profile。doctor可追加非执行性optional提示，不以runtime缺失改变原ok语义。数据库错误码建议duckdb_runtime_missing/invalid_path/load_failed/version_mismatch/missing_symbol/restart_required；说明需要1.5.6 C API共享库以及环境变量。库缺失不创建目录/数据库文件。profile保存导出、旧workspace恢复无需runtime。

JSON/Parquet从用户运行库提供，固定CI库必须拥有它们；官方库如缺扩展应给真实引擎错误，不能输出空支持或静默联网安装。不为了smoke放宽Agent策略。

### 3.3 清理接口、所有权和运行保护

为最小兼容修改，固定CLI为lazydb update --prune [--dry-run|--yes] [--json]；不在现有UpdateArgs中嵌套子命令。--prune与--check/--channel/--allow-downgrade冲突；--dry-run/--yes只在--prune下有效且互斥。不带yes默认预览，返回独立schema1 report，保留旧update JSON契约。禁止隐式网络检查。main原分发保持不变，由update::run在启动progress/network前路由。

只对安装状态与launcher current链一致的native根生效。引入每release的.release.json：schema/product/version/target/binary_sha256/previous_version和已验证文件清单；在installer/archive校验通过后的staging写入，再激活。current+previous实际激活链保留，不以目录SemVer最大值猜回滚版本；标记版本需与父目录/target一致。legacy无ownership记录目录预览标unknown并不删除，也不执行未知binary探测。markers和binary hash不能授权删除额外未知文件；目录存在unexpected files或任何内部symlink一律跳过。配置、sql、history、drivers、锁永不候选。首次upgrade没有trusted previous marker时legacy previous仍保留。

prune只显式--yes执行；默认不自动retention。与shell installer共用根.install.lock的mkdir+pid协议，活跃/未知pid不抢锁、不递归清除未知锁；Rust updater也参加。shell现有stale恢复中空pid抢锁竞态要修成保守busy，不为了旧行为继续危险删除。锁持有到写marker/切current完成。执行前复读current/state/previous和候选identity，候选目录atomic rename到同根的唯一.quarantine名称后再次验证，不跟随链接删除；异常保留隔离目录并报告partial。只删除名单中已核验普通文件和空目录，避免递归remove_dir_all遍历未知内容。

保护current_exe所在release及其他可确认运行版本。计划不新增全应用生命周期租约（避免扩大无关入口）：Unix受控进程引用探测可用且结果完整才执行清理，lsof缺失/权限不足/未知状态保守跳过；Windows没有可确认支持时prune报告unsupported或unknown且不删除。提供纯FakeProcessProbe测试，不把PID文件即真实在用的唯一证明。文档说明这是显式、保守磁盘清理，不能保证删除所有legacy目录。不得实际清理用户安装来验收。

## 4. 实施单元

### T0：固定输入、timeout和runtime测试准备

Files：新增scripts/ci/run-with-timeout.py、test-run-with-timeout.py、prepare-duckdb-runtime.py、duckdb-runtime-assets.json、test-prepare-duckdb-runtime.py；参考analysis.md，不修改业务。

Steps：1) Luna首读checkpoint/status/diff，固定80e118a和explicit target-dir，记录未提交依赖；2) timeout wrapper先写最小子进程树测试并实现，120秒内测试；3) downloader输入--target/--output-dir/--print-env，五target映射固定官方1.5.6URL、SHA256和目标文件，读取GitHub官方元数据取得真实digest后提交固定值，不能填placeholder；4) 用HTTP fixture验证hash错/路径穿越/重名/symlink/错误架构/网络失败/缓存污染，先验证整个archive再只提取库，临时文件原子发布；5) 用户生产安装器不调用该脚本，只有测试准备；6) 同机baseline只读构建和packaging按600秒限制分步记录，不把6386593包当80e118a实测。

Commands：python3 scripts/ci/test-run-with-timeout.py；python3 scripts/ci/run-with-timeout.py 120 -- python3 scripts/ci/test-prepare-duckdb-runtime.py。后续真实prepare每次120秒内；超时断点检查cache不无限下载。

Review：库artifact全部有确切来源/checksum；macOSuniversal只作测试runtime不进main archive；缓存目录非原工作区用户数据。
Acceptance：固定fixtures和有界命令可用于所有后续单元，不依赖旧临时目录、用户机器库或手动装库。

### T1：sys绑定可生成、无引擎可构建

Files：Cargo.toml/Cargo.lock；新增crates/libduckdb-sys-runtime下changeScope列出的Cargo/build/upstream/src/license/UPSTREAM、tests/bindings_generation.rs。

Steps：1) root设workspace包含兼容crate，default-members保持root，正式门禁增加--workspace覆盖；2) vendored ABI文本与helpers附MIT、来源版本、checksum；3) build.rs只syn AST生成OUT_DIR bindings/functions/table；4) feature默认空兼容，删除native链接/cc/cmake/download依赖；5) generator测试type常量和546指针签名、未知Item报错、确定性输出，不能把546大小当唯一正确断言；6) 通过完整根cargo check验证duckdb-rs所有被引用API，检查cargo tree无bundled。

Commands（均wrapper600）：cargo +1.94.0 test --locked -p libduckdb-sys --test bindings_generation；cargo +1.94.0 check --locked --bin lazydb；cargo +1.94.0 tree --locked --target aarch64-apple-darwin -e normal。

Review：复用公开wrapper实现，不复制引擎源码、不维护第二套Row解码；private helpers版权无遗漏；新workspace不意外构建非目标crate。
Acceptance：没有安装DuckDB即可构建正常客户端；生成代码无foreign强制链接，未来Cargo all-features不会重新编译C++引擎。T1只为构建闭环，还不能宣称缺库用户路径完成。

### T2：缺库启动→连接错误→加载修复→真实查询

Files：runtime crate src/runtime.rs、tests/runtime_loading.rs、tests/fixtures/runtime_stub.rs；src/db/duckdb_runtime.rs、src/db/mod.rs、src/db/duckdb.rs、src/cli.rs；tests/duckdb_runtime.rs。

Steps：1) subprocess测试环境显式缺库目录、隔离LAZYDB_CONFIG_HOME，version/capabilities/doctor/SQLite都正常，保存DuckDb profile后查询返回missing且文件不生成；2) 实现固定发现顺序、loader typed表/OnceLock/Mutex/成功Library永久持有、错误诊断；3) ensure_loaded位于任何Config::default之前，可spawn_blocking包住加载避免主async阻塞；4) bad path、错误架构、version错、缺symbol、并发初始化和失败重试均独立process或局部Loader实例，不在parallel测试内unsafe修改全局环境；5) 使用编译出的stub导出546个函数但仅执行version，缺symbol stub故意漏末项，side effect sentinel证明基础命令未加载库；6) 真实1.5.6 SELECT、JSON、Parquet、文件重开、free/drop后新连接；7) 成功后路径变更拒绝且旧连接仍工作。

Commands：wrapper600 cargo +1.94.0 test --locked -p libduckdb-sys --test runtime_loading；wrapper600 cargo +1.94.0 test --locked --test duckdb_runtime。

Review：不存在C回调跨ABI unwind；所有函数地址来自同一Library、全表发布前不暴露部分状态；路径错不回落；Windows安全依赖搜索；缺库无panic。
Acceptance：无runtime仍正常用客户端，只有DuckDb连接需要库；错误可恢复且runtime选择不会混用allocator；真实库输出与旧语义一致。

### T3：现有完整DuckDb能力和profile兼容

Files：tests/duckdb_adapter.rs、profile_compatibility.rs、profile_draft.rs、connection_switch.rs、agent_service.rs、agent_mcp.rs、lsp_catalog.rs、lsp_protocol.rs；必要窄修src/db/duckdb.rs。不预授权重写src/runtime.rs会话实现。

Steps：1) 为含原生DuckDb用例的测试命令注入T0准备runtime，现有测试不能return/skipped作为成功；2) 已有fixture全部经DuckDbAdapter connect，不在主启动初始化；3) 逐组文件/内存/只读缺文件/范围/attached+temp/session克隆/MANUALcommit rollback/取消与下一条query/关闭清理；4) 参数绑定、nested/UHUGEINT/Decimal38/TIME_NS、零行列metadata/多语句/预算和DMLRETURNING；5) Agent/MCP写策略仍保守，Parquet写测用适配器不放开Agent，LSP纯词法无库也可运行；6) 缺库profile不被当invalid而丢弃，连接失败不影响第二driver。

Commands（wrapper600，预编译和分组执行必要时拆分）：cargo +1.94.0 test --locked --test duckdb_adapter；cargo +1.94.0 test --locked --test profile_compatibility --test profile_draft --test connection_switch；cargo +1.94.0 test --locked --test agent_service --test agent_mcp；cargo +1.94.0 test --locked --test lsp_catalog --test lsp_protocol。

Review：不能以禁用能力/字符串降级/新skip换绿；原生句柄在后台cancel时仍保持Library。
Acceptance：现有功能闭环完整，缺库/有库两个模式均有证据。若出现原有与本任务无关失败，记录baseline复现而非乱改。

### T4：ZIP按平台剪裁和XZ9兼容

Files：Cargo.toml/Cargo.lock、src/update.rs、新src/update/archive.rs、scripts/release/package-archive.py、test-archive.sh、test-distribution.sh、.github/workflows/release.yml、tests/update_native.rs和update_http.rs所需Args兼容。

Steps：1) extracted archive module保留验证root/duplicates/entrytype/path边界；Windows生产zip只Stored/Deflate后端，Unix生产tar/xz2，tests保留两类dev依赖和cross-formatcontracts；2) Windows7z显式-m m=Deflate的正确参数以本地/CI实际CLI验证，避免复制错误命令；3) preset=9普通LZMA2，不改USTAR/兼容目录尾slash、文件名和manifest；4) 对旧静态xz2生成consumer fixture或固定最旧支持依赖测试，新archive解压hash等于source；5) normal-target tree证明生产无ZIP多codec，测试依赖树不混淆；6) Stored/Deflate两种Windows历史包、恶意路径/重复/伪目录/links保留安全回归。

Commands：wrapper120 sh scripts/release/test-archive.sh；wrapper600 cargo +1.94.0 test --locked --lib update::；wrapper600 cargo +1.94.0 test --locked --test update_native --test update_http；wrapper120 sh scripts/release/test-distribution.sh。

Review：featureunion可重新打开zip默认功能，检查root/dev/build分离；Windows错误路径仍输出稳定错误；XZ9内存成本文档说明。
Acceptance：正常各平台更新仍可用、旧decoder可读，缩小幅度真实记录；不启BCJ、不删安全测试。

### T5：原生release ownership和互斥协议

Files：src/update.rs、新src/update/install_lock.rs、src/update/release_ownership.rs；install.sh、pages/install-core.sh；scripts/release/test-installer.sh、test-pages.sh；tests/update_native.rs。

Steps：1) 实现与shell同根.install.lock mkdir+pidguard，5秒上限，缺pid/不明状态busy，不删除无主未知目录，guard只移除自己nonce匹配锁；2) shell也写owner nonce并仅清理自身锁，保留老pid锁busy兼容，修空pid抢锁竞态；3) Rustupdater在真实安装变更前获得相同锁，重读state/current，退出清理不覆盖别人锁；4) 已验证staging在激活前写.release.json，binary_sha256和filemanifest来自验证后的普通文件，记录previous actual version，fail-safe atomic写；5) shell与Rust共享schema逻辑/一致fixture，设置current后保留previous目录不清理；6) legacy marker缺失不补造未知ownership，升级可继续。

Commands：wrapper600 cargo +1.94.0 test --locked --test update_native；wrapper600 cargo +1.94.0 test --locked --lib update::；wrapper120 sh scripts/release/test-installer.sh；wrapper120 sh scripts/release/test-pages.sh。

Review：锁协议和旧installer可互斥，不能拿profile.lock文件锁代替mkdir锁；marker不随正常GitHubarchive进入旧消费者layout，应用只本地生成；联网不持锁无限等待。
Acceptance：installer/updater不能同时改current，标记可靠且失败不破坏既有激活/回滚。

### T6：显式旧版本清理闭环

Files：src/cli.rs、src/update.rs、src/update/prune.rs、tests/update_prune.rs、tests/update_native.rs；复用T5模块。不修改src/main.rs分发。

Steps：1) CLIparse测试update旧选项保持、新--prune互斥/defaultpreview，新增Args字段的所有struct literals按compiler最小补齐；2) report固定schema1含mode/current/previous/kept/candidates/skipped/reclaimed_bytes/errors，不复用旧update状态集合；3) 仅native根，扫描releases直属目录，ownership/target/hash/manifest/identity都验证，unknown/legacy/extra files/links记录保留；4) active ProcessProbe默认保守，受控Unixlsof通过程序参数而非shell拼接，2秒/探测上限，缺工具/权限则跳过；5) --yes同锁内重读，current/previous/current_exe绝不候选，rename quarantine后revalidate名单，精确删除、partial结果和再运行幂等；6) fixture测试锁争用/current切换/nonce替换/伪semver/symlink root/内部links/unknown内容/文件损坏/IO错误、Windowsunsupported；7) 全部操作tempfixture，不跑真实用户installation prune。

Commands：wrapper600 cargo +1.94.0 test --locked --test update_prune；wrapper600 cargo +1.94.0 test --locked --test update_native；wrapper600 cargo +1.94.0 test --locked --lib cli::tests。

Review：SemVer只名称不是所有权；不执行未知candidate binary；preview即使创建短暂锁也不改release内容；报告保守限制，不承诺清理所有旧目录。
Acceptance：preview→--yes→secondrun端到端可证，候选恰当、无越界和用户数据删除、并发行为可解释。

### T7：五平台CI、artifact门禁与体积报告

Files：.github/workflows/ci.yml/release.yml、scripts/release/check-client-artifact.py、test-client-artifact.py、smoke-duckdb-runtime.py、test-smoke-duckdb-runtime.py、check-binary-size.sh、check-assets.sh、check-macos-dependencies.sh、test-macos-dependencies.sh、test-distribution.sh；linux-runtime.py/test、policy及Dockerfile/build-linux/test仅必要修订。

Steps：1) 每个测试job准备T0runtime，环境通过GITHUB_ENV/明确命令注入，不在baseimage常驻引擎；release主程序先无库build+version/SQLite，defaultnormalfeatures无native link，再有库native测试；2) Linux两架构macOS两架构Windows各native验证版本/SQL/file/JSON/Parquet，可新增CI矩阵但不触发发布workflow；3) 新artifactcheck用标准Mach-O/ELF/PE tools查看imports/definedduckdb symbols，代理Rustmangled名称允许但Cduckdb_强制导入和engine定义拒绝；archive内容allowlist拒绝引擎库/header/cache，保留READMElicenses；4) report含raw_bytes/archive_bytes、target、SHA、toolchain、native_deps，可选runtime大小独立列；Windows工具用dumpbin或llvmobjdump受控探测，缺必需工具门禁失败而非silentpass；5) rootworkspacefmt/clippy/test覆盖syscrate；6) 固定Linuxneeded真实改变按report更新policy，不放宽到任意库；移除不再需要C++DuckDB构建前提时勿误删ODPI/SQLite仍需的Ccompiler；7) release只正常mainarchive，不Homebrew强制dependsduckdb；SBOM确认patched crate来源可审计，若现有SBOM只同名版本造成误导，另补明确metadata而非假装官方原版。

Commands：wrapper120 python3 scripts/release/test-client-artifact.py；wrapper120 python3 scripts/release/test-smoke-duckdb-runtime.py；wrapper120 sh scripts/release/test-distribution.sh；wrapper600 cargo +1.94.0 build --locked --release --bin lazydb；wrapper120 python3 scripts/release/check-client-artifact.py --binary <binary> --archive <archive> --target <target>。脚本CLI在实现时按上述固定，tests同合同。

Review：testfixture库不是productionpayload；不得移除macOSsystem-only检查来迁就ordinarydylib；nativeCI未执行需记录未覆盖，不把workflow配置存在当运行通过。
Acceptance：默认artifact可移植、无引擎，五平台有缺库和有库真实证据或明确外部必要验证项；体积报告能捕捉未来bundled回归。

### T8：文档、最终对照、门禁和Luna收尾

Files：README.md、docs/duckdb.md/configuration.md/releasing.md/database-capabilities.md/linux-runtime-compatibility.md、tests/docs.rs。任务validation.md持续追加，不复制到业务docs。

Steps：1) 文档外部C API1.5.6和目录变量、平台库名、CLI不保证有library、缺库消息、可信路径/成功加载需restart、不silentdownload；2) 保存已有URL/profile不迁移格式；3) docs说明prune只native/显式/legacy保守/进程探测受限，不把unsupported写为通用支持；4) 同80e118a同机SDK/profile/target构建最终main并原archive法比较，raw/下载/optionalruntime/保留release磁盘四项分开，不把独立实验百分比相加，保留baseline输入记录；5) 一次全门禁；6) unsafeABI/lifetime、版本和symbols、数据类型/事务取消、prune边界/锁、格式兼容/依赖license和scope逐项审查；7) 后续授权流程由Luna提交合并，不在plan阶段实现/commit。

Commands（Rustwrapper600，其他120）：cargo +1.94.0 fmt --all -- --check；cargo +1.94.0 clippy --locked --workspace --all-targets --all-features -- -D warnings；cargo +1.94.0 test --locked --workspace --all-targets --all-features；cargo +1.94.0 build --locked --release --bin lazydb；sh scripts/release/check-macos-dependencies.sh <actualbinary>；sh scripts/release/test-distribution.sh；git diff --check。Native远端driverjobs按原CI分组并600秒内，不在本地伪称全部live。

Review：逐项核对unsafe ABI和句柄生命周期、profile兼容、缺库基础命令、清理锁及目录边界、旧更新器解压兼容和五平台实际证据；检查每个通过结论都有对应命令结果，未执行项明确标注，暂缓候选不混入已完成范围。

Acceptance：用户无DuckDB库能正常用客户端，有兼容库保留完整DuckDB；压缩/更新/prune符合合同；最终检验有真实版本/结果，没有未验证声称；系统SQLite/TLS/剪贴板暂缓透明列出。

## 5. 补丁范围及变更控制

change-scope.json以历史计划中的changeScope为基础，已独立落盘，共72条预计实现/测试/文档路径，不列仅读取参考；没有删除/重命名业务文件，提取archive模块保持src/update.rs原路径并新增archive.rs。UPSTREAM pinnedABI文件和license是刻意新增，生成结果放OUT_DIR不提交大引擎。CI脚本cache/artifacts不入Git。若实际compiler发现UpdateArgs struct literal位于清单外，先加精确文件再只补兼容字段。Linuxpolicy等列为可能必要项，不要求每文件强行修改。

根workspace变化需验证Cargo包分发：路径patch在正常GitHub源码构建存在，cargo publish不能悄悄丢patch；本任务不发布crates.io新版本，docs明确源码构建需完整repository，不能承诺registry安装已更新。

## 6. 收尾复核及继续策略

T0→T1→T2构成第一完整闭环：正常无库启动、可恢复缺库连接、真实加载并查询。完成后持续T3→T8，不以“完成本轮复核”要求resume。T4可在runtime外部输入短暂缺失时独立推进，T5/T6只tempfixture。任何新失败先根据实际diff判断，不机械全量循环。

必需外部runner/runtime/权限真的缺失且所有独立工作完成时，记录external blocker及恢复条件；普通实现取舍不退回用户。人工/PTY检查S级不能阻塞。验证超时只一次针对诊断修复重试再记录限制/收尾审查，不无限progress。

计划阶段交付为同任务目录plan.md、change-scope.json和validation追加；实际文件校验通过后，最后写指定plan token回执completed。此完成状态只代表计划阶段交付，不表示业务代码已实施或后续测试已通过。第一实施动作由Luna在任务worktree确认实际diff，执行T0准备和T1/T2最小端到端闭环，再持续完成T3–T8。
