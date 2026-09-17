# Zenterm 审计问题清单

日期：2026-09-17。范围：工作区结构、会话生命周期、输入与终端协议、图像及 GPU 渲染、配置持久化和发布检查。

本清单来自代码路径审查。下文的“代码已确认”表示触发条件与代码行为能够直接对应；崩溃、卡顿等最终运行表现仍需复现。初始审计阶段未修改程序代码，也未执行 Cargo 编译、测试、跨平台运行或依赖漏洞库扫描；进入修复阶段后的验证结果以各项提交记录为准。

## 优先处理

## 其他功能与可靠性问题

### ZT-19：会话构造逻辑重复，`TerminalSession` 职责过重（架构债务）

`TerminalSession` 同时管理 PTY、终端核心、输入状态、鼠标选择、通知、剪贴板、字体/图像缓存和 GPU 实例缓存；创建逻辑还分别出现在应用初始化、普通新建标签和新建工作区路径中。未来修改启动参数、配置同步或资源初始化时容易产生分叉行为。

依据：[会话聚合状态](crates/zenterm-ui/src/session/types.rs#L94-L207)、[普通创建路径](crates/zenterm-ui/src/app/session_lifecycle.rs#L15-L39)、[新工作区创建路径](crates/zenterm-ui/src/app/dock.rs#L132-L158)。建议引入统一的 session factory，并逐步拆分运行时、视图缓存和通知状态。

### ZT-20：PTY 创建失败会直接 panic，缺少用户可恢复路径（代码已确认）

`TerminalSession::new` 对 PTY 创建使用 `expect`。默认 Shell 不存在、PTY 初始化失败或平台环境不满足时，应用会在创建会话阶段崩溃，无法显示错误并允许用户调整配置或重试。

依据：[PTY 创建](crates/zenterm-ui/src/session/new.rs#L69-L95)。建议让会话构造返回 `Result`，由应用层显示错误并决定是否关闭当前标签或回退到可用 Shell。

### ZT-21：Dock 每帧通过 JSON 序列化检测布局变化（风险，待压测）

Dock 渲染前后都会对完整 `DockState` 执行 `serde_json::to_vec`，仅为判断本帧是否发生布局变化。工作区、分屏和标签数量增加后，这个操作会进入 UI 热路径，并且序列化失败会 panic。

依据：[布局变化检测](crates/zenterm-ui/src/app/dock.rs#L314-L347)。建议使用 egui_dock 的变更信号或在明确的交互回调中标记 dirty，避免每帧序列化完整状态。

## 结构与验证缺口

## 已确定的行为与跨平台约束

本节记录进入修复阶段时采用的产品行为。协议语义参考 [iTerm2 Inline Images Protocol](https://iterm2.com/documentation-images.html)、[Kitty Graphics Protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/) 和 [Kitty File Transfer Protocol](https://sw.kovidgoyal.net/kitty/file-transfer-protocol/)。

### OSC 1337 File

- `inline=1`：在终端内显示文件，沿用当前图像渲染管线。
- `inline=0` 或省略：保存到系统 Downloads 目录；只接受安全文件名，自动处理重名，不覆盖已有文件，也不回退写入进程当前目录。
- Downloads 目录必须通过平台能力解析，不能拼接固定的 Unix 路径：Windows 使用 Known Folder，macOS 使用用户 Downloads 目录，Linux/Unix 优先使用 XDG user-dir 配置并回退到用户 Downloads 目录。
- 目录无法解析、无写权限或文件名无效时，报告失败并放弃保存；不能静默换写其他目录。
- 文件名处理需要同时覆盖 POSIX `/`、Windows `\\`、保留设备名、控制字符、大小写不敏感重名和 Unicode 规范化等情况。

### Kitty Graphics `t=f` / `t=t`

- `t=f`：保留兼容读取行为，但只读取普通文件；拒绝设备、管道、套接字等特殊文件，增加文件字节数、解压输出、解码像素和分块累积上限。
- `t=t`：只允许删除 Kitty 客户端明确创建的临时文件。实现应使用 Zenterm 专用临时命名空间或等效归属标记，不能仅凭 `/tmp`、`TMPDIR` 等目录前缀删除文件。
- POSIX 侧需要处理符号链接、挂载点和敏感目录；Windows 侧需要处理盘符、UNC 路径、目录分隔符、大小写不敏感比较以及 junction/reparse point，避免路径检查被绕过。
- 读取前应检查最终文件类型和规范化路径；跨平台 API 返回的路径编码失败时直接拒绝，不进行模糊转换。
- 所有尺寸计算使用 checked arithmetic；资源上限按 `usize`、GPU 最大 buffer 和平台可用内存分别校验，不能只依赖单一 320 MiB 缓存值。

### 会话恢复

- `tabs_enabled = false` 时完全忽略保存的 Dock 布局，只创建并使用单一会话 0。
- `tabs_enabled = true` 时恢复工作区和标签；保存的 `cwd` 用作新 Shell 的启动目录。
- `cwd` 必须在 PTY 创建前传入 Shell；目录不存在、不可访问或格式不适配当前平台时，记录错误并回退到平台默认工作目录，不阻止应用启动。
- Windows 需要覆盖盘符路径、UNC 路径和 PowerShell/cmd 的启动方式；POSIX 需要覆盖绝对路径、相对路径解析和权限失败。
- 恢复失败不能留下无标签运行的隐藏 PTY，也不能让界面显示的会话与键盘输入目标不一致。

### 修复阶段的验证要求

- 协议测试至少覆盖 Windows、macOS、Linux 的路径格式和临时目录规则。
- 文件测试覆盖重名、无权限、特殊文件、符号链接/junction、超大文件、损坏压缩数据和未结束分块。
- 会话恢复测试覆盖关闭/启用多标签切换、会话 0 缺失、cwd 不存在、Windows 盘符/UNC 和 POSIX 权限失败。
- 不使用当前工作目录作为跨平台兜底下载目录，不以路径字符串前缀作为唯一的文件所有权证明。

## 本轮验证边界

- `cargo fmt --all --check` 与 `git diff --check` 已通过。
- 未执行 Cargo 编译或测试、GUI/PTY 压测、跨平台检查、依赖漏洞数据库检查、GitHub Actions 远端验证。
- 本轮还确认 `cargo metadata --no-deps` 通过；未据此推断程序可编译或运行。
- 建议先为 ZT-01、ZT-02、ZT-03、ZT-04、ZT-05、ZT-16、ZT-18 建立最小复现，再修复并扩展工作区 CI 覆盖。
