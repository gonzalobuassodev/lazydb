# Linux Runtime Compatibility Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 让 Linux 发布产物准确声明其实际运行要求，在安装与自动更新下载前识别已知不兼容系统，并保持旧客户端、现有功能和用户安装状态可用。

**Architecture:** 从最终归档中的 ELF 生成逐架构、摘要绑定的运行报告，用受控构建、兼容策略和真实运行验证防止要求无意上升。服务端并行生成原样 v1 和增加 requirements 的 v2 manifest；安装器与 Rust 更新器优先 v2，只有明确 HTTP 404 才回退历史 v1，并始终保留激活前运行检查。Linux 包依赖使用同源报告生成，运行证据与归档分开保存。

**Tech Stack:** Rust 1.94.0、Python 3 标准库、GNU binutils、POSIX sh、GitHub Actions、digest 固定的 Linux 容器、nFPM 2.42.0。

---

## 0. 执行权限、输入转移和验收类别

本计划依据同目录 `analysis.md` 和 `source-implementation-plan.md`。当前 plan 阶段只写任务目录，不改业务代码、index、state/checkpoint，不创建 worktree 或分支，不运行子 Agent。后续实现、审查、纠偏与提交合并由 Luna 执行，遵循届时阶段权限。使用 @writing-plans 完成计划；模板中的 executing-plans 若在环境缺失，不作为执行障碍，也不要求额外的模型/Agent 分工。用户已经选择自动工作流，不再次询问执行方式或等待 resume。

### 输入版本

- 原工作区：`/Users/yelog/workspace/tui/lazydb`；目标 `main`；基线 `0e68ae20e9a455d7c0f9b46bcecff2e7d0283059`。
- 当前没有 tracked/staged 变化；唯一 untracked 输入是 `docs/plans/2026-10-15-linux-runtime-compatibility-implementation.md`。
- 原计划 468 行，SHA-256 `2ab04289f596eb0d3d637eb4aa38fe0876170be6b2d5447608c1c62603fb352b`；本目录 `source-implementation-plan.md` 是完整快照。
- 新 worktree 不会自动复制未提交计划。实现时将该快照按原路径新增到任务 worktree，作为计划来源文档随任务交付；已列入 change-scope.json。原工作区不变。
- 没有未提交业务实现可复用，不允许假定原计划中尚不存在的脚本已被检出。执行依据是本 plan.md；来源计划保留历史，不把其旧流程说明当成阶段权限。
- checkpoint.json 在本阶段读取时不存在，不创建它。续做时优先读取插件 checkpoint 和实际 diff；只更新简短进度，不能覆盖历史回执。

### 验证分类：必须在报告中标注类别

| 类别 | 来源 | 要求 |
| --- | --- | --- |
| M1 用户明确要求 | 当前任务及输入实施计划 | 产物要求据实确定；双架构；v1 兼容；预检；失败保留旧安装；包依赖；记录实际结果。不得以 home 的 2.35 或构建机版本定下限。 |
| M2 项目现有门禁 | `.github/workflows/ci.yml`、`release.yml` | 分发契约、Rust fmt/clippy/all-targets/all-features tests，以及现有平台 CI。修改后必须保持可执行和原行为。 |
| M3 本次实现验收 | 本功能新增的自动化行为测试、发布运行检查 | 扫描器强弱节点/架构/损坏输入，v2 404 协商，下载前拒绝，状态不变，最终归档运行，包元数据与报告一致。新增为本功能的自动化验收，不能冒充此前已有门禁。 |
| S 补充证据 | 工程调查建议 | native 链接 map 的进一步语义调查、额外发行版抽样、QEMU 对比、人工 PTY/TUI dogfood、actionlint 本机便利检查。无相关行为变化时不将人工检查自动变为必需门禁。 |

**环境受限处理：** 本机为 macOS arm64，前阶段确认没有 GNU readelf/nFPM/actionlint，Docker CLI 存在但 daemon 未运行。不要无变化重跑环境探测；可先写平台无关解析/契约测试。相关 Linux/Windows 运行通过 CI 或已有合适环境补证，未执行记录“未执行”，不是通过。对同一人工/PTY/本地环境问题最多一次有针对性修复重试，之后继续独立工作，收尾审查决定是否补证或记录限制。不降低发布门禁，不循环尝试 Docker，不要求用户代实施剩余功能。

**完成标准不等于发布授权：** 不替换已有 0.2.0 asset，不改 tag，不自动运行发布 workflow。普通实现取舍自主完成；提交/合并按后续阶段指令，当前不执行。

## 1. 固定契约与依赖顺序

### 报告与策略

新增 `scripts/release/linux-runtime.py`，既可作为 CLI 使用，又由 generator/packager 通过 importlib 加载复用校验逻辑。使用标准库，不新增生产 Rust/Python 依赖。

CLI 接口确定为：

```sh
python3 scripts/release/linux-runtime.py inspect \
  --binary "$binary" --archive "$archive" --target "$target" \
  --version "$version" --output "$report"
python3 scripts/release/linux-runtime.py check-policy \
  --report "$report" --policy scripts/release/linux-compatibility-policy.json
python3 scripts/release/linux-runtime.py verify-report \
  --report "$report" --archive "$archive" --target "$target" --version "$version"
```

输出报告名 `lazydb_VERSION_TARGET.runtime.json`，稳定排序、末尾换行，内容包括：

```text
schema=1, product=lazydb, version, target
archive: name, sha256
binary: sha256
elf: machine, class, endianness, interpreter, needed[], rpath[], runpath[]
glibc: loader_min, version_needs[{library,version,weak}],
       symbol_references[{name,version,binding,library}]
other_version_needs[{library,name,weak}]
```

`loader_min` 取必需版本需求节点，不把“弱符号”误当成“弱版本节点”。无有效必需 glibc 节点、私有 ABI、未知非数字标签或不匹配架构应拒绝报告。对 GCC 等其他版本需求留证，不把 glibc 条件当成全部系统能力。

报告在归档外，不能把包含自身 archive SHA 的报告打回同一归档；运行后生成另一个 evidence JSON，禁止更新已经 checksum 的报告。

策略文件按 target 记录 glibc 接受上限、NEEDED allow-list、interpreter、build image digest、验证环境。数值来自扫描和运行证据，无默认 2.35。新增非基础动态库要求必须显式评估；不提供自动“接受当前值”的 CI 开关。

### v1 / v2

- v1 `/channels/stable.json`、`beta.json`，schema 1，字段原样。
- v2 `/channels/v2/stable.json`、`beta.json`，schema 2；同源版本、tag、URL、摘要、五目标集合。
- Linux asset 新增 `requirements.libc = {"family":"glibc","min_version":"M.N"}`。示例数值不是产品常量。
- v2 Linux 要求必填、非 Linux 不得误加 libc 限制，继续拒绝未知字段和不合法 URL/版本/摘要。
- 仅 HTTP 404 可回退 v1；网络错误、403、5xx、200 无效 JSON、错误 schema 不回退。v2 路径返回 v1 文档也拒绝，避免“200 降级”。
- 新发布强制双报告/v2。历史报告缺失仅通过明确的 legacy 组装路径处理，不默默生成猜测要求。

### 单元依赖

```text
U1 输入/ABI 扫描 -> U2 构建策略
U1 -> U3 manifest/Pages 契约 -> U4 安装器 -> U5 更新器
U1/U2/U3/U4/U5 -> U6 最终归档运行与 CI
U1/U2 -> U7 包依赖
U3～U7 -> U8 迁移回归/文档/收尾
```

每单元先写有意义的失败用例，再实现最小完整行为，复核 diff 和相关测试通过后继续下一单元；不为纯文档或简单同步改动写镜像测试。不在每单元重复全量编译。

## U1：可复现的逐架构 ABI 报告（第一个未完成闭环）

**文件：**

- 新增 `scripts/release/linux-runtime.py`、`scripts/release/test-linux-runtime.py`。
- 新增最小 fixture 目录 `scripts/release/fixtures/linux-runtime/`（GNU readelf 两架构文本、小型 C 源码；不提交大二进制）。
- 修改 `scripts/release/test-distribution.sh`。
- 新增 `docs/linux-runtime-compatibility.md` 的事实调查部分。
- 在任务 worktree 新增来源文档 `docs/plans/2026-10-15-linux-runtime-compatibility-implementation.md`，复制指定快照。

**步骤：**

1. 确认任务 worktree 基线；读取原计划快照并核对 SHA，按原路径复制，不触碰原工作区和 index。
2. 先定义 Python 公开函数 `parse_readelf_outputs(...)`、`inspect(...)`、`validate_report(...)`、`compare_versions(...)`，编写标准库 unittest 失败用例。断言以解析后的语义为主，不仅比对快照。
3. fixture 覆盖：x86_64/ARM64；node non-weak + symbol weak；node weak；多个库；2.9 < 2.38；版本定义与需求分区；strip 后动态信息；truncated 输出；错误 machine；GLIBC_PRIVATE/未知标签。
4. 运行 `python3 -B scripts/release/test-linux-runtime.py`，确认初始失败源于未实现行为。
5. 实现 GNU readelf 调用，固定 `LC_ALL=C`，校验 class/machine/interpreter、NEEDED、RPATH/RUNPATH；所有子进程错误携带阶段和 stderr，不能返回空报告。
6. inspect 从精确 archive root `lazydb_VERSION_TARGET/lazydb` 读取 regular file，验证 binary 参数字节一致，拒绝链接、异常路径及架构不符。只需要读取成员，不执行归档内程序。
7. 计算两种 SHA，确定性写 report；verify-report 重读实际归档验证 report，不只验证 report 自洽。
8. 在 Linux 专用 unittest 用 `cc` 编译最小 C 程序、运行 readelf、strip 再扫描；非 Linux 明确标 skip。CI 在 Linux 必须运行该用例，不能通过缺工具 skip。
9. 读取当时 channel 并按真实 checksum 获取历史资产（临时文件仅放任务允许的临时/证据目录），记录两个架构现状。为 native 来源分析在同 tag 构建生成 map；ODPI-C 的 strtol 目前仅候选，不声称已完成归因。无法执行链接调查时将待补证清楚记录，不阻止 parser/metadata 实现。
10. 接入分发测试入口，写事实文档，记录已确认/待验证边界；不写死 2.35 或把 2.39 填给所有架构。

**复核：** ELF 需求节点与符号绑定分别处理；报告只绑定这份产物；不使用 strings/正则抓全部 GLIBC 取最大；Python fixture 可在 macOS 运行。

**验证命令与预期：**

```sh
python3 -B scripts/release/test-linux-runtime.py
python3 scripts/release/linux-runtime.py inspect --help
```

M3：平台无关用例通过；Linux 实物用例在 Linux 正向、错误输入拒绝；help exit 0。S：链接 map 深查不伪装成已完成的强制测试。接着进入 U2。

## U2：受控构建与要求上升检查

**文件：**

- 新增 `scripts/release/linux-compatibility-policy.json`、`scripts/release/build-linux.sh`、`scripts/release/Dockerfile.linux`。
- 修改 `scripts/release/linux-runtime.py`、`scripts/release/test-linux-runtime.py`。
- 修改 `.github/workflows/release.yml` Linux build 分支。
- 修改 `docs/linux-runtime-compatibility.md`。

**步骤：**

1. 在 scanner tests 添加 policy 用例：实际要求等于/低于上限成功，提高要求、NEEDED 新增、interpreter 变化、私有路径失败。
2. 实现 check-policy，按架构查策略，拒绝未知 target 和缺字段。report lower 不等于自动扩大官方支持范围。
3. 基于 U1 的事实写首次策略，两架构独立。查出高要求可避免时先记录代价与语义风险；本任务保持当前系列和功能，后续优化不默认绑在本次交付上。
4. Dockerfile 使用经过查证的 Ubuntu 24.04 digest，安装并记录必要 native 工具、Python、xz/binutils；Rust 固定 1.94.0。不能只固定 tag 或使用编造 digest。保留许可证和 default Oracle 驱动。
5. build-linux 接口为 `TARGET` 单参数，在原生 runner 运行固定容器，明确 Cargo registry/target 缓存映射和输出归属；允许 workflow 控制 CARGO_BUILD_JOBS。禁止 target-cpu=native。
6. Release matrix 的 Linux 构建使用 wrapper，macOS/Windows 保持原命令；cache key 含 Rust/target/image/policy 摘要。说明固定 image digest 不锁死后续 apt 内容，记录工具版本供调查。
7. 归档完成后调用 inspect/check-policy；artifact 上传新增 report，不能仅上传 tar/zip 丢失报告。U6 补齐独立运行 gate。

**复核：** 不声称全构建字节可复现；不能通过更新常量掩盖未解释上升；容器和原生输出路径一致。

**验证：** `python3 -B scripts/release/test-linux-runtime.py`、`sh -n scripts/release/build-linux.sh`；Linux 环境执行 `sh scripts/release/build-linux.sh x86_64-unknown-linux-gnu` / ARM64 对应 target。后两项需原生环境，M3 的发布 CI 路径必须具备；本地无 daemon 记录限制，不循环探测。构建出的报告通过策略检查且功能集合未删减。

## U3：同源 v1/v2 manifest 与 Pages 迁移契约

**文件：**

- 修改 `scripts/release/generate-channel-manifest.py`、`scripts/release/check-assets.sh`、`scripts/release/assemble-pages.sh`。
- 修改 `scripts/release/test-channel-manifest.sh`、`scripts/release/test-pages.sh`。
- 修改 `.github/workflows/pages.yml` 的元数据获取/保存逻辑。

**步骤：**

1. 新增 fixture 工厂生成真实最小 tar.xz、相应 binary/归档摘要和 synthetic report；所有调用 generator 的测试使用同一契约，纯字符串 fake archive 不能用于 v2 report 验证。
2. 添加失败用例：缺/篡改/调换 report、报告 checksum 缺失、report binary 摘要错、未知字段/requirements、v1/v2 版本不一致、相反 channel 暂时下载错误。
3. generator 兼容旧五个 positional 参数，增加 `--schema 1|2` 默认 1；v1 输出字段原样，v2 复用资产身份校验，验证 runtime report 及最终归档实际 binary 摘要。
4. 新 release 的 asset checker 要求两个 report 均在 SHA256SUMS 并校验绑定。增加显式 legacy 参数仅给历史 Pages 组装；新 publish 调用永远严格。
5. assembler 在 `channels/` 和 `channels/v2/` 生成当前 channel 双版本；不打包 binary 到 Pages。保留相反 channel 时校验二者版本、tag、五目标 URL/摘要一致。
6. pages workflow 下载相反 channel v1/v2，明确读取 HTTP 状态：404 可代表首次发布/历史 v2 不存在，其他错误停止；禁止用任意旧本地 JSON 悄悄回退导致不一致。保留原自动 Release SHA/手动 ref 语义。
7. 历史缺 report 的显式 legacy 路径仅生成 v1，v2 保持真正 404，不生成 null 要求。stable/beta 每次部署保留另一条历史状态。
8. 更新 Pages 文件 allow-list 和重复组装测试；新目录不能让文件名 allow-list 接受意外路径。

**复核：** 未触碰旧客户端 v1 parser；缺报告不会成为新发布默认分支；checksum 顺序没有 report 自引用。

**验证与验收：**

```sh
sh scripts/release/test-channel-manifest.sh
sh scripts/release/test-pages.sh
```

M2/M3：旧字段集合精确不变，v2 仅含真实绑定要求；篡改拒绝；首次/双 channel/redeploy 保留正确；网络非 404 错误不会清空另一 channel。继续 U4。

## U4：安装器下载前预检与失败诊断

**文件：** 修改 `install.sh`、`pages/install-core.sh`、`scripts/release/test-installer.sh`、`scripts/release/test-pages.sh`。

**步骤：**

1. 为 fake curl 添加 `--write-out`/status 行为及请求日志；模拟 v2/v1/404/403/500/timeout/invalid JSON。为 getconf 添加可注入结果，避免依赖宿主真实版本。
2. 测试低于要求时只请求 manifest，原 `current`/launcher/install.json/config/profile 的字节和链接均不变，无归档请求；等于/高于要求继续。
3. 两份 installer 优先请求 v2，捕获 curl exit 和 HTTP code 分开处理；只有明确 404 请求 v1。保持 TLS、URL allow-list、摘要和归档安全校验。
4. Python 解析要求为数字元组，Linux 先 getconf 再 os.confstr；确定不兼容 exit 非零并包含 release/target/current/required/处理建议。无法确定检测结果提示 unknown 后继续 staged 检查；可靠确认 musl 才明确拒绝，不能仅因 getconf 缺失判 musl。
5. 对 v1 提示“没有预检元数据”，不阻塞历史安装。输出不要建议手动替换系统 libc，也不要宣称旧版本一定可用。
6. staged binary stderr 留在诊断内，保存真实 exit status，不能在 `! command` 后误读反转后的 `$?`。相关错误输出分辨 glibc、其他动态库、权限等，不一律写 glibc 错误。
7. 同版本 DEST 复用时在切换前验证实际将被使用的 binary；已有坏 DEST 不能被 staged 成功掩盖。针对变化路径最小修复并保留旧激活状态。
8. 不扩展成全安装事务重写：新增预检/下载/校验/解压/staged/DEST 检查失败必须无激活变更；既有切换后外部故障的范围在文档中说明，不能声称所有 OS 故障可回滚。
9. Pages test 实际运行生成的单文件入口，保留 curl-pipe-sh、PATH、profile、MCP、channel lock 行为。

**复核：** root 与 Pages 行为相同；unknown 不被标成 compatible；仅 selected target 要求参与比较；Mac 不依赖 getconf。

**验证：** `sh scripts/release/test-installer.sh`、`sh scripts/release/test-pages.sh`。M3 必测请求次数及旧安装 snapshot；M2 既有 installer case 通过。人工重装到 home 属 S，不是验收前提。

## U5：Rust 原生更新的协议协商和兼容预检

**文件：** 修改 `src/update.rs`、`tests/update_native.rs`、`tests/update_http.rs`；新增 `src/update/runtime.rs`。

**步骤：**

1. runtime 模块定义数字版本和检测结果的纯逻辑，例如 `DetectedLibc::{Glibc(version), Other, Unknown}`、`Compatibility::{Satisfied, Unsupported(reason), Unknown(reason)}`。通过现有 InstallationProbe 注入 getconf 输出，Linux GNU 才比较，不新增 unsafe/生产依赖。
2. 添加 `RuntimeRequirements`/libc 数据结构，保持 deny_unknown_fields；ManifestAsset 的可选字段 serde default、skip none；v2 校验 Linux 必填且合法，v1 要求缺省。
3. 先用单测覆盖 2.9/2.38 比较、边界相等、溢出/错误类型、空输出、unknown、不同 target 要求；不能假定 SemVer 能解析二段 glibc。
4. 给 UpdateHttpClient 增加有默认实现的内部可选 GET 或结构化 404 检测接口；System 实现通过实际 status 判断，复用原代理/redirect/HTTPS 校验，不放宽下载 URL。fake 可显式返回 not found，不能把 anyhow 文本包含 404 当契约。
5. fetch 优先 v2，明确 404 后 v1；分别验证 endpoint 对应 schema，不能把 v2 的 200/schema1 当 fallback。覆盖 403/5xx/timeout/损坏文档。
6. inspect 对确有新候选且本机已确认不兼容返回现有 Error/action；同版本/非 native 包管理流程不被误改。unknown 按既有 JSON/action/stderr 规则说明。
7. apply 下载前再次检查，传入相同可测试 probe 或检查结果；CLI `run_with_progress` 和 TUI install 路径均覆盖，不能只保护其中一处。
8. staged failure 给出清理后的有界 stderr 和 exit status，复用 terminal sanitization；不新增通用进程框架。对 stderr 的真正缓冲限制与展示长度区分清楚，正常版本进程沿用现有生命周期。
9. native fixture HTTP 服务支持 `/v2/stable.json` 和 404 后 `/stable.json` 多次请求；验证下载次数=0、旧文件/链接 snapshot 不变、原安装可运行。proxy tests 确认新 GET 仍遵守现有代理配置。
10. 保持 update JSON 七字段、单行和 stdout 无进度/ANSI；TUI 沿用已有 Error/action 展示，不扩大状态枚举。

**验证：**

```sh
cargo +1.94.0 test --locked --lib update::
cargo +1.94.0 test --locked --test update_native
cargo +1.94.0 test --locked --test update_http
```

M2/M3：解析/404/预检/保护和本地代理测试通过；外部网络代理探测若属于原 optional/ignored 用例如实区分，不转为新强制联网门禁。复核全部 trait 实现编译与 CLI/TUI 两路径调用覆盖。

## U6：最终归档运行证据与 Release / Pages CI

**文件：**

- 新增 `scripts/release/smoke-linux-runtime.sh`、`scripts/release/test-linux-runtime-smoke.py`。
- 修改 `.github/workflows/release.yml`、`.github/workflows/pages.yml`、`.github/workflows/ci.yml`。
- 修改 `scripts/release/linux-compatibility-policy.json`、`scripts/release/check-assets.sh`。
- 修改 `scripts/release/smoke-online-install.sh`、`scripts/release/test-online-install.sh`。

**步骤：**

1. 定义 smoke 接口为 `ARCHIVE REPORT VERSION TARGET EVIDENCE_OUTPUT`，读取归档并验证 binding，不重新编译应用；临时 HOME/config/project 与用户环境隔离，关闭交互 MCP。
2. Python 驱动运行 version、doctor、临时 SQLite profile 的 SELECT 1、local fixture update-check；解析返回值=1，不能只检测 JSON 包含 rows。doctor 可选 warning 不当必需功能错误。
3. evidence 含 archive/binary 摘要、版本、target、镜像 digest、实际 glibc、native/emulated 标记、各测试结果。单独文件 `lazydb_VERSION_TARGET.runtime-evidence.json`。
4. 根据 U1/U2 的事实选边界、较新、低于要求三类验证环境。精确边界无常见发行版时使用隔离、匹配的 loader+libc sysroot/镜像，禁止新 loader+旧 libc 混搭；找不到边界证据时标明未验证差距，不能虚构通过或抬高 numeric 字段来绕开。
5. 两架构 native runner 正向；低于要求环境验证新安装器拒绝且未下载，必要时独立检查弱版本节点回退；QEMU 为 S 补充，输出明确模拟。
6. CI PR job 增加 scanner fixture 和 Linux 集成；Release 生成/校验报告并增加 runtime job，publish 明确 needs runtime。policy 被上调时 runtime 必须有相应镜像映射，否则 job 拒绝。
7. artifact upload/download path 包含报告/evidence；stable package 与 beta 跳过包的分支都要求 runtime 成功。全部资产就绪后 SHA256SUMS/检查/attestation，再发布，不让报告循环绑定 checksum。
8. Pages 在线验证用相应 native target 和受支持环境，新拒绝场景为期望失败测试，不能把 home 一律列为安装成功环境。保留 macOS 和 Windows 现有检查。
9. 修改 online-install fixture 验证无交互、重试次数、临时环境清理；只对 CDN 传播进行有界重试，确定不兼容不重试多轮相同安装。

**验证：**

```sh
python3 -B scripts/release/test-linux-runtime-smoke.py --help
sh scripts/release/test-online-install.sh
sh scripts/release/test-pages.sh
```

Linux CI 以实际参数执行 `sh scripts/release/smoke-linux-runtime.sh "$archive" "$report" "$version" "$target" "$evidence"`；M3：最终资产在声明环境通过且拒绝场景正确。M2：现有发布 DAG 与非 Linux 检查不回归。S：本机 `actionlint .github/workflows/ci.yml .github/workflows/release.yml .github/workflows/pages.yml` 可用时运行，缺工具不触发无限安装尝试；workflow 语义仍须代码复核。

## U7：Linux 包依赖与报告一致

**文件：** 修改 `packaging/nfpm.yaml`、`scripts/release/package-linux.sh`、`.github/workflows/release.yml`；新增 `scripts/release/test-linux-packages.sh`；必要的纯转换逻辑放已有 linux-runtime 模块及其 tests，不再新增依赖框架。

**步骤：**

1. 改包脚本接口为 `VERSION ARCH BINARY RUNTIME_REPORT OUTPUT_DIR`；所有生产调用同步；binary 摘要/target/版本与 report 不符时在执行 nFPM 前拒绝。
2. 用真实报告生成各格式依赖：DEB libc6 与 libgcc-s1 等实际包归属；RPM 按实际 ELF symbol requirement/系统库表达；Arch glibc 约束和基础库包。nFPM 不等于 rpmbuild 自动依赖扫描，不假定其自动补齐。
3. 通过标准库生成 nFPM 所需的临时配置/变量；不对用户目录写配置，不固定 2.35，不使用不能展开数组的环境替换误生成单个依赖字符串。
4. 更新 package job 下载 report/验证 binary，再生成与归档完全相同的 stable 包。beta 仍不打包。
5. Linux package test 调用 nFPM 2.42.0 构建真实包，查询 `dpkg-deb -f`、`rpm -qp --requires`、tar 提取 `.PKGINFO`；检查 package binary SHA 与 report 一致。对支持的代表发行版包管理器验证依赖解析/拒绝，不仅 grep 源模板。
6. 不将字体、密钥环服务、DB 服务、可选 Oracle Instant Client 写成所有用户的硬依赖；仅主程序加载必需的 native 代码要求属于主包。

**验证：** Linux package CI 运行 `sh scripts/release/test-linux-packages.sh`。M3：三格式依赖、版本、架构与报告一致，篡改报告在打包前失败。无本地 nfpm 记录限制，不声称已验证包安装；代表发行版额外抽样可为 S，但报告/包内容校验是本功能自动化必需验收。

## U8：迁移回归、文档与收尾

**文件：**

- 修改 `README.md`、`docs/releasing.md`、`docs/linux-runtime-compatibility.md`。
- 修改 `scripts/release/test-channel-manifest.sh`、`scripts/release/test-pages.sh`、`scripts/release/test-installer.sh`、`scripts/release/test-windows-installer.ps1`、`tests/update_native.rs` 的必要迁移断言。
- 仅在确需通过 CLI 暴露验证版本/限制时再扩展文档，不修改 version JSON/TUI 契约。
- 当前任务不准备发布版本，不修改 Cargo.toml/Cargo.lock/CHANGELOG.md；发布说明在独立 release 阶段准备。

**步骤：**

1. 从真实历史归档取得可在验证系统启动的旧客户端，让它读取新生成的 v1；确认严格旧 parser 接受。只用新 parser 读 v1 不足以验证迁移。
2. 旧客户端更新到新版本：环境满足要求可激活，不满足则 staged 拒绝且保留旧版；明确旧客户端无法获得下载前提示。
3. 新客户端测试 v2、历史 v1 fallback、首次部署、两 channel 先后部署、部分元数据缺失、CDN 暂时错误；不得为消除 404 修改发布中已有二进制。
4. Windows PowerShell/pwsh 继续消费 v1，无 Linux 工具依赖；macOS 新 Rust updater 能解析新协议但跳过 libc 检测。
5. 文档写清当前资产加载要求、已验证环境、可选客户端边界、查 glibc 命令、来源与要求上升流程；不声称理论最低版本已全部证明。解释本次主要消除错误分发与迟滞诊断，不保证 home 必定安装成功。
6. 针对相关 diff 做收尾复核；功能齐备后执行下述 M2 全量一次，有新失败或相关修改才重跑。不得把 analyze/plan 的语法结果当实现后的行为验证。
7. validation.md 逐条记录命令、exit、代码版本/dirty diff、平台、跳过及原因；后续审查检查 M1/M2/M3 与 S 的区分，决定环境未补证的交付限制。不能将未执行状态静默变为通过。
8. 后续 commit/merge 仅按自动流程阶段指令操作，不在本计划阶段或普通实现单元中插入发布操作，不要求用户 resume。

**全量验证：**

```sh
python3 -B scripts/release/test-linux-runtime.py
sh scripts/release/test-distribution.sh
cargo +1.94.0 fmt --all -- --check
cargo +1.94.0 clippy --locked --all-targets --all-features -- -D warnings
cargo +1.94.0 test --locked --all-targets --all-features
git diff --check
```

同时收集 U6 两架构 runtime、U7 package、既有 macOS/Windows CI 实际结果。已有数据库集成门禁仍按项目配置执行，不新增无关 DB 测试。人工 TUI/PTY、在 home 真实安装、额外发行版全面覆盖是 S，用户没有将它们设为强制手工门禁。

## 2. 收尾验收清单与交接

- [ ] 输入计划已从任务目录快照带入新 worktree；未依赖原工作区未提交业务行为。
- [ ] scanner 对最终资产精确绑定，强弱节点/符号、架构、异常输入均有行为证据。
- [ ] policy 数值由实际产物产生，要求意外上升阻止发布，没有硬编码 home 标准。
- [ ] v1 结构不变，真实旧 parser 兼容；v2 来源与归档一致，仅 404 fallback。
- [ ] 新安装/更新预检在下载前执行；拒绝及激活前失败不改变旧版或配置。
- [ ] Rust CLI/TUI 共用逻辑，JSON 七字段和非 native 更新方式不变。
- [ ] 双架构边界/较新/不兼容场景由自动化覆盖，证据区分 native、模拟、静态和真实运行。
- [ ] Release/Pages 双 channel 与 artifact/DAG/摘要顺序完整，历史缺元数据路径显式。
- [ ] DEB/RPM/Arch 的真实包元数据和 binary 与 report 一致。
- [ ] M1/M2/M3 检查与 S 建议单独记录，环境限制无无限重试、无通过冒报。
- [ ] 文档真实说明支持范围，不暗示原 glibc 2.35 机器必然可用。
- [ ] 原资产/tag/生产配置保持不变，发布授权不混入本任务。

**自动续做入口：** 下一阶段直接执行 U1 第 1～4 步：读取快照、创建扫描器解析 fixture 和失败测试，实现第一条可验证报告路径。完成 U1 后继续 U2～U8，不再次询问执行方式。任务分支可命名 `task/linux-runtime-compatibility`，最终由插件/Luna 命名。本 plan.md 与 change-scope.json 是本轮交接依据，不使用旧回执 next 作为新需求。
