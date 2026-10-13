# LazyDB Update CLI Progress Implementation Plan

> **For Claude:** 按下面任务逐项实施并验证；用户已要求在本会话直接实施。仅使用当前可用技能，不因缺少执行技能停止。未经用户要求不提交或推送。

**Goal:** 让 `lazydb update` 在联网前立即打印当前版本，按顺序呈现检查结果、真实下载进度、安装阶段及最终成功或失败。

**Architecture:** 保留现有 `UpdateReport` JSON 契约和最终摘要 renderer，在 CLI 的执行边界增加私有进度展示器。复用 `apply_native_update_with_progress` 的阶段及字节回调；TTY 使用单行刷新与独立心跳，普通日志仅输出关键阶段。所有等待开始前主动 flush，退出和失败时停止心跳并结束动态行。

**Tech Stack:** Rust 2024、std::io/std::sync/std::thread、现有 crossterm、现有 Tokio 和更新模块；不新增依赖。

---

## 设计依据与展示契约

目前 `src/main.rs` 等待 `update::run` 返回后才 println；`run` 安装分支使用无反馈的 `apply_native_update`，因此网络及安装全部完成前无输出。已有真实字节回调可直接复用。

按时间顺序参考输出（版本、下载量均为示意）：

```text
LazyDB update
  Current      0.1.9
[INFO] Checking for updates (stable)...
[INFO] Update available: 0.1.9 -> 0.1.10 (stable)
  Downloading [==========----------] 50%  8.0 / 16.0 MiB
  Verifying checksum...
  Extracting update...
  Verifying installation...
  Activating update...
[OK] LazyDB updated successfully
  Version      0.1.9 -> 0.1.10
  Channel      stable
```

- 核心体验是状态可见和结果可信。立即显示运行中的版本；若已安装版本与运行版本不同，现有最终报告继续保留已安装版本的事实。
- 检查及连接等待用 ASCII spinner 表示活跃；下载的百分比只来自已收到字节数/已知总大小。
- 未知或零总大小只显示已收到字节数；下载结束后仍显示校验、解压、安装验证及激活，最后才宣称成功。
- TTY 单行刷新约 10 次/秒，按终端宽度收缩，不隐藏光标。NO_COLOR 仅关闭颜色；TERM=dumb 及管道输出不用控制序列或动画。
- 非 TTY 保留阶段行和下载完成量，不为每个数据块输出一行。
- `--check` 输出检查过程与检查结果，不进入安装；包管理器安装继续显示原有操作指引。
- JSON 保持一行报告，无进度或装饰输出。
- 失败时清理当前动态行，显示完整原因并返回非零退出码。检查失败与安装失败采用不同标题。

## Task 1：实现私有 CLI 进度展示器

**Files:** Create `src/update/cli_progress.rs`; Modify `src/update.rs` 模块声明。

1. 将阶段名称、字节格式及真实百分比渲染为纯函数。
2. 实现拥有 stdout writer 的状态对象：静态日志、动态行、宽度限制、flush、阶段切换、结束清理。
3. 用 RAII 管理 TTY 心跳线程；回调共享 Mutex 状态，每次刷新限频，销毁时发出停止信号并 join。
4. 使用可捕获 writer 的单元测试验证未知大小/零大小、不超过 100%、阶段顺序、下载完成量和控制序列降级。

**Verification:** `cargo test --lib update::cli_progress::tests`；预期全部通过。

## Task 2：接入命令执行顺序与失败输出

**Files:** Modify `src/update.rs` 的 `run`; Modify `src/main.rs` 的 Update 分支（若错误输出需要显式呈现）。

1. 人类输出在任何联网/路径发现之前打印当前版本并 flush。
2. 启动检查状态；检查结束后清除动画。仅实际自动安装前输出可用更新摘要（不重复输出建议再次运行同一命令）。
3. 安装分支切换到 `apply_native_update_with_progress`，将现有回调交给展示器。
4. 保留已有报告及前后版本语义；JSON 分支完全静默。
5. 安装中出现任何错误都结束动画并附上清晰的失败标题、完整错误链。检查 error 的文本模式返回非零；JSON 继续返回原有 error 报告契约。

**Verification:** `cargo test --lib update::tests`；预期现有摘要、JSON、版本及 native 安装测试通过。

## Task 3：验证首次输出时机与 CLI 兼容性

**Files:** Modify `tests/update_native.rs`; Review `tests/update_http.rs`。

1. 为本地 manifest fixture 增加延迟放行门，子进程 stdout 经管道逐行读取。
2. 在 fixture 尚未响应时断言已收到当前版本及 Checking 行；这直接验证立即反馈而非仅验证最终文本。
3. 放行后验证可用更新检查结果、最新状态和错误状态；`--check` 不修改临时安装。
4. 保留并加强单行 JSON、无 ANSI、无进度输出的断言。
5. 使用现有 native 安装单元 fixture 检查真实进度回调序列与失败反馈。

**Verification:** `cargo test --test update_native --test update_http --test update_reducer`；预期全部通过。仅使用本地 HTTP 和临时目录，不执行开发者已安装程序的真实更新。

## Task 4：最终验证与交付

1. `cargo fmt --check`、`git diff --check`。
2. 检查 diff 范围、无新增依赖、无用户改动覆盖。
3. 将实际运行结果、终端预览情况及未验证限制记录在最终回复中。

**Acceptance:** 首次反馈发生在网络响应之前；检查结果先于下载；TTY 下载有真实进度；未知大小无伪造百分比；安装成功与检查最新明确区分；失败退出非零；JSON 契约保留；等待心跳无遗留线程。

## 实施与验证记录

上述四项任务已完成，未提交或推送。

- `cargo test --lib update::`：43 passed，包含展示器、真实安装阶段顺序、已有摘要语义及 TUI 更新测试。
- `cargo test --test update_native --test update_http --test update_reducer`：24 passed，1 ignored。跳过项是原有依赖外网及 macOS 系统代理的真实下载测试。
- CLI 延迟 manifest fixture 验证：在网络响应放行前已读取到当前版本及 Checking 行；检查模式不更改临时安装。
- 实际 CLI 经 PTY 运行 `update --check`，使用本地延迟 manifest，在 80 列正常终端、40 列 NO_COLOR 终端及 40 列 TERM=dumb 下均成功。正常终端等待期间持续刷新；NO_COLOR 无颜色代码；dumb 无 ANSI。
- 窄终端下载采用百分比及字节数优先的紧凑布局，尺寸未知时无百分比；单元测试覆盖该退化规则。
- 独立复核指出窄终端信息截断和输出清理错误遮盖业务错误，均已修正。
- `cargo fmt --check` 和 `git diff --check` 通过。
- 未运行开发者已安装二进制的真实在线自更新；完整安装阶段由临时目录中校验发布包的测试覆盖，实际 CLI 终端验证覆盖检查路径。
