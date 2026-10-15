# Linux Runtime Compatibility Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 根据 Linux 发布产物及其依赖的实际要求，建立可追溯、可验证的运行环境约束，让安装器和自动更新提前识别不兼容系统，并保留旧客户端的更新能力。

**Architecture:** 在构建与打包完成后检查最终 ELF，生成逐架构的运行要求报告，并用隔离环境验证报告中的边界。发布流程由同一份报告生成兼容元数据和 Linux 包依赖；新增版本化的 channel manifest，原有 manifest 保持原样服务旧客户端。构建基线的调整以依赖来源分析和运行验证为依据，不预设 glibc 2.35，也不将构建机的 glibc 版本当成软件要求。

**Tech Stack:** Rust 1.94.0、Cargo.lock、Python 3 标准库、GNU readelf/binutils、POSIX sh、GitHub Actions、隔离 Linux 容器、nFPM。

---

## 计划边界与执行约定

- 本文是实施计划；编写本文不代表已经实现代码、执行下面的测试或批准发布。
- 实现分为三个可独立审阅的阶段：发现与约束、元数据与消费端、发布与交付验证。
- 不以 `ssh home` 的环境作为产品标准；该机器只提供一个已确认的不兼容样本。
- 不预先承诺支持 Ubuntu 20.04、22.04 或任意 glibc 版本。先确认技术要求，再记录实际验证范围。
- 不修改用户系统 glibc，不默认引入 musl 产物，不修改已有 tag 或替换已发布二进制。
- 不默认新增生产 Rust 依赖。构建期检查优先复用 Python 标准库和 binutils。
- 实现过程中不执行 commit、push、tag 或部署；这些操作需要对应任务的明确授权。下面的交付批次也是建议的审阅边界。
- 开头的执行技能提示来自计划模板。如果实际环境没有该技能，按本文任务顺序和现有项目指令执行即可，不依赖额外的代理或插件。

## 已确认事实与仍需验证的内容

| 项目 | 证据与结论 |
| --- | --- |
| 故障主机 | Ubuntu 22.04.5 LTS，glibc 2.35，x86_64。 |
| 构建方式 | `.github/workflows/release.yml` 对两个 Linux GNU 目标直接执行 `cargo build`，Runner 分别为 Ubuntu 24.04 和 Ubuntu 24.04 ARM。 |
| 当前稳定版 | 本次调查时 public stable manifest 指向 `0.2.0`。 |
| x86_64 发布包 | 校验 SHA-256 后检查 ELF：存在强引用 `__isoc23_strtol@GLIBC_2.38`，最高标为非弱的 GLIBC 版本需求节点为 2.38。 |
| ARM64 发布包 | ELF 中 GLIBC_2.39 版本需求节点标为非弱；对应新符号中又存在弱绑定，必须结合加载器检查及原生运行验证处理，不能照搬 x86_64 结论。 |
| 安装保护 | 两个 POSIX 安装器都在切换 `current` 前执行 staged binary；Rust 更新器也在发布新状态前运行 staged binary。 |
| 诊断缺口 | Rust `run_staged_version()` 对子进程失败只给通用错误，没有保留加载器的具体 stderr。 |
| 测试缺口 | Linux 发布和线上安装检查集中在 Ubuntu 24.04，没有验证声明的最低运行边界。 |
| 元数据约束 | `ChannelManifest` 和 `ManifestAsset` 都使用 `serde(deny_unknown_fields)`；直接扩展现有线上 JSON 会破坏旧更新客户端。 |
| 包依赖缺口 | `packaging/nfpm.yaml` 的 DEB、RPM、Arch 依赖目前为空。 |
| 尚未确认 | 新版符号来自哪个具体编译单元；是否真正需要其语义；完整功能可运行的最低环境；是否值得调整构建方式。 |

## 核心设计决定

### 1. 将检测事实、兼容策略、运行证据分别记录

维护三种信息，避免一个“最低版本”承担不同含义：

1. **ELF 加载要求：** 当前文件的版本需求、动态库依赖、加载器和强弱符号信息，由扫描器生成。
2. **发布兼容策略：** 已接受的要求上限、依赖清单和测试环境，由版本控制管理；提高要求必须附原因。
3. **运行验证证据：** 最终归档文件在什么 glibc、架构和系统上通过了哪些操作，由 CI 生成。

安装前的 glibc 判断是必要条件检查；它不能代替最终启动检查，也不单独代表所有功能、所有发行版都可用。

### 2. 使用版本化 manifest，保持旧协议可用

保留现有地址与 schema 1 数据结构：

```text
/channels/stable.json
/channels/beta.json
```

新增地址：

```text
/channels/v2/stable.json
/channels/v2/beta.json
```

schema 2 的 Linux asset 增加 `requirements`；原字段、目标名称和归档文件名保持一致。新客户端优先请求 v2，只有收到明确的 HTTP 404 才回退 v1。403、超时、5xx、无效 JSON、未知 schema 或无效要求一律报错，不能借回退忽略损坏数据。

v1 回退用于尚未迁移的历史 channel：明确说明无法预检，继续保留 staged binary 检查。新发布流程必须提供 v2，不能长期依赖回退。旧客户端仍只能完成下载后的检查，不能通过修改服务端元数据给旧程序追加预检能力。

### 3. 首先控制现有构建，再评估是否降低要求

第一阶段不切换到 Ubuntu 22.04，也不直接引入 cargo-zigbuild。先固定当前 Linux 构建环境并记录真实要求，建立防止要求无意上升的检查。

只有确认新版符号可以在不删减功能的前提下避免，并完成两种架构验证后，才调整构建 sysroot 或容器。该调整是后续依据明确的优化，不是本次元数据建设的先决条件。

## 任务依赖与建议批次

```text
任务 1 调查 -> 任务 2 扫描器 -> 任务 3 策略与构建约束
                         -> 任务 4 版本化元数据 -> 任务 5 安装器
                                               -> 任务 6 自动更新
任务 2、3 -> 任务 7 运行边界验证 -> 任务 8 发布与 Pages 集成
任务 2、3 -> 任务 9 包依赖
任务 4～9 -> 任务 10 迁移与回归 -> 任务 11 文档与发布交付
```

建议审阅批次：A 为任务 1～3，B 为任务 4～6，C 为任务 7～9，D 为任务 10～11。每个批次都有独立证据和验收条件；依赖明确后再推进下一批次。

## 任务 1：复核产物并追溯新版 glibc 符号来源

**文件：**

- 新建：`docs/linux-runtime-compatibility.md`，先记录调查结果，之后补齐正式约定。
- 调查：`Cargo.toml`、`Cargo.lock`、`.github/workflows/release.yml`。
- 临时产物：`dist/runtime-audit/`；保留报告与命令摘要，二进制、链接 map 和下载文件不提交仓库。

**步骤：**

1. 读取当前 stable/beta manifest，记录版本、tag、URL、归档 SHA-256；下载两个 Linux 归档并验证摘要。不要假定未来 stable 仍为 0.2.0。
2. 在相应架构环境中检查最终归档内的 ELF，保存以下命令输出：

   ```sh
   readelf --wide --file-header "$binary"
   readelf --wide --program-headers "$binary"
   readelf --wide --version-info "$binary"
   readelf --wide --dyn-syms "$binary"
   readelf --wide --dynamic "$binary"
   ```

3. 单独记录 `GLIBC_2.38`、`GLIBC_2.39` 的版本需求节点标志、符号绑定及所属库；同时检查 `RPATH`、`RUNPATH` 和非基础动态库。
4. 使用与被审计发布包相同 tag 的源码、工具链和功能集合，在隔离的诊断输出目录重新构建，生成链接 map，并使用 `nm` 检查 native archive 的未定义符号，定位 `__isoc23_strtol` 的实际来源。先核对原发布 job 的构建环境，不能用任意当前源码的结果解释历史产物。示例命令在 Linux x86_64 上执行：

   ```sh
   mkdir -p dist/runtime-audit
   cargo +1.94.0 tree --locked -e features > dist/runtime-audit/cargo-features.txt
   CARGO_TARGET_DIR=dist/runtime-audit/target cargo +1.94.0 rustc \
     --release --locked --target x86_64-unknown-linux-gnu --bin lazydb -- \
     -C strip=none -C link-arg=-Wl,-Map,dist/runtime-audit/x86_64.map
   ```

   根据 map 继续检查实际引用的 `.o` 或 `.a` 文件；不要先假定来自 SQLite、Oracle 或 Rust 标准库。
5. 为每个提高最低要求的符号写出来源和必要性：直接功能需要、依赖的编译行为、可回退的优化路径，或暂未确定。弱符号的回退路径需要单独测试。

**验收：** 两种架构各有可复现的事实报告；能够说明已知新版符号来自哪里。尚未确认的项明确标出，不将推测写入正式支持声明。

## 任务 2：实现逐架构 ELF 运行要求扫描器

**文件：**

- 新建：`scripts/release/linux-runtime.py`。
- 新建：`scripts/release/test-linux-runtime.py`。
- 新建：`scripts/release/fixtures/linux-runtime/` 下的 readelf 文本样本和小型 C 测试源文件。
- 修改：`scripts/release/test-distribution.sh`，加入平台无关的解析测试。

**步骤：**

1. 先编写解析测试，覆盖版本节点非弱但符号弱、节点弱但符号存在、多个所属库、2.9 与 2.38 的数字比较、损坏输入和未知版本名称。
2. 实现 `inspect` 子命令，固定 `LC_ALL=C` 调用 GNU readelf，将版本需求、动态符号和依赖关联起来；不能扫描 `strings` 后取最大数字，也不能把版本定义误当成版本需求。
3. 校验 ELF class、endianness、machine 与目标架构匹配，确认有预期的动态加载器。文件格式不符、非预期架构、`GLIBC_PRIVATE` 或无法解释的非数字 GLIBC ABI 标签应阻止发布，并保留原始证据。
4. 输出确定性的 JSON，绑定 release、target、archive SHA-256、binary SHA-256；至少包含：

   ```text
   schema, product, version, target
   archive.name, archive.sha256, binary.sha256
   elf.machine, elf.interpreter, elf.needed, elf.rpath, elf.runpath
   glibc.loader_min
   glibc.version_needs[]: library, version, weak
   glibc.symbol_references[]: name, version, binding, library
   other_version_needs[]: library, name, weak
   ```

5. 增加 Linux 集成测试：编译小型 C 程序，实际运行 readelf，并核对扫描器报告；同时验证 `strip` 后仍能读取运行加载必需的动态版本信息。

**计算规则：**

- 按数字分量比较版本，不能使用浮点数或字典序。
- glibc 加载下限首先考虑版本需求节点；非弱节点即使只关联弱绑定符号，也不能被删除。
- 弱节点不直接当作硬下限，但必须保留，用于运行边界验证和诊断。
- 扫描结果代表这份 ELF 的加载要求，不能声称由此证明了完整应用的理论最低版本。
- 对随包提供的共享库做同样检查；对系统基础库记录需求并在干净运行环境验证，不把构建机 libc 自身的所有版本定义并入应用要求。

**验证命令：**

```sh
python3 -B scripts/release/test-linux-runtime.py
python3 scripts/release/linux-runtime.py inspect --help
```

预期：解析和错误路径测试通过；真实 binutils 集成用例在 Linux CI 上通过。在 macOS 上可以运行解析测试，Linux 专属用例必须明确跳过，不能伪报已执行。

## 任务 3：建立受控构建和兼容要求变更检查

**文件：**

- 新建：`scripts/release/linux-compatibility-policy.json`。
- 新建：`scripts/release/build-linux.sh`。
- 新建：`scripts/release/Dockerfile.linux`。
- 修改：`scripts/release/linux-runtime.py`，增加 `check-policy` 子命令。
- 修改：`.github/workflows/release.yml` 的 Linux 构建步骤和缓存键。

**步骤：**

1. 根据任务 1 的实际结果写入逐架构策略：当前接受的 GLIBC 加载要求上限、动态库 allow-list、预期加载器、构建镜像和后续需要运行的验证环境。首次策略值需由报告支持，不能填入预设的 2.35。
2. 为报告超过上限、新增未知动态库、出现私有路径和架构不符编写失败用例。报告要求降低时仍需运行验证，不自动宣称扩大了正式支持范围。
3. 将当前 Ubuntu 24.04 构建放入受控容器，固定镜像摘要、Rust 1.94.0 和 binutils 工具版本。保留原生 x86_64/ARM64 Runner，不默认切换构建发行版；记录安装的 native build 工具和关键包版本，并处理容器输出目录权限与 Cargo 缓存路径。
4. `build-linux.sh` 在受控环境执行 `cargo build --release --locked --target`，保留现有功能集合。不得用禁用驱动、`target-cpu=native` 或删减依赖来绕过检查。
5. 在归档完成后扫描最终文件并执行策略检查；缓存键包含 target、Rust、镜像摘要和策略摘要，避免不同基线复用 native 编译缓存。
6. 提高要求时必须同时修改策略与 `docs/linux-runtime-compatibility.md`，写明功能/依赖原因、影响系统和验证证据。CI 对未经策略更新的上升直接失败，不自动提高策略值。

如果任务 1 已经证实新版符号完全来自可避免的编译行为，应先记录候选优化及验证成本，再确定首次策略；不要把一次构建环境偶然产生的高要求直接升级成长期支持承诺。

**验收：** 同一构建来源可以重复生成语义一致的 ABI 报告；模拟引入更高 GLIBC 需求会阻止发布。构建镜像固定只限制环境漂移，不宣称全部系统包与产物已做到字节级可复现。

## 任务 4：生成兼容旧客户端的 v2 channel manifest

**文件：**

- 修改：`scripts/release/generate-channel-manifest.py`。
- 修改：`scripts/release/test-channel-manifest.sh`。
- 修改：`scripts/release/check-assets.sh`。
- 修改：`scripts/release/test-pages.sh` 的 fixture 构造。

**schema 2 的单个 Linux asset 契约示例：**

```json
{
  "url": "https://github.com/yelog/lazydb/releases/download/v0.2.0/lazydb_0.2.0_x86_64-unknown-linux-gnu.tar.xz",
  "sha256": "3c7f1a9b35d5bc2206b82d6d881c63d4637a47579a714c92de5b32ee336473f1",
  "requirements": {
    "libc": {
      "family": "glibc",
      "min_version": "2.38"
    }
  }
}
```

示例仅表达已检查的 0.2.0 x86_64 ELF 加载要求，不是新发布版本的预设策略；完整 v2 manifest 保留 v1 的身份、版本及资产字段，将顶层 `schema` 设为 2。macOS 和 Windows asset 不添加 Linux 要求。

**步骤：**

1. 在生成器中复用同一份 checksum/身份校验逻辑，增加明确的 `--schema 1|2` 选项；默认仍为 1，避免现有调用方隐式变化。
2. v2 生成时必须读取两个架构各自的 ABI 报告，并验证版本、target、归档名、归档摘要与实际资产完全一致。
3. 校验报告文件本身已包含在本次 `SHA256SUMS` 中。报告与二进制的来源必须一致，不能只因为文件名吻合就采用。
4. v1 输出保持现有字段集合，不增加 `requirements`，不放宽旧客户端的未知字段限制。
5. 新增测试：逐架构要求不同、报告缺失/过期/调换、摘要错误、无效版本、Linux 要求缺失、非 Linux 误加要求、两种 schema 的 URL 与摘要一致。
6. 更新发布资产检查，要求新发布版本包含两个 ABI 报告；历史发布重新组装时通过明确的 legacy 参数处理，不关闭新发布检查。

**验证命令：**

```sh
sh scripts/release/test-channel-manifest.sh
```

预期：schema 1 的既有契约继续成立；schema 2 对不完整或不匹配的报告拒绝生成。

## 任务 5：为 POSIX 安装器增加下载前预检与具体失败提示

**文件：**

- 修改：`install.sh`。
- 修改：`pages/install-core.sh`。
- 修改：`scripts/release/test-installer.sh`。
- 修改：`scripts/release/test-pages.sh`。

**步骤：**

1. 在 fixture 中为 v1、v2、404 回退和不同 glibc 版本增加测试；记录 curl 请求，证明不兼容时没有请求二进制归档。
2. 请求 v2 并严格校验身份、版本、目标及 requirements；仅明确 404 可回退 v1，其余错误保留原因并退出。
3. Linux 通过 `getconf GNU_LIBC_VERSION` 读取版本，必要时使用已经要求存在的 Python `os.confstr('CS_GNU_LIBC_VERSION')`；不执行 `ldd` 解析不受控输出，也不要求用户新增 readelf。
4. 数字比较当前版本和资产要求。版本不足时退出，并输出 release、target、当前 glibc、要求版本和可执行的处理建议；不建议替换系统 libc。
5. 无法确定版本与明确检测到非 glibc 要区分：检测未知时提示预检未完成，继续由 staged binary 检查兜底；确认是 musl 时说明当前 GNU 资产不适用。
6. staged binary 失败时保留原始 stderr、退出状态和上下文。提示不应误称所有失败都是 glibc 问题；动态库缺失、权限及架构错误仍可能发生。
7. 两份安装器保持行为一致，继续在任何激活或状态发布前完成验证；失败时不改动已有 launcher、`current`、`install.json` 和 shell profile。临时目录与锁按既有机制清理。
8. 不重构整个安装器生成体系；本次仅同步必要逻辑，并以同一组 fixture 覆盖 root installer、Pages core 和生成的单文件入口。

**关键测试矩阵：**

| 场景 | 预期 |
| --- | --- |
| 当前版本低于要求 | 下载归档前失败，旧安装保持可用。 |
| 当前版本等于/高于要求 | 继续校验摘要、解压并执行 staged binary。 |
| v2 404，v1 有效 | 提示 legacy 预检不可用，继续既有安装保护。 |
| v2 超时/403/5xx/格式错误 | 报错，不回退。 |
| getconf 不存在但 Python 能读取 | 正常预检。 |
| 检测完全不可用 | 明确提示未知，保留实际启动检查。 |
| glibc 通过但其他动态库缺失 | 展示实际加载器错误，不发布安装状态。 |
| macOS | 不做 Linux 检测，既有路径正常。 |

**验证命令：**

```sh
sh scripts/release/test-installer.sh
sh scripts/release/test-pages.sh
```

## 任务 6：让 Rust 更新器使用同一兼容契约

**文件：**

- 修改：`src/update.rs`。
- 新建：`src/update/runtime.rs`，放置版本解析和兼容判断，避免继续扩大主文件。
- 修改：`tests/update_native.rs`。
- 复用：`InstallationProbe`、`UpdateHttpClient` 以及现有 fake probe/http。

**步骤：**

1. 增加 v2 解析，同时继续接受 v1。`ManifestAsset` 增加可选 requirements，使用 `serde(default, skip_serializing_if = "Option::is_none")`；验证函数要求 v2 Linux 有有效要求，并继续拒绝未知字段。
2. `fetch_manifest()` 优先请求 `/v2/{channel}.json`。为 HTTP 404 建立可测试的明确判断路径；可以增加 trait 的默认方法封装状态识别，不把所有网络错误都解释成不存在。
3. 在 `src/update/runtime.rs` 中实现纯数字版本解析、比较和 glibc 契约检查，通过现有 `InstallationProbe.command_output()` 注入检测结果，不新增 libc 生产依赖或不必要的 unsafe FFI。
4. `inspect_installation()` 对存在更新且已确认不兼容的资产返回现有 `UpdateStatus::Error`，在 action 给出具体原因；不再显示可以直接更新。
5. `apply_native_update_with_progress()` 在 `download_with_progress()` 前再次验证，防止调用方绕过检查；检查信息未知时保留 staged binary 验证。
6. 保持既有 update JSON 的七个顶层字段和状态契约，本次不扩展 TUI 状态机。机器输出的警告或原因使用既有 action/stderr 规则，不能污染 JSON stdout。
7. 改善 `run_staged_version()`：子进程非零退出时保留状态及有界 stderr，复用项目已有终端文本清理能力；读取和清理 staging 的错误路径保持完整。
8. 使用 fake HTTP 的归档请求计数验证预检拒绝发生在下载前。snapshot 比较 `current`、launcher、安装状态字节内容和配置，验证拒绝及启动失败均不损坏现有安装。
9. 更新 integration fixture 的 HTTP 服务，使其能处理 v2 路径及仅 404 的回退，不再假定永远只有一次 `/stable.json` 请求。

**必须覆盖：**

- 两个架构使用各自 requirements，不能取全平台最大值。
- 等于下限、低于下限、版本格式损坏、未知 libc、历史 v1。
- v2 404 可回退；无效 v2 和其他 HTTP 错误不可回退。
- `update --check --json` 保持单行、七字段、无额外 stdout。
- 旧 manifest 仍由当前 v1 parser 接受，非原生安装仍提示通过原包管理器更新。
- 下载前拒绝、下载失败、摘要错误、staged binary 失败均保留旧安装。

**验证命令：**

```sh
cargo +1.94.0 test --locked --lib update::
cargo +1.94.0 test --locked --test update_native
```

## 任务 7：建立与产物要求对应的运行边界验证

**文件：**

- 新建：`scripts/release/smoke-linux-runtime.sh`。
- 新建：`scripts/release/test-linux-runtime-smoke.py`，使用真实 release binary 的 CLI 测试驱动。
- 修改：`scripts/release/linux-compatibility-policy.json`，记录有依据的运行镜像。
- 修改：`.github/workflows/release.yml`，增加发布前兼容验证 job。

**步骤：**

1. 消费已经生成的最终归档，不在测试环境重新编译 LazyDB。核对 archive/binary 摘要与 ABI 报告一致。
2. 为每个架构安排三个有明确目的的环境：报告声明的运行边界、一个较新且受维护的系统、一个已知低于要求的系统。测试镜像由报告与策略选出，不由 `home` 的版本决定。
3. 如果常见发行版没有恰好对应的 glibc 版本，使用隔离的匹配运行镜像或 sysroot 验证；不能因为 Ubuntu 24.04 通过，就声称 2.38 边界已经通过。测试用旧系统镜像只用于隔离验证，不成为用户生产环境建议。
4. x86_64、ARM64 优先原生运行。确有 Runner 限制时可以增加 QEMU 检查，但报告中区分模拟与原生证据，正式支持声明不能混淆两者。
5. 正向场景至少验证：`version --json`、`doctor --json`、SQLite 查询、使用本地 fixture manifest 的 `update --check`。最后一项同时覆盖本机版本检测和 Rust 子进程启动回退路径。
6. 数据库 smoke 使用隔离的 `LAZYDB_CONFIG_HOME`，复用现有 `tests/connections_cli.rs` 的命令形态：

   ```sh
   "$binary" connections add --name runtime-smoke --url 'sqlite::memory:' \
     --scope global --project "$project_dir" --json
   "$binary" agent query --connection runtime-smoke --project "$project_dir" \
     --sql 'SELECT 1 AS value'
   ```

   测试驱动创建临时项目和 `.git` 目录，解析 JSON，确认查询成功且返回值为 1；不能只判断进程没有崩溃。
7. Doctor 对密钥环、桌面环境等可选能力的 warning 不作为启动失败，但需验证 JSON 合法且必需功能正常。可选 Oracle 客户端的限制单独记录，不把全部客户端版本承诺纳入 glibc 扫描结果。
8. 反向场景在低于要求的系统上验证安装器明确拒绝，直接启动失败不会破坏安装。若弱节点可能触发加载器差异，专门在缺少该节点的环境验证回退。
9. 为最终归档生成独立的运行证据 JSON：version、target、archive SHA-256、binary SHA-256、实际 glibc、镜像摘要、测试项及结果。不要在运行后修改已经纳入 checksum 的 ABI 报告。

**验收：** 每条对外声明的最低运行边界都有证据。若边界验证失败，查明遗漏要求或修正构建；不得仅降低元数据数值使检查通过。若暂时无法验证精确边界，文档明确标注 ELF 要求与已验证环境之间的差距。

## 任务 8：把 ABI 报告和 v2 元数据接入 Release / Pages

**文件：**

- 修改：`.github/workflows/release.yml`。
- 修改：`.github/workflows/pages.yml`。
- 修改：`scripts/release/assemble-pages.sh`。
- 修改：`scripts/release/check-assets.sh`。
- 修改：`scripts/release/test-pages.sh`。
- 修改：`scripts/release/test-online-install.sh`、`scripts/release/smoke-online-install.sh`。
- 修改：`.github/workflows/ci.yml`，接入新脚本及契约检查。

**顺序约束：**

```text
构建 -> 归档 -> 扫描/策略检查 -> 归档运行验证 -> Linux 打包
     -> 汇总 ABI 报告及运行证据 -> SHA256SUMS -> 资产检查/attestation
     -> 发布 Release -> 从已校验资产生成 v1 与 v2 -> Pages 部署 -> 线上验证
```

**步骤：**

1. 给每个 Linux target 上传固定命名的报告 `lazydb_VERSION_TARGET.runtime.json` 和运行证据文件；所有发布所需 JSON 纳入摘要和来源证明。
2. `publish` 明确依赖兼容验证成功；任一架构缺报告、报告不匹配、运行失败或策略失败都阻止发布。
3. `assemble-pages.sh` 同时生成当前 channel 的 v1、v2；扩展允许发布的文件列表，只增加两个 v2 JSON，不把二进制搬到 Pages。
4. 部署 stable 时保留 beta 的 v1、v2，反之亦然。v2 的历史缺失只能按迁移规则处理；不能复制另一个版本的要求，也不能在其他下载错误时悄悄丢失相反 channel。
5. Pages 线上验证增加 Linux 两种架构及受支持边界环境。当前不兼容系统可以用作拒绝行为检查，不能强制要求其安装成功。
6. 新增测试：重复组装内容稳定、相反 channel 保留、首次部署仅一个 channel、v1/v2 URL 和摘要一致、缺失新报告阻断、历史 legacy 输入受控。
7. 验证当前 PowerShell 安装器仍能消费 v1；Windows/macOS 不新增 Linux 检测逻辑或依赖。

**验证命令：**

```sh
sh scripts/release/test-pages.sh
sh scripts/release/test-online-install.sh
sh scripts/release/test-distribution.sh
```

## 任务 9：从实际 ELF 要求生成 Linux 包依赖

**文件：**

- 修改：`packaging/nfpm.yaml`。
- 修改：`scripts/release/package-linux.sh`。
- 新建：`scripts/release/test-linux-packages.sh`。
- 修改：`.github/workflows/release.yml` 的 package job。

**步骤：**

1. 将 `package-linux.sh` 的内部调用契约明确为 `VERSION ARCH BINARY RUNTIME_REPORT OUTPUT_DIR`，同步 Release 中唯一的生产调用方；先校验 report 的 binary 摘要与输入匹配。
2. 基于报告生成目标格式的依赖，不在模板里硬编码 2.35。DEB 的 `libc6`、RPM 的 glibc/符号依赖、Arch 的 glibc 约束分别使用各自包管理器支持的表达式。
3. 核对 `libgcc_s`、`libm` 和其他必需系统库的实际归属。DEB 在对应基线环境使用依赖分析工具验证，RPM 检查 ELF 自动依赖或等价的显式符号要求，Arch 检查包元数据；不能只补一个 libc 名称后宣称所有依赖完整。
4. 对 RPM/Arch 的发行版布局差异使用隔离的包查询或安装测试。Linux GNU 归档仍不因此承诺所有发行版兼容。
5. 使用真实包执行 `dpkg-deb -f`、`rpm -qp --requires`、解包读取 `.PKGINFO`；确认架构、版本、依赖与报告一致。
6. 保持密钥环服务、数据库服务、字体和可选 Oracle 客户端的可选性质；只声明程序正常加载和必需功能实际需要的库。

**验收：** 不兼容 libc 的系统在包管理器安装阶段获得明确拒绝；兼容环境能够安装并运行与归档相同的二进制。

## 任务 10：安排协议迁移和完整回归

**文件：**

- 修改：`scripts/release/test-channel-manifest.sh`、`scripts/release/test-pages.sh`。
- 修改：`scripts/release/test-installer.sh`、`tests/update_native.rs`。
- 修改：`scripts/release/test-windows-installer.ps1`，仅增加必要的旧契约兼容断言。
- 修改：`docs/releasing.md`，补充 v1/v2 双发布和历史 channel 操作。

**迁移步骤：**

1. 先让服务端代码具备双版本生成能力，保持现有 v1 地址和结构不变。
2. 新发布从同一组归档、摘要和 ABI 报告生成 v1/v2，随后部署新安装器。新客户端上线前检查两条 channel 的状态和相反 channel 的保留行为。
3. 对尚无 ABI 报告的历史版本，保留明确的 404 -> v1 路径和 staged binary 保护。若需要提前补齐历史 v2，先审计原始归档并验证，再作为独立的元数据部署执行；不得重新编译或替换原二进制。
4. 使用已发布的旧版二进制，在兼容环境中真实读取新生成的 v1，确认未知字段限制没有被触发。仅用新客户端测试旧 JSON 不足以证明旧客户端兼容。
5. 验证旧客户端更新到新版本的实际路径：满足新产物要求时可激活；不满足时旧客户端在 staged 检查退出并保留旧版本。旧客户端无法获得新增的下载前检查，这个限制需写清楚。
6. 验证新安装器/新客户端读取历史 v1、正常 v2、损坏 v2、不兼容 v2 的行为；保留首次安装、重复安装、升级和更新失败回归。

**完整验证命令：**

```sh
python3 -B scripts/release/test-linux-runtime.py
sh scripts/release/test-distribution.sh
cargo +1.94.0 fmt --all -- --check
cargo +1.94.0 clippy --locked --all-targets --all-features -- -D warnings
cargo +1.94.0 test --locked --all-targets --all-features
```

另外运行两个架构的归档兼容 job、Linux 包检查、macOS 发布 smoke、Windows 两种 PowerShell installer job。数据库适配器现有 CI 按项目原流程执行，不用新增无关测试扩大本次范围。

## 任务 11：整理文档、验收证据和交付范围

**文件：**

- 修改：`README.md`。
- 修改：`docs/releasing.md`。
- 完成：`docs/linux-runtime-compatibility.md`。
- 发布准备时修改：`CHANGELOG.md`，具体版本由独立发布任务确定。

**步骤：**

1. 文档分别列出当前发布包加载要求、已验证发行版/架构、可选客户端限制和构建来源；避免写成“Linux 均支持”或只写 Ubuntu 版本。
2. 提供查看当前 libc、安装失败后的处理方式和源码构建入口。没有可用历史兼容产物时，不建议用户切到一个未经验证的旧版本。
3. 记录新依赖导致最低版本上升时的流程：来源分析 -> 策略变更 -> 边界测试 -> 包依赖与 manifest 更新 -> 发布说明。
4. 若调查证明当前新符号来自可避免的编译行为，另列范围明确的构建基线优化任务，以完整功能和双架构验证为验收标准；不要将元数据工程和未经验证的降基线改动捆绑发布。
5. 汇总各任务实际运行结果、未验证项、报告与资产摘要。准备新补丁版本的可审阅变更，不替换已发布 0.2.0 资产；只有获得发布任务授权后才 commit/tag/push/部署。

## 最终验收清单

- [ ] glibc 限制来自最终 ELF 和必要运行验证，非 `home` 或 Runner 版本。
- [ ] 两个架构独立报告，正确处理版本节点标志与弱符号绑定的差异。
- [ ] 每份报告、运行证据和 manifest 都能绑定准确的发布资产摘要。
- [ ] 未经策略更新的要求上升或依赖新增会阻止发布。
- [ ] 旧 v1 manifest 结构不变，已发布旧客户端能够继续解析。
- [ ] v2 仅在明确 404 时回退；其他错误不会绕过验证。
- [ ] 不兼容系统在新安装器和更新器下载归档前收到具体提示。
- [ ] 未知检测结果不会被误标为兼容；实际启动检查仍然存在。
- [ ] 任何拒绝或 staged binary 失败都保留已有安装和用户配置。
- [ ] 声明的运行边界和较新系统通过最终归档的正向测试。
- [ ] 低于要求的环境通过可解释的负向测试。
- [ ] Linux 包依赖与报告一致；macOS、Windows 既有路径正常。
- [ ] 文档区分静态加载要求、实际验证范围与可选功能要求。
- [ ] 完成实施不意味着已经获得对外发布或替换资产的授权。
