# dev.ps1 —— 一条命令进入开发循环。
#
# 为什么要有：做模块时的循环原来是
#   改一行 json → 手动导入 → 手动点一天 → 看结果
# 这个循环本身让人不想调试。现在：
#   pwsh -File tools/dev.ps1 -Module ..\nanxun-1938\nanxun_1938.imod
# 改完存盘，内核自己重载，页面自己换地图。
#
# 参数：
#   -Module   模块目录。不给就用 $env:IRONMIND_MODULES 里的第一个 .imod
#   -Port     默认 8899
#   -Build    先重新编译（改了 Rust 才需要；只改 JSON 不用）

param(
  [string]$Module = "",
  [int]$Port = 8899,
  [switch]$Build
)

$ErrorActionPreference = "Stop"
$repo = Split-Path $PSScriptRoot -Parent
Set-Location $repo

if (-not $Module) {
  $roots = @()
  if ($env:IRONMIND_MODULES) { $roots += ($env:IRONMIND_MODULES -split '[;,]') }
  $roots += (Join-Path $repo "content\scenarios")
  foreach ($r in $roots) {
    if (-not (Test-Path $r)) { continue }
    $hit = Get-ChildItem $r -Directory -Filter "*.imod" -ErrorAction SilentlyContinue |
      Select-Object -First 1
    if ($hit) { $Module = $hit.FullName; break }
  }
}
if (-not $Module -or -not (Test-Path $Module)) {
  throw "找不到模块目录。用 -Module 指定，或设 IRONMIND_MODULES。"
}

$exe = Join-Path $repo "target\release\sim-server.exe"
if ($Build -or -not (Test-Path $exe)) {
  Write-Host "编译内核（release）……"
  $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
  cargo build --release -p sim-server
  if ($LASTEXITCODE -ne 0) { throw "编译失败" }
}

Write-Host ""
Write-Host "模块：$Module"
Write-Host "页面：http://127.0.0.1:$Port/"
Write-Host "改完存盘即重载；Ctrl+C 退出。"
Write-Host ""

& $exe --module $Module --port $Port --watch
