[CmdletBinding()]
param(
    [string]$ZentermImage,
    [string]$ZentermGrayscaleImage,
    [string]$ReferenceImage,
    [switch]$Capture,
    [ValidateSet("Screen", "PrintWindow")]
    [string]$CaptureMethod = "Screen",
    [int]$ZentermProcessId = 0,
    [int]$ReferenceProcessId = 0,
    [string]$ZentermTitle = "Zenterm Font Fixture",
    [string]$ReferenceTitle,
    [string]$OutputDir = (Join-Path (Resolve-Path (Join-Path $PSScriptRoot "..\..")) "target\font-render-check"),
    [int]$Zoom = 8,
    [int]$Threshold = 12,
    [int]$MergeGap = 2,
    [int]$CompareLast = 3,
    [string[]]$Labels = @("ASCII", "CJK", "Mixed")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Add-CaptureType {
    if ("FontRenderCapture" -as [type]) {
        return
    }

    Add-Type -AssemblyName System.Drawing.Common
    Add-Type -TypeDefinition @"
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;

public static class FontRenderCapture
{
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int count);

    [DllImport("user32.dll")]
    private static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);

    public static bool GetRect(IntPtr hWnd, out RECT rect)
    {
        return GetWindowRect(hWnd, out rect);
    }

    [DllImport("user32.dll")]
    private static extern bool IsWindowVisible(IntPtr hWnd);

    [DllImport("user32.dll")]
    private static extern bool SetProcessDPIAware();

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool SetProcessDpiAwarenessContext(IntPtr value);

    public static void InitializeDpi()
    {
        // -4 = DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2. Fall back to
        // system awareness on older Windows versions.
        if (!SetProcessDpiAwarenessContext(new IntPtr(-4)))
            SetProcessDPIAware();
    }

    [DllImport("user32.dll")]
    private static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

    [DllImport("user32.dll")]
    private static extern bool SetForegroundWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    private static extern bool BringWindowToTop(IntPtr hWnd);

    [DllImport("user32.dll")]
    private static extern bool SetWindowPos(
        IntPtr hWnd,
        IntPtr hWndInsertAfter,
        int x,
        int y,
        int cx,
        int cy,
        uint flags);

    [DllImport("user32.dll")]
    private static extern int GetSystemMetrics(int index);

    [DllImport("kernel32.dll")]
    private static extern void Sleep(uint milliseconds);

    [DllImport("user32.dll")]
    private static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);

    public static string GetTitle(IntPtr hWnd)
    {
        var text = new StringBuilder(1024);
        GetWindowText(hWnd, text, text.Capacity);
        return text.ToString();
    }

    public static bool Visible(IntPtr hWnd)
    {
        return hWnd != IntPtr.Zero && IsWindowVisible(hWnd);
    }

    private static bool IsOffVirtualScreen(RECT rect)
    {
        var x = GetSystemMetrics(76); // SM_XVIRTUALSCREEN
        var y = GetSystemMetrics(77); // SM_YVIRTUALSCREEN
        var width = GetSystemMetrics(78); // SM_CXVIRTUALSCREEN
        var height = GetSystemMetrics(79); // SM_CYVIRTUALSCREEN
        return rect.Right <= x || rect.Bottom <= y ||
               rect.Left >= x + width || rect.Top >= y + height;
    }

    public static Bitmap Capture(IntPtr hWnd)
    {
        if (!GetWindowRect(hWnd, out var rect))
            throw new InvalidOperationException("GetWindowRect failed");

        var width = rect.Right - rect.Left;
        var height = rect.Bottom - rect.Top;
        if (width <= 0 || height <= 0)
            throw new InvalidOperationException("Window has an invalid size");

        var bitmap = new Bitmap(width, height, PixelFormat.Format32bppArgb);
        using (var graphics = Graphics.FromImage(bitmap))
        {
            var hdc = graphics.GetHdc();
            try
            {
                // PW_RENDERFULLCONTENT also captures occluded DWM-backed windows.
                if (!PrintWindow(hWnd, hdc, 2))
                    throw new InvalidOperationException("PrintWindow failed");
            }
            finally
            {
                graphics.ReleaseHdc(hdc);
            }
        }
        return bitmap;
    }

    public static Bitmap CaptureScreen(IntPtr hWnd)
    {
        if (!Visible(hWnd))
            throw new InvalidOperationException("Window is not visible");

        // A minimized fixture may retain a tiny (-25600,-25600) rectangle.
        // Restore it before measuring the pixels to be copied.
        ShowWindow(hWnd, 9); // SW_RESTORE
        Sleep(120);

        if (!GetWindowRect(hWnd, out var rect))
            throw new InvalidOperationException("GetWindowRect failed after restore");

        var width = rect.Right - rect.Left;
        var height = rect.Bottom - rect.Top;
        if (width < 200 || height < 100)
            throw new InvalidOperationException($"Window is too small after restore: {width}x{height}");

        // Move only a fixture that is wholly outside all monitors. This is
        // needed for repeatable captures of windows parked at -25600.
        if (IsOffVirtualScreen(rect))
        {
            var screenX = GetSystemMetrics(76);
            var screenY = GetSystemMetrics(77);
            SetWindowPos(hWnd, IntPtr.Zero, screenX + 40, screenY + 40,
                width, height, 0x0040); // SWP_SHOWWINDOW
            Sleep(80);
            if (!GetWindowRect(hWnd, out rect))
                throw new InvalidOperationException("GetWindowRect failed after reposition");
        }

        // Set the target topmost for the short copy interval. Merely calling
        // SetForegroundWindow can be ignored by Windows' foreground-lock
        // policy, which would otherwise copy an occluding Codex/Explorer
        // window. The original z-order is restored in finally.
        var hwndTopmost = new IntPtr(-1); // HWND_TOPMOST
        var hwndNotTopmost = new IntPtr(-2); // HWND_NOTOPMOST
        SetWindowPos(hWnd, hwndTopmost, rect.Left, rect.Top, width, height, 0x0040); // SWP_SHOWWINDOW
        try
        {
            BringWindowToTop(hWnd);
            SetForegroundWindow(hWnd);
            Sleep(180);

            if (!GetWindowRect(hWnd, out rect))
                throw new InvalidOperationException("GetWindowRect failed after activation");
            width = rect.Right - rect.Left;
            height = rect.Bottom - rect.Top;
            if (width <= 0 || height <= 0)
                throw new InvalidOperationException("Window has an invalid size after activation");

            var bitmap = new Bitmap(width, height, PixelFormat.Format32bppArgb);
            using (var graphics = Graphics.FromImage(bitmap))
            {
                graphics.CopyFromScreen(rect.Left, rect.Top, 0, 0,
                    new Size(width, height), CopyPixelOperation.SourceCopy);
            }
            return bitmap;
        }
        finally
        {
            SetWindowPos(hWnd, hwndNotTopmost, 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0010); // SWP_NOSIZE|SWP_NOMOVE|SWP_NOACTIVATE
        }
    }
}
"@ -ReferencedAssemblies @(
        [System.Drawing.Bitmap].Assembly.Location,
        ([System.Reflection.Assembly]::Load("System.Drawing.Primitives")).Location,
        ([System.Reflection.Assembly]::Load("System.Private.Windows.GdiPlus")).Location,
        ([System.Reflection.Assembly]::Load("System.Private.Windows.Core")).Location
    )
}

function Get-WindowCandidate {
    param(
        [string]$ProcessName,
        [int]$ProcessId,
        [string]$Title,
        [switch]$AllowScreenRecovery
    )

    $processes = if ($ProcessId -gt 0) {
        @(Get-Process -Id $ProcessId -ErrorAction Stop)
    } else {
        @(Get-Process -Name $ProcessName -ErrorAction SilentlyContinue)
    }

    $candidates = @(
        foreach ($process in $processes) {
            $handle = [IntPtr]$process.MainWindowHandle
            if (-not [FontRenderCapture]::Visible($handle)) {
                continue
            }
            $rect = [FontRenderCapture+RECT]::new()
            if (-not [FontRenderCapture]::GetRect($handle, [ref]$rect)) {
                continue
            }
            $windowTitle = [FontRenderCapture]::GetTitle($handle)
            if ($Title -and $windowTitle -ne $Title) {
                continue
            }
            [pscustomobject]@{
                ProcessId = $process.Id
                ProcessName = $process.ProcessName
                Handle = $handle
                Title = $windowTitle
                StartTime = $process.StartTime
                Left = $rect.Left
                Top = $rect.Top
                Width = $rect.Right - $rect.Left
                Height = $rect.Bottom - $rect.Top
            }
        }
    )

    if ($candidates.Count -ne 1) {
        $details = ($candidates | ForEach-Object {
                "pid=$($_.ProcessId) title=$($_.Title) started=$($_.StartTime.ToString('s'))"
            }) -join "; "
        throw "Expected exactly one $ProcessName window. Found $($candidates.Count): $details. Supply an explicit process id."
    }
    if (-not $AllowScreenRecovery -and ($candidates[0].Width -lt 200 -or $candidates[0].Height -lt 100 -or
        $candidates[0].Left -lt -1000 -or $candidates[0].Top -lt -1000)) {
        throw "Selected $ProcessName window is minimized/off-screen or too small: pid=$($candidates[0].ProcessId) rect=($($candidates[0].Left),$($candidates[0].Top),$($candidates[0].Width)x$($candidates[0].Height)). Restore it before capture."
    }
    return $candidates[0]
}

function Save-WindowCapture {
    param(
        [Parameter(Mandatory)]$Window,
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][ValidateSet("Screen", "PrintWindow")][string]$Method
    )

    $bitmap = if ($Method -eq "Screen") {
        [FontRenderCapture]::CaptureScreen($Window.Handle)
    } else {
        [FontRenderCapture]::Capture($Window.Handle)
    }
    try {
        $bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
        [pscustomobject]@{
            Path = $Path
            Width = $bitmap.Width
            Height = $bitmap.Height
        }
    } finally {
        $bitmap.Dispose()
    }
}

function Get-PixelDistance {
    param(
        [Parameter(Mandatory)][System.Drawing.Color]$Pixel,
        [Parameter(Mandatory)][int[]]$Background
    )

    $dr = [Math]::Abs(([int]$Pixel.R) - $Background[0])
    $dg = [Math]::Abs(([int]$Pixel.G) - $Background[1])
    $db = [Math]::Abs(([int]$Pixel.B) - $Background[2])
    return [Math]::Max($dr, [Math]::Max($dg, $db))
}

function Get-BackgroundColor {
    param(
        [Parameter(Mandatory)][System.Drawing.Bitmap]$Bitmap
    )

    # A window border or a shadow can occupy pixel (0,0).  Use a coarse
    # histogram instead, so the dominant terminal fill wins over chrome.
    $histogram = @{}
    $stepX = [Math]::Max(1, [int]($Bitmap.Width / 64))
    $stepY = [Math]::Max(1, [int]($Bitmap.Height / 64))
    for ($y = 0; $y -lt $Bitmap.Height; $y += $stepY) {
        for ($x = 0; $x -lt $Bitmap.Width; $x += $stepX) {
            $pixel = $Bitmap.GetPixel($x, $y)
            $key = "{0},{1},{2}" -f $pixel.R, $pixel.G, $pixel.B
            if ($histogram.ContainsKey($key)) {
                $histogram[$key]++
            } else {
                $histogram[$key] = 1
            }
        }
    }
    $dominant = $histogram.GetEnumerator() |
        Sort-Object Value -Descending |
        Select-Object -First 1
    return @($dominant.Key -split ',' | ForEach-Object { [int]$_ })
}

function Get-ContentRange {
    param(
        [Parameter(Mandatory)][System.Drawing.Bitmap]$Bitmap,
        [Parameter(Mandatory)][int[]]$Background,
        [Parameter(Mandatory)][int]$Threshold
    )

    # Exclude a persistent window border/shadow from the row statistics.  A
    # WezTerm PrintWindow capture commonly has a 9 px black strip at the left;
    # treating it as ink would merge every row into one giant band.
    $columnInk = New-Object int[] $Bitmap.Width
    for ($y = 0; $y -lt $Bitmap.Height; $y++) {
        for ($x = 0; $x -lt $Bitmap.Width; $x++) {
            if ((Get-PixelDistance ( $Bitmap.GetPixel($x, $y) ) $Background) -gt $Threshold) {
                $columnInk[$x]++
            }
        }
    }
    $persistent = [Math]::Max(1, [int]($Bitmap.Height * 0.8))
    $left = 0
    while ($left -lt $Bitmap.Width -and $columnInk[$left] -ge $persistent) {
        $left++
    }
    $right = $Bitmap.Width - 1
    while ($right -gt $left -and $columnInk[$right] -ge $persistent) {
        $right--
    }
    [pscustomobject]@{ X0 = $left; X1 = $right }
}

function Get-RowBands {
    param(
        [Parameter(Mandatory)][System.Drawing.Bitmap]$Bitmap,
        [Parameter(Mandatory)][int[]]$Background,
        [Parameter(Mandatory)][int]$Threshold,
        [Parameter(Mandatory)][int]$MergeGap,
        [Parameter(Mandatory)]$ContentRange
    )

    $active = New-Object bool[] $Bitmap.Height
    for ($y = 0; $y -lt $Bitmap.Height; $y++) {
        $ink = 0
        for ($x = $ContentRange.X0; $x -le $ContentRange.X1; $x++) {
            if ((Get-PixelDistance ( $Bitmap.GetPixel($x, $y) ) $Background) -gt $Threshold) {
                $ink++
            }
        }
        # Ignore isolated border pixels and antialiased noise.
        $active[$y] = $ink -ge [Math]::Max(3, [int]($Bitmap.Width * 0.001))
    }

    $bands = [System.Collections.Generic.List[object]]::new()
    $start = -1
    for ($y = 0; $y -lt $active.Length; $y++) {
        if ($active[$y] -and $start -lt 0) {
            $start = $y
        }
        $lastRow = $y -eq $active.Length - 1
        if ($start -ge 0 -and ((!$active[$y]) -or $lastRow)) {
            $end = if ($active[$y]) { $y } else { $y - 1 }
            if ($bands.Count -gt 0 -and ($start - $bands[$bands.Count - 1].Y1 - 1) -le $MergeGap) {
                $bands[$bands.Count - 1].Y1 = $end
            } else {
                [void]$bands.Add([pscustomobject]@{ Y0 = $start; Y1 = $end })
            }
            $start = -1
        }
    }
    return @($bands)
}

function Get-BandMetrics {
    param(
        [Parameter(Mandatory)][System.Drawing.Bitmap]$Bitmap,
        [Parameter(Mandatory)]$Band,
        [Parameter(Mandatory)][int[]]$Background,
        [Parameter(Mandatory)][int]$Threshold,
        [Parameter(Mandatory)]$ContentRange
    )

    $bgLum = 0.2126 * $Background[0] + 0.7152 * $Background[1] + 0.0722 * $Background[2]
    $x0 = $ContentRange.X1 + 1
    $x1 = -1
    $inkPixels = 0
    [double]$coverageMass = 0.0
    [double]$rgbFringe = 0.0
    $fringePixels = 0

    for ($y = $Band.Y0; $y -le $Band.Y1; $y++) {
        for ($x = $ContentRange.X0; $x -le $ContentRange.X1; $x++) {
            $pixel = $Bitmap.GetPixel($x, $y)
            $distance = Get-PixelDistance $pixel $Background
            if ($distance -le $Threshold) {
                continue
            }

            $inkPixels++
            $x0 = [Math]::Min($x0, $x)
            $x1 = [Math]::Max($x1, $x)
            $lum = 0.2126 * $pixel.R + 0.7152 * $pixel.G + 0.0722 * $pixel.B
            $coverage = if ($bgLum -lt 128.0) {
                ($lum - $bgLum) / [Math]::Max(1.0, 255.0 - $bgLum)
            } else {
                ($bgLum - $lum) / [Math]::Max(1.0, $bgLum)
            }
            $coverageMass += [Math]::Max(0.0, [Math]::Min(1.0, $coverage))
            $spread = ([Math]::Max($pixel.R, [Math]::Max($pixel.G, $pixel.B)) - [Math]::Min($pixel.R, [Math]::Min($pixel.G, $pixel.B))) / 255.0
            $rgbFringe += $spread
            if ($spread -gt 0.02) {
                $fringePixels++
            }
        }
    }

    [pscustomobject]@{
        Y0 = $Band.Y0
        Y1 = $Band.Y1
        Height = $Band.Y1 - $Band.Y0 + 1
        BackgroundLuma = [Math]::Round($bgLum, 2)
        X0 = if ($x1 -ge 0) { $x0 } else { 0 }
        X1 = if ($x1 -ge 0) { $x1 } else { 0 }
        InkPixels = $inkPixels
        CoverageMass = [Math]::Round($coverageMass, 2)
        RgbFringe = [Math]::Round($rgbFringe, 2)
        FringePixels = $fringePixels
    }
}

function Save-BandZoom {
    param(
        [Parameter(Mandatory)][System.Drawing.Bitmap]$Bitmap,
        [Parameter(Mandatory)]$Metrics,
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][int]$Zoom
    )

    $pad = 2
    $srcX = [Math]::Max(0, $Metrics.X0 - $pad)
    $srcY = [Math]::Max(0, $Metrics.Y0 - $pad)
    $srcRight = [Math]::Min($Bitmap.Width - 1, $Metrics.X1 + $pad)
    $srcBottom = [Math]::Min($Bitmap.Height - 1, $Metrics.Y1 + $pad)
    $srcWidth = [Math]::Max(1, $srcRight - $srcX + 1)
    $srcHeight = [Math]::Max(1, $srcBottom - $srcY + 1)
    $zoomed = [System.Drawing.Bitmap]::new($srcWidth * $Zoom, $srcHeight * $Zoom)
    try {
        $graphics = [System.Drawing.Graphics]::FromImage($zoomed)
        try {
            $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
            $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::Half
            $graphics.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
            $destination = [System.Drawing.Rectangle]::new(0, 0, $zoomed.Width, $zoomed.Height)
            $source = [System.Drawing.Rectangle]::new($srcX, $srcY, $srcWidth, $srcHeight)
            $graphics.DrawImage($Bitmap, $destination, $source, [System.Drawing.GraphicsUnit]::Pixel)
        } finally {
            $graphics.Dispose()
        }
        $zoomed.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    } finally {
        $zoomed.Dispose()
    }
}

function Get-ImageAnalysis {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Name
    )

    $resolved = (Resolve-Path $Path).Path
    $bitmap = [System.Drawing.Bitmap]::new($resolved)
    try {
        $background = Get-BackgroundColor $bitmap
        $contentRange = Get-ContentRange $bitmap $background $Threshold
        $bands = Get-RowBands $bitmap $background $Threshold $MergeGap $contentRange
        $metrics = @($bands | ForEach-Object { Get-BandMetrics $bitmap $_ $background $Threshold $contentRange })
        $usable = @($metrics | Where-Object { $_.InkPixels -gt 0 -and $_.Height -ge 3 })
        if ($usable.Count -gt $CompareLast) {
            $selected = @($usable | Select-Object -Last $CompareLast)
        } else {
            $selected = $usable
        }

        $zoomPaths = @()
        for ($i = 0; $i -lt $selected.Count; $i++) {
            $label = if ($i -lt $Labels.Count) { $Labels[$i] } else { "Band$i" }
            $zoomPath = Join-Path $OutputDir ("{0}-{1}-zoom.png" -f $Name, $label.ToLowerInvariant())
            Save-BandZoom $bitmap $selected[$i] $zoomPath $Zoom
            $zoomPaths += $zoomPath
        }

        [pscustomobject]@{
            Name = $Name
            Path = $resolved
            Width = $bitmap.Width
            Height = $bitmap.Height
            Background = $background
            ContentRange = $contentRange
            Bands = $metrics
            Selected = $selected
            Zooms = $zoomPaths
        }
    } finally {
        $bitmap.Dispose()
    }
}

function Compare-SelectedBands {
    param(
        [Parameter(Mandatory)]$Left,
        [Parameter(Mandatory)]$Right,
        [Parameter(Mandatory)][string]$LeftName,
        [Parameter(Mandatory)][string]$RightName
    )

    $count = [Math]::Min($Left.Selected.Count, $Right.Selected.Count)
    $result = @()
    for ($i = 0; $i -lt $count; $i++) {
        $leftBand = $Left.Selected[$i]
        $rightBand = $Right.Selected[$i]
        $massRatio = if ($rightBand.CoverageMass -gt 0) {
            [Math]::Round($leftBand.CoverageMass / $rightBand.CoverageMass, 4)
        } else { $null }
        $inkRatio = if ($rightBand.InkPixels -gt 0) {
            [Math]::Round($leftBand.InkPixels / $rightBand.InkPixels, 4)
        } else { $null }
        $result += [pscustomobject]@{
            Label = if ($i -lt $Labels.Count) { $Labels[$i] } else { "Band$i" }
            Left = $LeftName
            Right = $RightName
            InkPixels = $leftBand.InkPixels
            ReferenceInkPixels = $rightBand.InkPixels
            InkRatio = $inkRatio
            CoverageMass = $leftBand.CoverageMass
            ReferenceCoverageMass = $rightBand.CoverageMass
            CoverageRatio = $massRatio
            RgbFringe = $leftBand.RgbFringe
            ReferenceRgbFringe = $rightBand.RgbFringe
            FringePixels = $leftBand.FringePixels
            ReferenceFringePixels = $rightBand.FringePixels
        }
    }
    return @($result)
}

if ($Capture) {
    Add-CaptureType
    [FontRenderCapture]::InitializeDpi()
    New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

    $allowScreenRecovery = $CaptureMethod -eq "Screen"
    $zentermWindow = Get-WindowCandidate "zenterm" $ZentermProcessId $ZentermTitle -AllowScreenRecovery:$allowScreenRecovery
    $referenceWindow = Get-WindowCandidate "wezterm-gui" $ReferenceProcessId $ReferenceTitle -AllowScreenRecovery:$allowScreenRecovery
    if (-not $ZentermImage) {
        $ZentermImage = Join-Path $OutputDir "zenterm-captured.png"
    }
    if (-not $ReferenceImage) {
        $ReferenceImage = Join-Path $OutputDir "wezterm-captured.png"
    }
    $zentermCapture = Save-WindowCapture $zentermWindow $ZentermImage $CaptureMethod
    $referenceCapture = Save-WindowCapture $referenceWindow $ReferenceImage $CaptureMethod
    $captureManifest = [pscustomobject]@{
        Zenterm = $zentermWindow
        Reference = $referenceWindow
        ZentermImage = $ZentermImage
        ReferenceImage = $ReferenceImage
        CaptureMethod = $CaptureMethod
        ZentermCapture = $zentermCapture
        ReferenceCapture = $referenceCapture
    }
    $captureManifest | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $OutputDir "capture.json") -Encoding utf8NoBOM
}

if (-not $ZentermImage -or -not $ReferenceImage) {
    throw "Supply -ZentermImage and -ReferenceImage, or use -Capture with process ids."
}

Add-Type -AssemblyName System.Drawing
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$analyses = [ordered]@{}
$analyses["zenterm"] = Get-ImageAnalysis $ZentermImage "zenterm"
if ($ZentermGrayscaleImage) {
    $analyses["zenterm-grayscale"] = Get-ImageAnalysis $ZentermGrayscaleImage "zenterm-grayscale"
}
$analyses["reference"] = Get-ImageAnalysis $ReferenceImage "reference"

$comparisons = [ordered]@{}
$comparisons["zenterm-vs-reference"] = @(Compare-SelectedBands $analyses["zenterm"] $analyses["reference"] "zenterm" "reference")
if ($analyses.Contains("zenterm-grayscale")) {
    $comparisons["zenterm-grayscale-vs-reference"] = @(Compare-SelectedBands $analyses["zenterm-grayscale"] $analyses["reference"] "zenterm-grayscale" "reference")
}

$report = [pscustomobject]@{
    GeneratedAt = [DateTime]::UtcNow.ToString("o")
    Parameters = [pscustomobject]@{
        Threshold = $Threshold
        MergeGap = $MergeGap
        Zoom = $Zoom
        CompareLast = $CompareLast
        Labels = $Labels
    }
    Images = $analyses
    Comparisons = $comparisons
}
$reportPath = Join-Path $OutputDir "report.json"
$report | ConvertTo-Json -Depth 12 | Set-Content $reportPath -Encoding utf8NoBOM

Write-Output "Font render report: $reportPath"
foreach ($entry in $comparisons.GetEnumerator()) {
    Write-Output "[$($entry.Key)]"
    foreach ($row in $entry.Value) {
        Write-Output ("  {0}: ink={1}/{2} ratio={3}; mass={4}/{5} ratio={6}; fringe={7}/{8} fringePixels={9}/{10}" -f `
            $row.Label, $row.InkPixels, $row.ReferenceInkPixels, $row.InkRatio, `
            $row.CoverageMass, $row.ReferenceCoverageMass, $row.CoverageRatio, `
            $row.RgbFringe, $row.ReferenceRgbFringe, $row.FringePixels, $row.ReferenceFringePixels)
    }
}
Write-Output "Zooms: $((@($analyses.Values | ForEach-Object { $_.Zooms }) -join ', '))"
