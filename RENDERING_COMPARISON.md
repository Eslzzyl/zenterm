# Alacritty vs Zenterm 渲染管线对比

本文档详细对比两个终端模拟器在 GPU 文字渲染管线上的架构差异，
帮助理解 zenterm 为何需要 CPU 端 glyph 裁剪，以及这些设计选择的 trade-off。

## 目录

1. [整体架构](#1-整体架构)
2. [渲染流程：双 Pass vs 单 Pass](#2-渲染流程双-pass-vs-单-pass)
3. [核心差异：混合策略](#3-核心差异混合策略)
4. [字形溢出与裁剪](#4-字形溢出与裁剪)
5. [Cell 度量计算](#5-cell-度量计算)
6. [Block Elements 内置渲染](#6-block-elements-内置渲染)
7. [光标渲染](#7-光标渲染)
8. [字形缓存与图集](#8-字形缓存与图集)
9. [总结与 Trade-off](#9-总结与-trade-off)

---

## 1. 整体架构

| | Alacritty | Zenterm |
|--|-----------|---------|
| GPU API | OpenGL / OpenGL ES | wgpu (Metal/Vulkan/DX12/GLES) |
| 光栅化库 | crossfont (FreeType/CoreText) | cosmic-text + swash + 内置软件光栅器 |
| 渲染模型 | 两遍 Instanced Rendering | 单遍 Instanced Rendering |
| 混合方式 | **双源混合**（Dual-Source Blending） | 标准 Alpha 混合 |
| 裁剪 | **无 CPU 端裁剪** | **有 CPU 端裁剪** |

---

## 2. 渲染流程：双 Pass vs 单 Pass

### Alacritty：两遍渲染

Alacritty 将每个 cell 的渲染拆分为两个独立的 GPU pass：

```
Pass 0（背景）：所有非空 cell 的背景 quad
  ┌──────────────────────────────┐
  │  blend = SrcAlpha / 1-SrcAlpha │
  │  shader 输出 bg_color          │
  │  bg_alpha == 0 → discard       │
  └──────────────────────────────┘

Pass 1（文字）：所有非空 cell 的 glyph quad
  ┌──────────────────────────────┐
  │  blend = Src1Color / 1-Src1Color │  ← 双源混合
  │  shader 输出 fg_color              │
  │  ALPHA_MASK = coverage             │
  └──────────────────────────────┘
```

参考代码：`alacritty/src/renderer/text/glsl3.rs:241-257`

关键点：
- 两遍使用**同一组 vertex/instance 数据**，只是 shader uniform 和 blend state 不同
- 背景 quad 精确等于 cell 尺寸（或 2× cell 用于宽字符）
- 默认背景 cell 的 `bg_alpha = 0`，fragment shader 直接 `discard`

### Zenterm：单遍交错渲染

Zenterm 在一个 pass 中同时渲染背景和 glyph，通过 instance 的 `flags` 字段区分：

```
单次 Draw Call：
  ┌─ SOLID instance ──→ 背景/光标/选区 quad（无纹理采样）
  ├─ MASK instance ───→ glyph quad（grayscale mask → mix(bg, fg, α)）
  ├─ SUBPIXEL instance → glyph quad（LCD subpixel → per-channel blend）
  └─ COLOR instance ──→ glyph quad（emoji → RGBA 直接输出）
```

参考代码：`crates/zenterm-render/src/lib.rs` fragment shader

所有 instance 打包在同一个 vertex buffer 中，一次 `draw_indexed` 调用完成。

---

## 3. 核心差异：混合策略

这是两个终端最根本的架构差异，直接影响了裁剪需求。

### Alacritty：双源混合（Dual-Source Blending）

Alacritty 的 fragment shader 有两个输出：

```glsl
// Pass 1 (glyph) fragment shader
layout(location = 0, index = 0) out vec4 color;      // 主输出
layout(location = 0, index = 1) out vec4 alphaMask;   // 副输出（coverage）

void main() {
    vec3 textColor = texture(mask, TexCoords).rgb;
    ALPHA_MASK = vec4(textColor, textColor.r);  // coverage → 副输出
    FRAG_COLOR = vec4(fg.rgb, 1.0);             // 纯前景色 → 主输出
}
```

GPU 混合方程：

```
result.rgb = src.rgb × src1.rgb + dst.rgb × (1 - src1.rgb)
           = fg × coverage + framebuffer × (1 - coverage)
```

**关键效果**：当 `coverage = 0`（透明像素）时：

```
result = fg × 0 + framebuffer × 1 = framebuffer  （保持原样）
```

透明像素**不修改帧缓冲**。glyph quad 溢出到相邻 cell 时，溢出区域的透明像素不会画任何东西。

参考代码：`alacritty/res/glsl3/text.f.glsl`、`alacritty/res/glsl3/text.v.glsl`

### Zenterm：标准 Alpha 混合

Zenterm 的 fragment shader 只有一个输出，`bg_color` 被烘焙进结果：

```wgsl
// MASK glyph fragment shader
let alpha = texel.r;
return vec4<f32>(
    bg_r + (fg_r - bg_r) * alpha,    // mix(bg, fg, α)
    bg_g + (fg_g - bg_g) * alpha,
    bg_b + (fg_b - bg_b) * alpha,
    1.0,
);
```

GPU 混合方程（预乘 Alpha）：

```
result = src × 1 + dst × (1 - src_alpha)
       = mix(bg, fg, α) × 1 + framebuffer × 0
       = mix(bg, fg, α)
```

**关键效果**：当 `coverage = 0`（透明像素）时：

```
result = mix(bg, fg, 0) = bg_color  （输出背景色！）
```

透明像素**输出 bg_color**。glyph quad 溢出到相邻 cell 时，溢出区域会画上 `bg_color`，覆盖相邻 cell 的内容。

参考代码：`crates/zenterm-render/src/lib.rs:493-501`

---

## 4. 字形溢出与裁剪

### 为什么字形会溢出 cell

swash 光栅化 glyph 时，位图的包围盒可能超出 cell 边界：

1. **Bézier 控制点溢出**：矢量轮廓的控制点可以超出实际曲线，导致包围盒偏大
2. **像素对齐的 floor/ceil**：包围盒对齐到像素网格时，上下各可能多出 1px
3. **OS/2 度量与实际轮廓不一致**：字体的 ascent/descent 是排版建议值，个别字符的轮廓可以超出

### Alacritty：不需要裁剪

因为双源混合，溢出区域的透明像素不修改帧缓冲，溢出不可见。

### Zenterm：需要裁剪

因为标准 alpha 混合，溢出区域的透明像素输出 `bg_color`，溢出可见。
所以在 CPU 端构建 instance 数据时，将 glyph quad 裁剪到 cell 边界内，同时调整 UV 坐标。

参考代码：`crates/zenterm-ui/src/session.rs:860-886`
参考文档：`GLYPH_CLIP.md`

### 裁剪的影响范围

| glyph 类型 | 溢出行为 | 裁剪效果 |
|-----------|---------|---------|
| 普通文字（默认背景） | bg_color = 终端背景色，溢出与背景同色 → 不可见 | 裁剪是安全网 |
| 普通文字（非默认背景） | bg_color ≠ 背景色，溢出可见 | 裁剪消除伪影 |
| Block elements (░▒▓█) | bearing_y 修复前：溢出整个 descent 区域 | 修复后裁剪是 no-op |
| Box drawing (─│┌┐) | 通常无溢出 | 裁剪是 no-op |
| 光标（Block cursor） | bg_color = 光标色 ≠ 背景色 | 裁剪消除颜色渗漏 |

---

## 5. Cell 度量计算

| | Alacritty | Zenterm |
|--|-----------|---------|
| cell_width | `(average_advance + offset_x).floor()` | `ensure_glyph('W').advance.ceil()` |
| cell_height | `(metrics.line_height + offset_y).floor()` | `(cell_ascent + cell_descent).ceil()` |
| line_height 来源 | crossfont `Metrics.line_height`（含 line_gap） | cosmic-text `max_ascent + max_descent`（不含 line_gap） |
| baseline | 由 `cell_height - descent` 隐式确定 | `cell_ascent`（cosmic-text `max_ascent`） |

Alacritty 的 `metrics.line_height` 包含字体的 `line_gap`（行间距），
而 zenterm 的 `cell_ascent + cell_descent` 不包含。实际差异通常只有 1-2px，
但在某些字体上可能导致 zenterm 的 cell 稍微紧凑。

参考代码：
- Alacritty: `alacritty/src/display/mod.rs:1608-1615`
- Zenterm: `crates/zenterm-glyph/src/lib.rs` `cell_size()` 方法

---

## 6. Block Elements 内置渲染

两个终端都对 Unicode Block Elements（U+2580–U+259F）和 Box Drawing（U+2500–U+257F）
提供了内置的软件光栅化，绕过系统字体。

### Alacritty

- 文件：`alacritty/src/renderer/text/builtin_font.rs`
- 通过 `builtin_box_drawing()` 函数生成像素 buffer
- 结果缓存在 glyph cache 中，和字体 glyph 一样处理
- bearing_y 来自 `crossfont::Metrics`，与普通字符一致

### Zenterm

- 文件：`crates/zenterm-glyph/src/builtin.rs`
- `BuiltinParams` 包含 `cell_width`、`cell_height`、`cell_ascent`
- 每个 glyph 生成 `cell_width × cell_height` 像素的纯色矩形
- `bearing_y = cell_ascent`（修复后），确保 glyph 覆盖整个 cell

### 历史 Bug

Zenterm 曾将 `bearing_y` 设为 `cell_height`（整个 cell 高度），
导致 glyph 定位在 baseline 之上 `cell_height` 像素处，完全不覆盖 baseline 以下区域。
CPU 裁剪切掉上方溢出后，每个 cell 底部的 `cell_descent` 像素露白，
产生行间可见间隙。修复：将 `bearing_y` 改为 `cell_ascent`。

参考提交：`8a0b470`

---

## 7. 光标渲染

### Alacritty

| 光标形状 | 实现方式 |
|---------|---------|
| Block | 修改 cell 的 `fg`/`bg`/`bg_alpha=1.0`，走正常的两遍文字渲染 |
| Beam | 一个 `RenderRect`（竖线），标准 alpha 混合 |
| Underline | 一个 `RenderRect`（横线），标准 alpha 混合 |
| HollowBlock | 四个 `RenderRect`（边框线），标准 alpha 混合 |

Block cursor 的巧妙之处：不画额外的 quad，而是让 cell 本身"变成"光标颜色，
然后正常渲染文字。双源混合确保光标的 bg 不会渗到相邻 cell。

参考代码：`alacritty/src/display/content.rs:166-177`

### Zenterm

| 光标形状 | 实现方式 |
|---------|---------|
| Block | SOLID quad（cell 大小）+ glyph quad（反色） |
| Beam | SOLID quad（1px 宽） |
| Underline | SOLID quad（cell 底部横线） |
| HollowBlock | 四条 SOLID quad（边框线） |

Block cursor 的 glyph quad 使用 `bg_color = cell.fg`（光标色），
如果 glyph 溢出，光标色会渗到相邻 cell。CPU 裁剪防止了这个问题。

参考代码：`crates/zenterm-ui/src/session.rs:899-936`

---

## 8. 字形缓存与图集

| | Alacritty | Zenterm |
|--|-----------|---------|
| 图集格式 | 1024×1024 RGBA，多张 atlas | Power-of-2 RGBA，单张 atlas（自动扩容到 4096） |
| 上传方式 | `glTexSubImage2D`（增量上传） | `queue.write_texture`（整张上传） |
| 缓存键 | `(font_key, font_size, char)` | `(char, font_size.to_bits())` |
| 光栅化 | crossfont（FreeType/CoreText） | swash（Subpixel format） + builtin 软件光栅 |
| 子像素渲染 | LCD subpixel（RGB 三通道独立 coverage） | LCD subpixel（同左） |
| 纹理过滤 | Linear（文字）/ Nearest（取决于后端） | **Nearest**（全部） |

---

## 9. 总结与 Trade-off

### Zenterm 选择当前方案的原因

1. **wgpu 抽象层**：双源混合虽然在所有 wgpu 后端都有支持，
   但 wgpu 的 `DUAL_SOURCE_BLENDING` 特性需要运行时检测，
   不如标准 alpha 混合来得直接

2. **单 Pass 渲染**：将 SOLID/MASK/SUBPIXEL/COLOR 合并到一次 draw call，
   减少 GPU 状态切换。代价是所有 instance 共享同一个 blend state

3. **CPU 裁剪**：几行浮点运算，对每 个 glyph 执行，开销可忽略。
   换来了对混合策略的简化

### 如果迁移到双源混合

| 方面 | 工作量 | 说明 |
|------|--------|------|
| Fragment shader | 中等 | MASK/SUBPIXEL 路径改为双输出 |
| Pipeline blend state | 中等 | 可能需要多个 pipeline（SUBPIXEL 与 MASK 混合方式不同） |
| Instance 数据 | 轻微 | bg_color 可从 glyph instance 中移除 |
| 裁剪代码 | 可删除 | 透明像素不修改帧缓冲，溢出不可见 |
| 测试 | 大量 | 所有 glyph 类型 + 光标 + 选区 + 透明窗口 |
| **总计** | **约 2-3 天** | |

### 当前状态

修复 `bearing_y` 后，管线功能正确：
- Block elements 行间无间隙
- 裁剪对 builtin glyph 是 no-op
- 对普通 glyph 只裁掉 1-2px 不可见溢出
- 光标颜色不渗漏
