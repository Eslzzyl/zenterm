# 字体渲染对比与优化结论

更新时间：2026-09-16。本文针对 Windows、DPI 125%、JetBrainsMono Nerd Font
以及当前工作区中的实际截图，分析 zenterm 的字体边缘为何比运行中的 WezTerm
更厚/更不锐利，并评估在继续使用 Rust 字体栈的前提下的优化空间。

## 结论摘要

当前最强的实测证据指向默认抗锯齿模式，而非汉字 fallback：

1. zenterm 默认使用 RenderMode::Subpixel，WezTerm 当前默认配置走
   FreeType FT_RENDER_MODE_NORMAL，也就是灰度光栅化。
2. 同一字体、字号、DPI、前景色和背景色下，zenterm 切换到 Grayscale 后，
   ASCII 与 CJK 行的截图指标都与 WezTerm 对齐；Subpixel 的 ASCII ink/mass
   约为灰度结果的 1.16/1.17 倍，并产生明显 RGB 彩边。
3. zenterm 对 Subpixel coverage 额外执行 coverage^(1/1.3)。这会提高所有
   中间 coverage，必然使细边变亮、变厚；它与默认模式差异叠加。
4. 因此不需要更换 C 字体库也有明确优化空间。第一优先级是让默认模式与
   面板/合成环境匹配，第二优先级是重新校准 gamma 与 Subpixel 合成，第三
   优先级是补齐字体 weight/style 和 cache key。

这不是“swash 一定比 FreeType 差”的证据。它证明了当前 zenterm 的默认
参数组合和 GPU 合成结果与当前 WezTerm 的默认组合不同。若目标是逐像素复制
DirectWrite/FreeType 的全部细节，纯 Rust 栈无法承诺完全一致；若目标是清晰、
稳定、无彩边的终端文字，现有 cosmic-text + harfrust + swash + zeno 足以
继续优化。

## 本机可重复检查方法

检查工具位于 tools/font-render-check/check.ps1，说明位于
tools/font-render-check/README.md。它做四件事：

1. 读取两个终端的 PNG，背景使用粗粒度颜色直方图估计，避免窗口边框被当成
   文字；
2. 按行投影自动识别文字带，默认选最后三行作为 ASCII/CJK/Mixed；
3. 用 Nearest-neighbor 放大文字行，保留 RGB 边缘的原始像素结构；
4. 计算 InkPixels、近似 CoverageMass、RgbFringe 和两张图之间的比值，写入
   JSON。

已有截图的复核命令：

    pwsh -NoProfile -File .\tools\font-render-check\check.ps1 -ZentermImage .\target\zenterm-fixture.png -ZentermGrayscaleImage .\target\zenterm-grayscale.png -ReferenceImage .\target\wezterm-controlled.png

输出在 target/font-render-check/：

- report.json：每个文字行的 ink 像素数、近似 coverage mass、RGB fringe
  和与 WezTerm 的比值；
- zenterm-ascii-zoom.png、zenterm-cjk-zoom.png、zenterm-mixed-zoom.png 等：
  自动裁剪并以 8 倍 Nearest 放大的行图；
- capture.json：使用 -Capture 时记录窗口标题、PID 和截图路径。

窗口捕获默认使用屏幕像素路径：检查器按每次运行的实际窗口矩形恢复目标窗口，
必要时把完全位于所有显示器之外的 fixture 放到当前虚拟屏幕，短暂置顶后复制
屏幕像素，再恢复非置顶状态。这个路径能捕获当前 eframe/wgpu 内容；远程桌面
导致分辨率或 DPI 变化时，矩形和行检测随截图重新计算，不依赖固定坐标。`-CaptureMethod
PrintWindow` 仍可显式使用，但对当前 zenterm 窗口可能返回黑块或背景块，不能
把这类输出作为渲染质量证据。PNG 读取与像素分析仅用于测试，不改变生产字体链路。

RgbFringe 定义为文字行内各像素 max(R,G,B)-min(R,G,B) 的累计值；灰度边缘
应接近零，LCD Subpixel 的彩边会明显增大。CoverageMass 是按背景亮度归一化
的近似覆盖量，用于识别字重变化，同时支持深色和浅色主题；它不是字体引擎的
内部 coverage，不能单独作为视觉质量分数。

## 实测结果

固定条件：

- Windows 当前 DPI：1.25；
- zenterm 物理字号：20 px；
- WezTerm：12 pt × 1.25 = 20 px；
- 字体：JetBrainsMono Nerd Font；
- 前景/背景：#f2f2f2 / #101010；
- 样本包含 ASCII、汉字和混排。

ASCII/CJK 是行级统计，包含行前的 ASCII: / CJK: 标签；单独放大图和日志
用于确认 fallback 字形。

| 样本 | InkPixels | CoverageMass | RgbFringe |
| --- | ---: | ---: | ---: |
| zenterm Subpixel / ASCII | 1116 | 691.67 | 193.73 |
| zenterm Grayscale / ASCII | 964 | 593.44 | 0 |
| WezTerm / ASCII | 964 | 593.42 | 0 |
| zenterm Subpixel / CJK 行 | 1935 | 1125.04 | 50.80 |
| zenterm Grayscale / CJK 行 | 1898 | 1098.86 | 0 |
| WezTerm / CJK 行 | 1898 | 1098.86 | 0 |

CJK 行的少量 Subpixel fringe 来自行前的 ASCII 标签；实际 fallback 汉字在
日志中被记录为 Microsoft YaHei UI + format=Alpha，放大图没有可见 LCD
彩边。当前 A/B 最有区分度的结果是：Grayscale 与 WezTerm 的 ASCII/CJK
行对齐，Subpixel 只在主字体 ASCII 上显著变厚并出现彩边。

## zenterm 当前管线

    cosmic-text / harfrust shaping
            ↓ LayoutGlyph / CacheKey
    swash + zeno rasterization
            ↓ RGBA atlas
    wgpu texture (Rgba8Unorm, Nearest)
            ↓ WGSL
    egui-wgpu target (普通 ALPHA_BLENDING)

关键实现证据：

- crates/zenterm-glyph/src/rasterize.rs:21-49：Subpixel coverage 以
  1/1.3 的幂做校正；
- rasterize.rs:92-108：主字体可以输出 Format::Subpixel，resolved fallback
  强制使用 Format::Alpha；
- atlas_impl.rs:616-625：通过 cosmic-text 先按字符 shaping；
  atlas_impl.rs:656-665：当前 125% DPI 下设置 DISABLE_HINTING；
- crates/zenterm-render/src/atlas.rs:8-13：使用非 sRGB Rgba8Unorm 保存
  coverage；atlas.rs:87-104：使用 Nearest 采样；
- crates/zenterm-render/src/shaders.rs:99-105：顶点颜色转线性空间；
  shaders.rs:183-199：Subpixel 使用 max(coverage) 作为公共 alpha，再把
  每通道颜色转回 sRGB；
- crates/zenterm-render/src/lib.rs:383-391：pipeline 使用
  wgpu::BlendState::ALPHA_BLENDING，所有 glyph 类型共用一个普通 alpha
  blend state。

当前依赖的 egui-wgpu 0.34 在可用时优先选择非 sRGB 的
Rgba8Unorm/Bgra8Unorm framebuffer，因此 shader 中的手动 sRGB 转换有其前提，
现有证据不足以把问题归结为简单的“双重 gamma”。更敏感的点是 Subpixel
同时输出逐通道颜色和公共 alpha，却仍交给普通 source-over blend；这需要对
不同背景、透明度和重叠 glyph 做单独方程验证。

Nearest 本身不是这轮问题的主因：它避免了把物理 LCD coverage 在 atlas 边界
再次插值。真正敏感的是 coverage 和合成：

- 1.3 gamma 对 0.5 会得到约 0.587，对 0.25 会得到约 0.344。边缘中间色
  被抬高，视觉上就是更厚、更亮；
- Subpixel shader 把每通道 coverage 归一到 max_c，再交给普通 alpha blend。
  这个补偿在目标帧缓冲已经是对应 cell 背景时，代数上可以得到每通道
  coverage；但 shader 已先把颜色转回 sRGB，普通 alpha blend 仍作用于 target
  中的颜色。对非默认背景、光标、重叠 quad 或不同透明度场景，它不能等价于
  真正的逐通道 coverage 合成器；
- 所有 glyph 类型共享普通 alpha pipeline，Subpixel 不能像 dual-source
  路径那样把 RGB coverage 作为独立 blend weight 交给 GPU。

## 与主流终端的实现差异

### WezTerm

当前源代码快照为 target/upstream/wezterm，commit 2658f62。

- Windows 的字体定位默认使用 GDI，但字体光栅器默认选择 FreeType；这两个
  层次不能混为一谈；
- config/src/font.rs:241-259 定义 FreeTypeLoadTarget。默认 Normal 的文档
  明确写的是面向标准灰度渲染；HorizontalLcd / VerticalLcd 才是 LCD target；
- config/src/font.rs:702-709 将 FontRasterizerSelection 默认设为 FreeType；
- wezterm-font/src/ftwrap.rs:74-95 把 Normal 映射到 FT_RENDER_MODE_NORMAL；
  dpi >= 100 时默认加 NO_HINTING，与当前 125% fixture 的高 DPI 方向相近；
- wezterm-font/src/rasterizer/freetype.rs:211-249 只在真正 LCD 像素模式下
  读取 RGB coverage，并将最大通道作为线性 alpha，同时做线性到 sRGB 的
  atlas 转换；
- WezTerm 还暴露 load target、render target、LCD filter、interpreter version、
  pixel geometry、weight/style 等配置，因此可调参数面明显大于 zenterm 当前
  实现。

### Alacritty

当前源代码快照为 target/upstream/alacritty，commit d692748。

- alacritty/src/renderer/text/mod.rs:142-159 按 cell 的 BOLD/ITALIC 选择四套
  独立 FontKey，并把 FontKey 放进 GlyphKey；
- alacritty/src/renderer/text/atlas.rs:95-96 使用 GL_LINEAR；其 atlas 坐标与
  rasterizer 的 bitmap 边界配套，不能直接移植到 zenterm 当前 UV/裁剪实现；
- alacritty/res/glsl3/text.f.glsl 输出主颜色和独立 alphaMask；
  gles2.rs:401-416 在支持扩展时使用 dual-source blending，不支持时退化到
  多 pass；
- Alacritty 的 crossfont 依赖 FreeType/CoreText 等平台字体后端，其
  rasterizer 细节不能作为纯 Rust swash 的逐像素目标，但 cache key 和 blend
  分层值得借鉴。

### Windows Terminal

当前源代码快照为 target/upstream/terminal，commit 9694946。

- src/renderer/atlas/AtlasEngine.cpp:872-880 先按文本片段调用 DirectWrite
  MapCharacters；1074-1138 再调用 GetGlyphs / GetGlyphPlacements；
- shader_common.hlsl:5-11 明确区分 grayscale、ClearType、builtin glyph 和
  passthrough；
- shader_ps.hlsl:45-70 对 grayscale 与 ClearType 分支分别处理；
  dwrite_helpers.hlsl:38-130 还执行增强对比度、颜色相关 gamma 修正和每通道
  alpha 修正；
- BackendD3D.cpp:150-180 的 blend state 使用 SrcBlend=ONE 与
  DestBlend=INV_SRC1_COLOR，让 shader 计算出的 weights 直接控制背景和前景
  合成，同时兼容普通 source-over 内容。

Windows Terminal 的优势来自 DirectWrite 的字体 fallback/排版和与其 rendering
params 配合的 alpha 修正。纯 Rust 实现可以复刻接口层思路和数学模型，但无法
在不调用该平台库的情况下承诺相同的 hinting、ClearType gamma 和字库内部策略。

## 根因排序

| 优先级 | 假设 | 证据 | 判断 |
| --- | --- | --- | --- |
| 1 | 默认 Subpixel 与 WezTerm 灰度路径不同 | Grayscale A/B 的 ASCII/CJK 指标重合；Subpixel ASCII mass 约 1.17 倍、fringe 非零 | 已有强证据 |
| 2 | SUBPIXEL_GAMMA=1.3 抬高边缘 coverage | 代码中仅 Subpixel 应用幂变换；幂函数对中间值单调增大 | 已确认会增粗，具体最佳值需矩阵实验 |
| 3 | 普通 alpha pipeline 承担逐通道 LCD 合成 | shader 使用 max_c 补偿后仍输出 sRGB 颜色，再走普通 alpha blend | 结构性风险，需 dual-source/离线方程 A/B 定量 |
| 4 | hinting 策略差异 | 两者在高 DPI 都趋向关闭 hinting，但 target/阈值不同 | 次要，需 None/Auto/Full 实验 |
| 5 | fallback 汉字光栅化差异 | 日志显示 Microsoft YaHei UI + Format::Alpha；CJK 行与参考近似重合 | 当前样本中不是主因 |
| 6 | bold/italic 未接入真实字体属性 | shaping.rs:159-161 只用于 run 分界；atlas_impl.rs:616-625 的 Attrs 没有 weight/style，cache key 也只有 char+size | 非当前 regular 样本主因，但属于确定的质量缺口 |

## 不改用 C 库时的优化路线

### P0：默认行为与环境匹配

建议将 Windows 默认策略调整为灰度，或提供明确的显示器安全默认值：

- RenderMode::Grayscale 作为默认，Subpixel 作为显式可选项；
- 如果保留自动模式，至少同时检查字体平滑是否启用、ClearType level、窗口
  是否透明/合成以及当前显示器是否确实有水平 RGB 子像素；当前
  SubpixelLayout::detect() 只读取 RGB/BGR orientation，禁用 ClearType 或
  OLED 时仍可能返回 RGB，见 crates/zenterm-core/src/lib.rs:370-420；
- 通过截图工具回归 ASCII、CJK、emoji 和彩色背景，确保无彩边退化。

### P1：校准 coverage 与 Subpixel 合成

把 SUBPIXEL_GAMMA 从硬编码改成可实验参数，至少跑：

    render = grayscale / subpixel
    gamma  = 1.0 / 1.1 / 1.2 / 1.3
    hint   = none / auto / full
    font   = regular / bold / italic / bold-italic
    script = ASCII / CJK fallback / emoji / mixed

选参数时同时约束：ASCII mass 不增厚、fringe 接近零、CJK 笔画不塌陷、字面
位置稳定。不能只用单个白色像素数决定质量。

### P1：改进 Subpixel 合成

有两条纯 Rust/wgpu 可行路径：

1. 保留单 pass，明确把 target 视为非 sRGB，并对 shader 的颜色与 alpha 方程
   做离线验证，避免将背景预合成和普通 alpha source-over 混用；
2. 增加单独的文字 pipeline，使用 wgpu 运行时可用的 dual-source blending
   特性；不支持时保留灰度或多 pass fallback。

第二条更接近 Alacritty/Windows Terminal 的正确模型，但需要处理 wgpu 后端
能力、不同 glyph 类型和透明背景。它适合作为后续实验，不应在没有截图 A/B
前直接替换默认路径。

### P2：补齐字体属性与缓存维度

bold/italic 应传给 cosmic-text Attrs::weight() / Attrs::style()，并至少扩展
glyph cache key 为：

    (char, font_size, resolved_face, weight, style, render_mode, hinting, subpixel_layout)

run cache 也要包含 style/weight。Alacritty 的四套 FontKey 是可核对的参考。
这会改善粗体/斜体的字重和斜率一致性，也避免字符与样式字符错误共享同一
atlas entry。

### P2：统一 DPI 重建参数

crates/zenterm-ui/src/session/reinit.rs:41-52 仍把字号/字体、hinting、render
mode 通过保守 fallback/硬编码重建。配置热更新路径使用正确配置，而 DPI 事件
路径可能回到 18.0、monospace、Auto、Subpixel。这不会解释当前固定截图，但
会造成跨显示器移动后字体突然变化，应该将实际 FontConfig 贯穿到 DPI reinit。

## 最终判断

在不改用 FreeType、DirectWrite 等 C/系统字体库的前提下，项目有实质的优化
空间，且第一步不需要重写字体引擎：

- 先把默认模式从无条件 Subpixel 调整为灰度/环境感知；
- 将 1.3 gamma 改成可验证、可配置的策略并用截图矩阵选值；
- 再评估 dual-source 或多 pass 的 wgpu 文字 pipeline；
- 最后修复 weight/style/cache/DPI 维度。

这些改变可以直接改善当前观察到的边缘厚、彩边、锐度不稳定。剩余差异主要
来自不同光栅器的 hinting、字形轮廓和字体 fallback 选择；纯 Rust 路径可以达到
主流终端级别的观感和稳定性，但不应把逐像素复制 WezTerm 的 FreeType/平台
字体后端作为可验证承诺。
