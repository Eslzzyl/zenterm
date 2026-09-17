# Zenterm 审计问题清单

日期：2026-09-17。范围：工作区结构、会话生命周期、输入与终端协议、图像及 GPU 渲染、配置持久化和发布检查。

本清单来自代码路径审查。下文的“代码已确认”表示触发条件与代码行为能够直接对应；崩溃、卡顿等最终运行表现仍需复现。审计期间未修改程序代码，也未执行 Cargo 编译、测试、跨平台运行或依赖漏洞库扫描。

## 优先处理

### ZT-04：图像协议缺少统一的输入、解压、累积和布局上限（代码已确认；运行影响待复现）

Kitty Deflate 在像素数检查前使用无输出上限的解压；PNG 等图像也在解码后才检查像素数。多块传输、未结束的 OSC/APC 会持续累积数据。帧尺寸检查和分块日志还直接计算 `w * h * 4`，存在整数溢出路径。Sixel 按协议尺寸直接创建 RGBA 图像，没有对应的像素预算；Kitty placement 的 `columns/rows` 也会直接参与 `Vec::with_capacity`，未先限制到网格范围。`ImageCache` 的 320 MiB 预算在图像解码和分块累积之后才起作用，不能约束这些峰值。

依据：[解压与后置检查](crates/zenterm-term/src/image/kitty.rs#L679-L735)、[帧尺寸](crates/zenterm-term/src/image/kitty.rs#L763-L788)、[Sixel 分配](crates/zenterm-term/src/image/sixel.rs#L203-L309)、[placement 分配](crates/zenterm-term/src/image/placement.rs#L171-L197)、[分块累积](crates/zenterm-term/src/image/kitty.rs#L1166-L1255)、[OSC 缓冲](crates/zenterm-term/src/term/terminal/mod.rs#L311-L324)、[APC 缓冲](crates/zenterm-term/src/term/terminal/protocol.rs#L54-L63)。建议设置协议字节与解码像素预算，按预算解压，限制 placement，并使用检查过的乘法。

### ZT-05：GPU 实例截断后仍使用未截断的绘制范围（代码已确认；运行影响待复现）

实例缓冲固定容纳 40,000 个实例。超出时 `update_instances` 只上传前 40,000 个，而 `atlas_ranges` 仍由原始实例列表生成，绘制时直接使用原范围。足够大的终端网格或多窗格可使范围越过已上传实例；是否表现为 wgpu 验证错误或画面缺失需运行验证。

依据：[缓冲容量](crates/zenterm-render/src/lib.rs#L182-L189)、[截断](crates/zenterm-render/src/lib.rs#L583-L599)、[范围绘制](crates/zenterm-render/src/lib.rs#L647-L699)、[两者分别传入](crates/zenterm-render/src/callback.rs#L441-L469)。建议按实际实例数扩容，或对上传与全部绘制范围使用同一裁剪结果。

## 其他功能与可靠性问题

### ZT-06：PTY 排空没有单帧工作量预算（风险，待压测）

后台读取线程使用 256 项有界通道，控制了排队量；主线程每帧仍循环读取到通道暂时为空，随后把所有数据一次性交给终端解析。持续高速输出时，单帧耗时缺少上界，可能拖慢其他会话和界面响应。

依据：[有界通道](crates/zenterm-pty/src/lib.rs#L136)、[主线程排空](crates/zenterm-ui/src/session/pty.rs#L29-L62)、[遍历全部会话](crates/zenterm-ui/src/app/session_lifecycle.rs#L97-L105)。建议压测持续输出，并考虑按字节数或时间片处理。

### ZT-07：Windows 覆盖写入存在丢失旧文件的窗口（代码已确认）

配置和布局文件先写 `*.tmp`。重命名失败时，Windows 分支尝试删除原文件后再次重命名；若进程此时中止或第二次重命名失败，旧文件已被移除。“原子写入”的注释不适用于这条路径。

依据：[配置写入](crates/zenterm-config/src/config.rs#L174-L194)、[布局写入](crates/zenterm-ui/src/layout_io.rs#L217-L235)。建议采用可保证替换语义的 Windows 文件操作，并添加失败注入测试。

### ZT-08：`persist_layout = false` 在退出时仍写布局（代码已确认）

逐帧持久化会检查该开关，但 `on_exit` 无条件调用 `persist_layout_now`；后者写入 `dock.json` 与 `sessions.json`。因此该配置不能阻止正常退出时的保存。

依据：[逐帧检查](crates/zenterm-ui/src/app/persistence.rs#L17-L31)、[退出调用](crates/zenterm-ui/src/app/mod.rs#L569-L578)。建议让退出路径遵守同一开关。

### ZT-09：背景图异步加载可能被旧请求覆盖（代码已确认；触发取决于完成顺序）

每次路径变化都会启动新的解码线程，但完成时没有核对当前配置路径或请求版本。旧图片若在新图片加载或“清除背景”之后完成，仍可把数据写回共享状态。

依据：[配置变更](crates/zenterm-ui/src/app/config.rs#L168-L190)、[线程完成写回](crates/zenterm-ui/src/app/mod.rs#L722-L762)。建议加入请求代次或取消机制，并测试快速切换、清除背景的场景。

### ZT-10：背景图相对路径与配置说明不一致（代码已确认）

配置类型说明相对路径以配置文件目录为基准，加载器却直接 `ImageReader::open(&path)`，因此实际以进程工作目录为基准。

依据：[配置说明](crates/zenterm-config/src/background.rs#L28-L33)、[打开文件](crates/zenterm-ui/src/app/mod.rs#L741)。建议在加载前依据 `Config::path()` 解析相对路径。

### ZT-11：恢复的 `cwd` 只更新元数据，未设置新 shell 的工作目录（代码已确认；产品意图待确认）

恢复时先构造 `TerminalSession` 并启动默认 shell，之后才从 `sessions.json` 给会话的 `cwd` 字段赋值。PTY 启动命令没有使用保存的目录。若“恢复会话”预期包含 shell 工作目录，当前行为与预期不符。

依据：[PTY 启动](crates/zenterm-pty/src/lib.rs#L104-L123)、[会话构造](crates/zenterm-ui/src/session/new.rs#L69-L102)、[元数据赋值](crates/zenterm-ui/src/app/mod.rs#L270-L284)。

### ZT-12：重启提示按非默认值判断，可能长期误报（代码已确认）

`Config::diff_to` 只要变更前后任一窗口配置的标题或装饰不同于默认值，就设置 `needs_restart`，即使用户没有修改这些字段。设置页据此显示重启提示。

依据：[判定](crates/zenterm-config/src/config.rs#L210-L221)、[窗口条件](crates/zenterm-config/src/window.rs#L65-L77)、[设置页提示](crates/zenterm-ui/src/settings.rs#L313-L326)。建议按本次变更的字段判断。

### ZT-16：字体热更新没有同步终端的物理 cell 尺寸（代码已确认）

修改字体后，UI 会更新 `session.cell_width/cell_height`，但没有同步 `Terminal::cell_pixel_width/height`。如果行列数没有变化，后续 resize 会提前返回；即使发生 resize，`Terminal::resize` 也只更新总像素尺寸，不更新 cell 像素尺寸。Kitty/Sixel 图像布局及终端尺寸查询因此可能继续使用旧字体指标。

依据：[字体配置写回](crates/zenterm-ui/src/app/config.rs#L127-L146)、[DPI 更新](crates/zenterm-ui/src/session/reinit.rs#L56-L60)、[Terminal resize](crates/zenterm-term/src/term/terminal/grid.rs#L18-L34)。建议把 cell 物理尺寸更新收敛到统一的字体/DPI 重建流程，并补充热更新回归测试。

### ZT-17：光标闪烁间隔的单位在文档与实现之间不一致（代码已确认）

配置和字段注释把 `blink_interval` 定义为帧数，并以 60 FPS 计算；渲染和重绘调度却把它当作毫秒。默认值 `30` 的实际闪烁节奏与文档描述不一致，且最小周期逻辑又额外使用了 100 ms 下限。

依据：[配置定义](crates/zenterm-config/src/cursor.rs#L28-L34)、[重绘调度](crates/zenterm-ui/src/app/mod.rs#L411-L419)、[渲染相位](crates/zenterm-ui/src/session/render/mod.rs#L170-L188)。建议统一为明确的时间单位，或恢复真正按帧计数的实现。

### ZT-18：终端图像的 GPU source 引用在清空 placement 后可能滞留（风险，待压测）

`Terminal::resize` 会清空 image placement，但 `TerminalSession.image_sources` 只在 session `Drop` 或 CPU 图像缓存驱逐时释放。反复 resize、覆盖或重建图像时，GPU 图像 source 引用可能长期保留，直到会话关闭或缓存驱逐。

依据：[resize 清空 placement](crates/zenterm-term/src/term/terminal/grid.rs#L23-L34)、[source 释放路径](crates/zenterm-ui/src/glyph_cache.rs#L258-L304)、[仅在 Drop 全量释放](crates/zenterm-ui/src/session/types.rs#L275-L280)。建议让 placement 生命周期和 source 引用生命周期显式关联，并增加重复 resize/图像覆盖测试。

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

### ZT-13：终端核心间接依赖 UI 事件类型（架构债务）

九个 crate 的宏观职责划分清晰，但 `zenterm-term` 为获取 Kitty 键盘标志依赖 `zenterm-input`；`zenterm-input` 又以 `egui::Event` 为公开映射输入。这使终端核心的依赖图包含 UI 框架，降低了独立复用与无 UI 测试的便利性。文件长度本身不足以判定设计问题，优先关注这条依赖边界。

依据：[term 依赖](crates/zenterm-term/Cargo.toml#L8)、[input 依赖](crates/zenterm-input/Cargo.toml#L8)、[标志类型使用](crates/zenterm-term/src/term/terminal/effects.rs#L69-L71)。可考虑把协议标志类型放到与 UI 无关的模块。

### ZT-14：发布检查未覆盖工作区单测，普通推送没有该 CI 检查（代码已确认）

唯一工作流只在 `v*` 标签推送或手动派发时启动；其中 Clippy 与测试都仅指定 `zenterm` 包。其他库 crate 的单元测试不会由这条 `cargo test --package zenterm` 命令执行；项目虽另有终端视觉测试工具，它也没有接入该工作流。不能据此推断库测试当前是否通过。

依据：[触发器](.github/workflows/release.yml#L1-L8)、[检查命令](.github/workflows/release.yml#L71-L105)、[视觉测试说明](terminal-render-test/README.md#L1-L21)。建议增加日常推送/PR 检查，并覆盖工作区测试。

### ZT-15：架构文档和配置文档存在明显漂移（代码已确认）

`docs/architecture.md` 和 `docs/components.md` 的目录图仍展示根目录 `src/` 与 `alacritty/`、`wezterm/`，依赖表还保留 `copypasta` 等已不符合当前 manifest 的内容，而当前实现为 `crates/` 工作区。配置文档还把窗口 padding 默认值写成 `8,6`，代码默认值为 `12,10`。这些差异会误导模块定位、配置排查和维护。

依据：[旧目录图](docs/architecture.md#L155-L198)、[组件文档](docs/components.md#L119-L126)、[当前工作区](Cargo.toml#L1-L13)、[配置文档](docs/usages/config.md#L14-L20)、[代码默认值](crates/zenterm-config/src/window.rs#L118-L130)。建议更新目录图、依赖表、配置默认值与当前数据流说明。

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
