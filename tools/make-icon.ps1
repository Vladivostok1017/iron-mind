# make-icon.ps1 —— 生成桌面版的图标。
#
# 为什么要有脚本而不是塞一张 png：桌面版需要两种东西，
#   1) 窗口图标：tao 要 raw RGBA（Icon::from_rgba），不能直接吃 png；
#   2) 快捷方式图标：要一个 .ico。
# 两者都从同一段画图代码出，配色改了不会有"窗口和快捷方式不一样"的问题。

param(
  [string]$OutDir = (Join-Path $PSScriptRoot "..\crates\sim-desktop\assets")
)

Add-Type -AssemblyName System.Drawing

$OutDir = [System.IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

function Draw-Face([int]$size) {
  $bmp = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
  $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit

  # 底：深蓝 → 近黑，圆角。和界面的配色是一套。
  $rect = New-Object System.Drawing.Rectangle(0, 0, $size, $size)
  $c1 = [System.Drawing.ColorTranslator]::FromHtml("#20303f")
  $c2 = [System.Drawing.ColorTranslator]::FromHtml("#0b0e12")
  $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush($rect, $c1, $c2, 90.0)
  $r = [Math]::Max(2, [int]($size * 0.18))
  $path = New-Object System.Drawing.Drawing2D.GraphicsPath
  $d = $r * 2
  $path.AddArc(0, 0, $d, $d, 180, 90)
  $path.AddArc($size - $d, 0, $d, $d, 270, 90)
  $path.AddArc($size - $d, $size - $d, $d, $d, 0, 90)
  $path.AddArc(0, $size - $d, $d, $d, 90, 90)
  $path.CloseFigure()
  $g.FillPath($brush, $path)

  # 一圈金边：小尺寸下就是那个"这是个徽记"的信号
  $penW = [Math]::Max(1.0, $size * 0.035)
  $gold = [System.Drawing.ColorTranslator]::FromHtml("#e3b04b")
  $pen = New-Object System.Drawing.Pen($gold, $penW)
  $inset = $penW / 2 + $size * 0.03
  $b = [Math]::Max(0, [int]$inset)
  $ring = New-Object System.Drawing.Rectangle($b, $b, ($size - $b * 2), ($size - $b * 2))
  $rr = [Math]::Max(1, [int]($size * 0.15))
  $ringPath = New-Object System.Drawing.Drawing2D.GraphicsPath
  $dd = $rr * 2
  $ringPath.AddArc($ring.X, $ring.Y, $dd, $dd, 180, 90)
  $ringPath.AddArc($ring.Right - $dd, $ring.Y, $dd, $dd, 270, 90)
  $ringPath.AddArc($ring.Right - $dd, $ring.Bottom - $dd, $dd, $dd, 0, 90)
  $ringPath.AddArc($ring.X, $ring.Bottom - $dd, $dd, $dd, 90, 90)
  $ringPath.CloseFigure()
  $g.DrawPath($pen, $ringPath)

  # 字：钢。字体没有就退到黑体，再没有就用系统默认，绝不因为缺字体而崩。
  $text = [char]0x94A2
  $font = $null
  foreach ($name in @("Microsoft YaHei", "SimHei", "Segoe UI")) {
    try {
      $font = New-Object System.Drawing.Font($name, ($size * 0.56), [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
      break
    } catch { }
  }
  if ($font) {
    $fmt = New-Object System.Drawing.StringFormat
    $fmt.Alignment = [System.Drawing.StringAlignment]::Center
    $fmt.LineAlignment = [System.Drawing.StringAlignment]::Center
    $tb = New-Object System.Drawing.RectangleF(0, ($size * 0.01), $size, $size)
    $light = [System.Drawing.ColorTranslator]::FromHtml("#f0d9a8")
    $tb2 = New-Object System.Drawing.SolidBrush($light)
    $g.DrawString([string]$text, $font, $tb2, $tb, $fmt)
    $tb2.Dispose(); $fmt.Dispose(); $font.Dispose()
  }
  $g.Dispose()
  return $bmp
}

# ── 1) 窗口图标：raw RGBA，32x32 ──
$small = Draw-Face 32
$rgba = New-Object byte[] (32 * 32 * 4)
for ($y = 0; $y -lt 32; $y++) {
  for ($x = 0; $x -lt 32; $x++) {
    $px = $small.GetPixel($x, $y)
    $i = ($y * 32 + $x) * 4
    $rgba[$i] = $px.R; $rgba[$i + 1] = $px.G; $rgba[$i + 2] = $px.B; $rgba[$i + 3] = $px.A
  }
}
[System.IO.File]::WriteAllBytes((Join-Path $OutDir "icon32.rgba"), $rgba)
$small.Dispose()

# ── 2) 快捷方式图标：64x64，经典 32bpp DIB（不用 png-in-ico，老 shell 也认）──
$big = Draw-Face 64
$w = 64; $h = 64
$xor = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter($xor)
# BITMAPINFOHEADER：高度写两倍（XOR + AND 两张图）
$bw.Write([uint32]40); $bw.Write([int32]$w); $bw.Write([int32]($h * 2))
$bw.Write([uint16]1); $bw.Write([uint16]32); $bw.Write([uint32]0)
$bw.Write([uint32]($w * $h * 4)); $bw.Write([int32]0); $bw.Write([int32]0)
$bw.Write([uint32]0); $bw.Write([uint32]0)
for ($y = $h - 1; $y -ge 0; $y--) {
  for ($x = 0; $x -lt $w; $x++) {
    $px = $big.GetPixel($x, $y)
    $bw.Write([byte]$px.B); $bw.Write([byte]$px.G); $bw.Write([byte]$px.R); $bw.Write([byte]$px.A)
  }
}
# AND 掩码：32bpp 下用不上，但格式要求存在，按行补零（每行 4 字节对齐）
$rowBytes = [Math]::Ceiling($w / 32.0) * 4
for ($y = 0; $y -lt $h; $y++) { for ($i = 0; $i -lt $rowBytes; $i++) { $bw.Write([byte]0) } }
$bw.Flush()
$img = $xor.ToArray()
$bw.Dispose(); $xor.Dispose()
$big.Dispose()

$ico = New-Object System.IO.MemoryStream
$ib = New-Object System.IO.BinaryWriter($ico)
$ib.Write([uint16]0); $ib.Write([uint16]1); $ib.Write([uint16]1)   # ICONDIR
$ib.Write([byte]$w); $ib.Write([byte]$h); $ib.Write([byte]0); $ib.Write([byte]0)
$ib.Write([uint16]1); $ib.Write([uint16]32)                        # planes, bpp
$ib.Write([uint32]$img.Length); $ib.Write([uint32]22)              # 数据长、偏移（6+16=22）
$ib.Write($img)
$ib.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $OutDir "ironmind.ico"), $ico.ToArray())
$ib.Dispose(); $ico.Dispose()

Write-Host ("icon ok -> {0}" -f $OutDir)

# 预览图：给人看的。图标这种东西不看不放心，128x128 存一张到 _shots 里。
$preview = Draw-Face 128
$previewDir = Join-Path $PSScriptRoot "_shots"
New-Item -ItemType Directory -Force -Path $previewDir | Out-Null
$preview.Save((Join-Path $previewDir "icon-preview.png"), [System.Drawing.Imaging.ImageFormat]::Png)
$preview.Dispose()

Get-ChildItem $OutDir | Select-Object Name, Length | Format-Table -AutoSize
