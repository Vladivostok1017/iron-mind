# install.ps1 —— 把钢心装成一个正常的 Windows 程序。
#
# 目标不是"能跑"（cargo run 就能跑），而是**双击就能玩**：
#   · 一个原生窗口，不是浏览器标签
#   · 开始菜单和桌面有图标
#   · "应用和功能"里能卸载
#   · 存档、日志、模块都在一个自己的目录里，不散在仓库里
#
# 不需要管理员：全部装在 %LOCALAPPDATA%\IronMind，注册表只写 HKCU。
#
# 用法：
#   pwsh -File install.ps1
#   pwsh -File install.ps1 -SkipBuild -Modules F:\dsh\nanxun-1938
#   pwsh -File install.ps1 -Uninstall

param(
  [string]$Dir = (Join-Path $env:LOCALAPPDATA "IronMind"),
  [string[]]$Modules = @(),
  [string]$DefaultModule = "",
  [switch]$SkipBuild,
  [switch]$NoShortcuts,
  [switch]$Uninstall
)

$ErrorActionPreference = "Stop"
$Repo = $PSScriptRoot
$AppName = "钢心"
$ExeName = "IronMind.exe"

function Step($t) { Write-Host "==> $t" -ForegroundColor Cyan }
function Info($t) { Write-Host "    $t" -ForegroundColor DarkGray }

# ── 卸载 ──
if ($Uninstall) {
  Step "卸载"
  # 卸载时程序可能正开着：先请它退出，否则文件被占用删不掉
  Get-Process -Name "IronMind", "sim-server" -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Milliseconds 500
  $lnks = @(
    (Join-Path ([Environment]::GetFolderPath('Programs')) "$AppName.lnk"),
    (Join-Path ([Environment]::GetFolderPath('Desktop')) "$AppName.lnk")
  )
  foreach ($l in $lnks) {
    if (Test-Path $l) { Remove-Item $l -Force; Info "删除快捷方式 $l" }
  }
  $reg = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\IronMind"
  if (Test-Path $reg) { Remove-Item $reg -Recurse -Force; Info "删除注册表项" }
  if (Test-Path $Dir) {
    # 存档是玩家的东西，默认留着
    $keepSaves = Join-Path $Dir "saves"
    if (Test-Path $keepSaves) {
      $bak = Join-Path ([Environment]::GetFolderPath('MyDocuments')) "IronMind-saves"
      New-Item -ItemType Directory -Force -Path $bak | Out-Null
      Copy-Item (Join-Path $keepSaves "*") $bak -Recurse -Force -ErrorAction SilentlyContinue
      Info "存档已备份到 $bak"
    }
    Remove-Item $Dir -Recurse -Force
    Info "已删除 $Dir"
  }
  Write-Host "`n卸载完成。" -ForegroundColor Green
  return
}

# ── 版本 ──
$ver = "0.1.0"
$cargo = Join-Path $Repo "Cargo.toml"
if (Test-Path $cargo) {
  $m = Select-String -Path $cargo -Pattern '^\s*version\s*=\s*"([^"]+)"' | Select-Object -First 1
  if ($m -and $m.Matches.Count) { $ver = $m.Matches[0].Groups[1].Value }
}

# ── 构建 ──
$rel = Join-Path $Repo "target\release"
if (-not $SkipBuild) {
  Step "编译（release）"
  $cargoExe = (Get-Command cargo -ErrorAction SilentlyContinue).Source
  if (-not $cargoExe) {
    $cargoExe = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
  }
  if (-not (Test-Path $cargoExe)) { throw "找不到 cargo。装一下 Rust，或者用 -SkipBuild 指定已编译好的产物。" }
  # 正在运行的话会锁住 exe，先请它退出
  Get-Process -Name "IronMind", "sim-server" -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Milliseconds 300
  & $cargoExe build --release -p sim-desktop -p sim-server --manifest-path (Join-Path $Repo "Cargo.toml")
  if ($LASTEXITCODE -ne 0) { throw "编译失败。" }
}

$desktopExe = Join-Path $rel "sim-desktop.exe"
$serverExe = Join-Path $rel "sim-server.exe"
if (-not (Test-Path $desktopExe)) { throw "没找到 $desktopExe，先编译一次（不要加 -SkipBuild）。" }
if (-not (Test-Path $serverExe)) { throw "没找到 $serverExe，先编译一次（不要加 -SkipBuild）。" }

# ── 安装目录 ──
Step "安装到 $Dir"
New-Item -ItemType Directory -Force -Path $Dir | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $Dir "modules") | Out-Null

# 拷贝前先确认没有实例在跑，否则文件被占用
Get-Process -Name "IronMind", "sim-server" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 300

Copy-Item $desktopExe (Join-Path $Dir $ExeName) -Force
Copy-Item $serverExe (Join-Path $Dir "sim-server.exe") -Force
$ico = Join-Path $Repo "crates\sim-desktop\assets\ironmind.ico"
if (Test-Path $ico) { Copy-Item $ico (Join-Path $Dir "ironmind.ico") -Force }
Info "程序文件已就位"

# ── 模块 ──
# 引擎自带的示例 + 指定的模块仓库。装好之后玩家换战役不用重新编译。
Step "安装模块"
$modDests = Join-Path $Dir "modules"
Get-ChildItem $modDests -Directory -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -like "*.imod" } | Remove-Item -Recurse -Force

$modSources = @()
$builtin = Join-Path $Repo "content\scenarios"
if (Test-Path $builtin) { $modSources += $builtin }
foreach ($m in $Modules) { if (Test-Path $m) { $modSources += $m } }
if ($env:IRONMIND_MODULES) {
  foreach ($m in ($env:IRONMIND_MODULES -split '[;,]')) { if ($m -and (Test-Path $m)) { $modSources += $m } }
}

$seen = @{}
$count = 0
foreach ($src in $modSources) {
  $src = (Resolve-Path $src).Path
  if ($seen.ContainsKey($src)) { continue }
  $seen[$src] = $true
  # 一个根目录下可能有多个 .imod；根目录本身就是 .imod 时也算一个
  $cands = @()
  if ($src -like "*.imod") { $cands = @($src) }
  else { $cands = Get-ChildItem $src -Directory -Filter "*.imod" -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName } }
  foreach ($c in $cands) {
    $name = Split-Path $c -Leaf
    if (-not (Test-Path (Join-Path $c "module.json"))) { continue }
    Copy-Item $c (Join-Path $modDests $name) -Recurse -Force
    $count++
    Info "  $name"
  }
}
if ($count -eq 0) { Info "  （没有找到模块；游戏能启动，但没战役可开）" }

# ── 配置模板 ──
$envFile = Join-Path $Dir "settings.env"
if (-not (Test-Path $envFile)) {
  $tmpl = @'
# 钢心 / Iron Mind 的配置。改完重开游戏生效。
# 一行一个 KEY=VALUE，# 开头是注释。
# 优先级：系统环境变量 > 这个文件 > 程序默认。

# 将领大脑：off = 全用本地模拟判断（不联网）；留空 = 有 key 就用真模型
# IRONMIND_LLM=off

# 真实模型（任何 OpenAI 兼容接口都行）
# IRONMIND_API_KEY=sk-................
# IRONMIND_MODEL=deepseek-chat
# IRONMIND_API_BASE=https://api.deepseek.com/v1

# 敌方统帅部多久开一次会（日）
# IRONMIND_HQ_INTERVAL=2

# 想用别处的模块仓库，取消下面这行的注释（会覆盖安装目录里的 modules/）
# IRONMIND_MODULES=D:\my-modules
'@
  Set-Content -Path $envFile -Value $tmpl -Encoding utf8
  Info "写了配置模板 settings.env（要接真模型就把 API key 填进去）"
}

# ── 默认战役 ──
# 第一次打开时先加载哪一个模块。引擎不替玩家挑战役，所以这个只能由安装者指定。
if ($DefaultModule) {
  $pick = $DefaultModule
  if (-not (Test-Path $pick)) { $pick = Join-Path (Join-Path $Dir "modules") $DefaultModule }
  if (Test-Path $pick) {
    $cur = Get-Content $envFile -Raw
    if ($cur -notmatch '(?m)^\s*IRONMIND_MODULE\s*=') {
      Add-Content -Path $envFile -Value "# 启动时默认加载的战役（安装时指定）" -Encoding utf8
      Add-Content -Path $envFile -Value ("IRONMIND_MODULE=" + (Resolve-Path $pick).Path) -Encoding utf8
    }
    Info "默认战役：$DefaultModule"
  } else {
    Write-Host "    注意：没找到默认模块 $DefaultModule" -ForegroundColor Yellow
  }
}

# ── 快捷方式 ──
if (-not $NoShortcuts) {
  Step "创建快捷方式"
  $sh = New-Object -ComObject WScript.Shell
  $targets = @((Join-Path ([Environment]::GetFolderPath('Programs')) "$AppName.lnk"))
  $desk = [Environment]::GetFolderPath('Desktop')
  if ($desk) { $targets += (Join-Path $desk "$AppName.lnk") }
  foreach ($t in $targets) {
    $lnk = $sh.CreateShortcut($t)
    $lnk.TargetPath = Join-Path $Dir $ExeName
    $lnk.WorkingDirectory = $Dir
    $lnk.Description = "钢心 · Iron Mind"
    if (Test-Path (Join-Path $Dir "ironmind.ico")) { $lnk.IconLocation = (Join-Path $Dir "ironmind.ico") }
    $lnk.Save()
    Info $t
  }
}

# ── 卸载登记（应用和功能里能看到）──
Step "登记卸载信息"
$reg = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\IronMind"
New-Item -Path $reg -Force | Out-Null
$uninstaller = Join-Path $Dir "uninstall.ps1"
@"
# 卸载钢心：删快捷方式、注册表项、程序目录。存档会先备份到"我的文档"。
& '$Repo\install.ps1' -Uninstall -Dir '$Dir'
"@ | Set-Content -Path $uninstaller -Encoding utf8
Set-ItemProperty -Path $reg -Name "DisplayName" -Value $AppName
Set-ItemProperty -Path $reg -Name "DisplayVersion" -Value $ver
Set-ItemProperty -Path $reg -Name "Publisher" -Value "Iron Mind"
Set-ItemProperty -Path $reg -Name "InstallLocation" -Value $Dir
Set-ItemProperty -Path $reg -Name "DisplayIcon" -Value (Join-Path $Dir "ironmind.ico")
Set-ItemProperty -Path $reg -Name "NoModify" -Value 1 -Type DWord
Set-ItemProperty -Path $reg -Name "NoRepair" -Value 1 -Type DWord
Set-ItemProperty -Path $reg -Name "UninstallString" -Value "pwsh -NoProfile -File `"$uninstaller`""
Info "已登记：$AppName $ver"

# ── WebView2 检查 ──
$wv2 = @(
  "C:\Program Files (x86)\Microsoft\EdgeWebView\Application",
  "C:\Program Files\Microsoft\EdgeWebView\Application"
) | Where-Object { Test-Path $_ }
if (-not $wv2) {
  Write-Host "`n注意：没检测到 WebView2 运行时。Win11 和较新的 Win10 自带；" -ForegroundColor Yellow
  Write-Host "      如果没有，装一次微软的 Evergreen 运行时即可（免费）。" -ForegroundColor Yellow
}

Write-Host "`n装好了：$AppName $ver" -ForegroundColor Green
Write-Host "  程序：$(Join-Path $Dir $ExeName)"
Write-Host "  模块：$count 个"
Write-Host "  配置：$envFile"
Write-Host "  存档：$(Join-Path $Dir 'saves')（第一次进游戏后生成）"
Write-Host "`n开始菜单或桌面上双击「$AppName」即可。" -ForegroundColor Green
