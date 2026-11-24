# 打一个可以直接双击运行的 Windows 便携包（GitHub Release 附件用）
#
#   pwsh -File tools/package.ps1
#   pwsh -File tools/package.ps1 -Modules F:\dsh\nanxun-1938   # 顺带把战役模块也打进去
#
# 产物在 dist/ 下：
#   IronMind-<版本>-win64.zip        解压双击 IronMind.exe 就能玩
#   nanxun_1938-v<模块版本>.imod.zip  只含战役模块（给已经有引擎的人用）
#
# 为什么要有它：Release 上放一个"要用户先装 Rust 再 cargo build"的 zip 等于没发。
# 这个脚本只做复制与压缩，不碰任何逻辑——真正跑得起来与否由 dist 里的 exe 决定。

param(
  # 额外要塞进 modules/ 的模块仓库（可多个，逗号或分号分隔）
  [string]$Modules = "",
  [string]$OutDir = "dist"
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot

Push-Location $repo
try {
  # ---- 版本号：以 Cargo.toml 的 workspace 版本为准，别手写两份 ----
  $ver = (Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"').Matches[0].Groups[1].Value
  $name = "IronMind-$ver-win64"
  Write-Host "打包 钢心 $ver -> $name"

  # ---- 先编（release；顺带保证打进去的就是当前源码） ----
  Write-Host "编译 release…"
  cargo build --release -q 2>&1 | Out-Host
  foreach ($exe in @("sim-desktop.exe", "sim-server.exe")) {
    if (-not (Test-Path "target\release\$exe")) { throw "缺少 target\release\$exe，编译没成功" }
  }

  $dist = Join-Path $repo $OutDir
  $stage = Join-Path $dist $name
  if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
  New-Item -ItemType Directory -Path $stage -Force | Out-Null
  New-Item -ItemType Directory -Path (Join-Path $stage "modules") -Force | Out-Null

  # ---- 二进制：桌面外壳改名成 IronMind.exe，玩家看到的就是这个名字 ----
  Copy-Item "target\release\sim-desktop.exe" (Join-Path $stage "IronMind.exe")
  Copy-Item "target\release\sim-server.exe"  (Join-Path $stage "sim-server.exe")
  Copy-Item "LICENSE" (Join-Path $stage "LICENSE")

  # ---- 引擎自带的模块 ----
  foreach ($m in Get-ChildItem "content\scenarios" -Directory -Filter "*.imod") {
    Copy-Item -Recurse $m.FullName (Join-Path $stage "modules")
    Write-Host "  + modules/$($m.Name)"
  }

  # ---- 外部模块仓库（比如南浔线） ----
  if ($Modules) {
    foreach ($part in $Modules.Split([char[]]@(';', ','))) {
      $dir = $part.Trim()
      if (-not $dir -or -not (Test-Path $dir)) { continue }
      # 传目录就找它下面的 .imod；传 .imod 就用它本身
      $mods = if ((Get-Item $dir).Extension -eq ".imod") { @(Get-Item $dir) } else { @(Get-ChildItem $dir -Directory -Filter "*.imod") }
      foreach ($m in $mods) {
        Copy-Item -Recurse $m.FullName (Join-Path $stage "modules")
        Write-Host "  + modules/$($m.Name)（外部）"
      }
    }
  }

  # ---- 说明与配置样例 ----
  $readme = @"
钢心 / Iron Mind $ver —— 便携版（Windows x64）

怎么玩
  双击 IronMind.exe。会开一个原生窗口，首页选战役，跟着引导走。

  配置（模型 key、模块目录）在 %LOCALAPPDATA%\IronMind\settings.env，
  第一次启动会让你在界面上填，不需要手动改文件。

  存档在 %LOCALAPPDATA%\IronMind\saves。

  想装的更"正式"一点（开始菜单、桌面快捷方式、可卸载）：
  用源码仓库里的 install.ps1，见 https://github.com/Vladivostok1017/iron-mind

加战役
  把 .imod 文件夹放进本目录的 modules\ 下，重启即可在首页看到。
  也可以在「设置 → 模块目录」里导入。

关于安全提示
  这个 exe 没有代码签名证书，Windows SmartScreen 第一次会拦一下：
  「更多信息」→「仍要运行」。源码全部在 GitHub 上，可以自己 cargo build。

本包内含
  IronMind.exe        桌面外壳（原生窗口）
  sim-server.exe      本地内核（只监听 127.0.0.1，不开端口对外）
  modules\            战役模块（引擎自带的示例 + 你下载的）
  LICENSE             MIT
"@
  Set-Content -Path (Join-Path $stage "README.txt") -Value $readme -Encoding utf8

  $envExample = @"
# 复制成 %LOCALAPPDATA%\IronMind\settings.env 后按需修改；界面上能填的这里都能写。
# 不填 key 也能玩：将领退回本地规则判断，只是不会"说话"。
IRONMIND_LLM=deepseek-chat
IRONMIND_API_BASE=https://api.deepseek.com/v1
IRONMIND_API_KEY=
"@
  Set-Content -Path (Join-Path $stage "settings.env.example") -Value $envExample -Encoding utf8

  # ---- 压缩 ----
  $zip = Join-Path $dist "$name.zip"
  if (Test-Path $zip) { Remove-Item -Force $zip }
  Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip -CompressionLevel Optimal
  $sizeMb = [math]::Round((Get-Item $zip).Length / 1MB, 1)
  Write-Host "`n产物：$zip（$sizeMb MB）"

  # ---- 战役模块单独一个包（给已经有引擎的人） ----
  if ($Modules) {
    foreach ($part in $Modules.Split([char[]]@(';', ','))) {
      $dir = $part.Trim()
      if (-not $dir -or -not (Test-Path $dir)) { continue }
      $mods = if ((Get-Item $dir).Extension -eq ".imod") { @(Get-Item $dir) } else { @(Get-ChildItem $dir -Directory -Filter "*.imod") }
      foreach ($m in $mods) {
        $mj = Get-Content (Join-Path $m.FullName "module.json") -Raw | ConvertFrom-Json
        # 目录名是 `nanxun_1938.imod`，BaseName 对目录不管用，老老实实去掉扩展名
        $stem = [System.IO.Path]::GetFileNameWithoutExtension($m.Name)
        $mz = Join-Path $dist ("$stem-v$($mj.version).zip")
        if (Test-Path $mz) { Remove-Item -Force $mz }
        # 注意：压缩的是 .imod **文件夹本身**，解压后得到的才是可直接导入的目录
        Compress-Archive -Path $m.FullName -DestinationPath $mz -CompressionLevel Optimal
        Write-Host "产物：$mz（$([math]::Round((Get-Item $mz).Length/1MB,1)) MB）"
      }
    }
  }

  # ---- 校验码：Release 页面上贴一份，别人下完能自查 ----
  Write-Host "`nSHA256:"
  Get-ChildItem $dist -Filter "*.zip" | ForEach-Object {
    $h = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower()
    "  $h  $($_.Name)"
  }
} finally { Pop-Location }
