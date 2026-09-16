# Font render check

这是一个 Windows 本机截图回归工具，用来比较 zenterm 与正在运行的
WezTerm。默认通过短暂置顶窗口并复制屏幕像素捕获实际显示内容，再用像素差
自动找出文字行，生成 Nearest-neighbor 放大图和 JSON 指标；不依赖 Python、
C 字体库或 OCR。

## 一次性对照

先让两个终端显示相同的固定文本。建议至少包含：

```text
ASCII: AaHhNn0Qg @#$% && ->
CJK: 汉字 渲染 边缘 锐利
Mixed: Hn0Qg e n M W 你好世界>
```

固定条件应保持一致：字体、物理字号、前景/背景色、DPI、窗口缩放和
字体粗细。截图中可额外存在 shell banner；工具会默认取最后三条文字行，
所以可以直接处理当前已有的 fixture 截图。

对已有截图运行：

```powershell
pwsh -File .\tools\font-render-check\check.ps1 `
  -ZentermImage .\target\zenterm-fixture.png `
  -ZentermGrayscaleImage .\target\zenterm-grayscale.png `
  -ReferenceImage .\target\wezterm-controlled.png
```

输出在 `target\font-render-check\`：

- `report.json`：每个文字行的 ink 像素数、近似 coverage mass、RGB fringe
  和与 WezTerm 的比值；
- `*-ascii-zoom.png`、`*-cjk-zoom.png`、`*-mixed-zoom.png`：自动裁剪并以
  8 倍 Nearest 放大的行图；
- `capture.json`：使用 `-Capture` 时记录窗口标题、PID 和截图路径。

`-Capture` 默认使用 `-CaptureMethod Screen`。它会恢复最小化的 fixture，必要
时将完全位于所有显示器之外的窗口移到当前虚拟屏幕，并在复制期间短暂置顶；
这对 eframe/wgpu 窗口可靠，代价是会切换前台。屏幕或窗口尺寸变化时，工具重新
读取每个窗口的实际物理像素矩形，不使用固定屏幕尺寸。

需要捕获可被 DWM 离屏绘制的普通窗口时，可显式使用：

```powershell
pwsh -File .\tools\font-render-check\check.ps1 `
  -Capture -CaptureMethod PrintWindow `
  -ZentermProcessId <zenterm-pid> -ReferenceProcessId <wezterm-gui-pid>
```

`PrintWindow` 不适合 zenterm 当前的 wgpu 内容；若输出只有背景或出现整块
无效区域，应改用默认的 `Screen` 模式。

## 捕获正在运行的窗口

窗口标题可能重复时必须指定 PID，避免抓错窗口。先查看 PID：

```powershell
Get-Process zenterm,wezterm-gui |
  Select-Object Id,ProcessName,MainWindowTitle,StartTime
```

然后运行：

```powershell
pwsh -File .\tools\font-render-check\check.ps1 `
  -Capture `
  -ZentermProcessId <zenterm-pid> `
  -ReferenceProcessId <wezterm-gui-pid> `
  -OutputDir .\target\font-render-check\run-01
```

`-CaptureMethod Screen` 会读取实际屏幕像素；为处理最小化或离屏的 fixture，
会恢复窗口、必要时移动到当前虚拟屏幕并短暂置顶，然后恢复非置顶状态。因此
它可能切换前台，但不发送按键或输入。若 WezTerm 窗口标题为空或不稳定，可
直接传 `-ReferenceProcessId`。`-CaptureMethod PrintWindow` 不改变前台，然而
对 zenterm 的 wgpu 内容可能只得到黑块或背景。

## 如何解释指标

`RgbFringe` 是文字行内各像素 `max(R,G,B)-min(R,G,B)` 的累计值；灰度边缘
应接近零，LCD Subpixel 的彩边会明显增大。`CoverageMass` 是按背景亮度归一
化的近似覆盖量，用于识别字重/边缘变厚，同时支持深色和浅色主题；它不是
字体引擎的内部 coverage，不能单独作为视觉质量分数。

判读顺序是：先看放大图，再看 RGB fringe，最后用 coverage/ink 比值定位
“变厚”还是“位置/字体度量不同”。如果 zenterm 的 Grayscale 与 WezTerm
接近，而 Subpixel 的 fringe 和 mass 明显偏高，优先检查渲染模式、gamma
和合成方程；如果只有 CJK 偏差，才优先检查 fallback 字体、fallback 的
hinting 与 cell 约束。
