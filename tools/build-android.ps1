#!/usr/bin/env pwsh
<#
  一条命令做出能装的 APK。

    pwsh -File tools\build-android.ps1                          # release，内嵌南浔线
    pwsh -File tools\build-android.ps1 -Module F:\x\y.imod       # 内嵌你自己的模块
    pwsh -File tools\build-android.ps1 -Install                  # 做完直接装到连着的手机
    pwsh -File tools\build-android.ps1 -Debug                    # 调试构建（.so 大很多）

  三件事，顺序不能反：
  1. 用 NDK 的 clang 编 `libsim_mobile.so`（把选定的战役模块**打进 .so**）
  2. 把 .so 拷进 `android/app/src/main/jniLibs/arm64-v8a/`
  3. Gradle 出 APK（wry 的 Kotlin 胶水在步骤 1 由 build.rs 生成）

  为什么默认 release：debug 的 `.so` 有 60 MB，APK 会大到没法装；
  release 带 LTO，`.so` 3.8 MB。
#>
param(
  [string]$Module = "F:\dsh\nanxun-1938\nanxun_1938.imod",
  [string]$Ndk = "C:\Android\android-ndk-r27c",
  [string]$Sdk = "$env:LOCALAPPDATA\Android\Sdk",
  [string]$JavaHome = "",
  [switch]$Install,
  [switch]$Debug
)

$ErrorActionPreference = "Stop"

# Gradle 只用 wrapper 缓存里已有的那份（这台机器装过 Android Studio），
# 免得为了一个 wrapper 再下一次发行版。
#
# 按**版本号**挑最新的，不能按路径字符串排：`gradle-8.2` 字符串比
# `gradle-8.12.1` 大，但 8.2 < 8.9，而 AGP 8.7 要求 ≥ 8.9 ——
# 拿到 8.2 会直接报 "Minimum supported Gradle version is 8.9"。
function Find-Gradle {
  $dist = "$env:USERPROFILE\.gradle\wrapper\dists"
  $all = Get-ChildItem $dist -Recurse -Filter "gradle.bat" -ErrorAction SilentlyContinue |
    ForEach-Object {
      if ($_.FullName -match 'gradle-(\d+)\.(\d+)(?:\.(\d+))?') {
        $maj = [int]$Matches[1]; $min = [int]$Matches[2]
        $pat = if ($Matches[3]) { [int]$Matches[3] } else { 0 }
        [pscustomobject]@{ Path = $_.FullName; Maj = $maj; Min = $min; Pat = $pat }
      }
    } | Sort-Object Maj, Min, Pat -Descending
  if ($all) { return $all[0].Path }
  $cmd = Get-Command gradle -ErrorAction SilentlyContinue
  if ($cmd) { return $cmd.Source }
  throw "找不到 gradle（装一个，或先让 Android Studio 建个项目把 wrapper 缓存下来）"
}

function Find-JavaHome {
  $cand = @(
    "$env:LOCALAPPDATA\Programs\Eclipse Adoptium\jdk-21.0.8.9-hotspot",
    "$env:ProgramFiles\Eclipse Adoptium\jdk-21.0.8.9-hotspot",
    "$env:ProgramFiles\Java\jdk-21",
    "$env:JAVA_HOME"
  ) | Where-Object { $_ -and (Test-Path (Join-Path $_ "bin\java.exe")) }
  $hit = $cand | Select-Object -First 1
  if (-not $hit) { throw "找不到 JDK 17+（用 -JavaHome 指定）" }
  return $hit
}

$repo = Split-Path -Parent $PSScriptRoot
Push-Location $repo
try {
  $api = 24   # 和 app/build.gradle.kts 的 minSdk 对齐
  $target = "aarch64-linux-android"
  $bin = Join-Path $Ndk "toolchains\llvm\prebuilt\windows-x86_64\bin"
  if (-not (Test-Path $bin)) { throw "NDK 路径不对：$bin" }

  if (-not $JavaHome) { $JavaHome = Find-JavaHome }
  $env:JAVA_HOME = $JavaHome
  $env:ANDROID_HOME = $Sdk
  $env:ANDROID_SDK_ROOT = $Sdk

  # ---- 1. 编 .so ----
  $env:IRONMIND_BUNDLE_MODULE = $Module
  $env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = Join-Path $bin "aarch64-linux-android$api-clang.cmd"
  $env:CC_aarch64_linux_android = $env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER
  $env:AR_aarch64_linux_android = Join-Path $bin "llvm-ar.exe"
  # wry 的 build.rs 靠这三个变量生成 Kotlin 胶水。目录必须先存在。
  $env:WRY_ANDROID_PACKAGE = "com.ironmind.steelheart"
  $env:WRY_ANDROID_LIBRARY = "sim_mobile"
  $env:WRY_ANDROID_KOTLIN_FILES_OUT_DIR = Join-Path $repo "android\app\src\main\kotlin\com\ironmind\steelheart"
  New-Item -ItemType Directory -Force -Path $env:WRY_ANDROID_KOTLIN_FILES_OUT_DIR | Out-Null

  $profileArgs = @()
  $profile = "debug"
  if (-not $Debug) { $profileArgs = @("--release"); $profile = "release" }
  Write-Host "==> 编译内核（$target · $profile · 模块：$Module）" -ForegroundColor Cyan
  & cargo build -p sim-mobile --target $target @profileArgs
  if ($LASTEXITCODE -ne 0) { throw "cargo build 失败" }

  # ---- 2. 放进 jniLibs ----
  $so = Join-Path $repo "target\$target\$profile\libsim_mobile.so"
  if (-not (Test-Path $so)) { throw "没找到 $so" }
  $jni = Join-Path $repo "android\app\src\main\jniLibs\arm64-v8a"
  New-Item -ItemType Directory -Force -Path $jni | Out-Null
  Copy-Item $so (Join-Path $jni "libsim_mobile.so") -Force
  Write-Host ("     .so：{0:N1} MB" -f ((Get-Item $so).Length / 1MB))

  # ---- 3. Gradle 出包 ----
  Write-Host "==> Gradle 打包" -ForegroundColor Cyan
  $gradle = Find-Gradle
  $task = if ($Debug) { "assembleDebug" } else { "assembleRelease" }
  Push-Location (Join-Path $repo "android")
  try {
    $started = Get-Date
    & $gradle $task "--console=plain"
    # 判据是**APK 有没有出来**，不是退出码：gradle.bat 经过 cmd 包装时
    # $LASTEXITCODE 偶尔会带着上一条命令的值，信它会把成功的构建当失败。
    $code = $LASTEXITCODE
  } finally { Pop-Location }

  $apk = Get-ChildItem (Join-Path $repo "android\app\build\outputs\apk") -Recurse -Filter "*.apk" |
         Where-Object { $_.LastWriteTime -ge $started.AddSeconds(-3) } |
         Sort-Object LastWriteTime | Select-Object -Last 1
  if (-not $apk) { throw "Gradle 没产出 APK（exit=$code）" }
  if ($code -ne 0) { Write-Warning "Gradle 退出码是 $code，但 APK 已经更新，按成功处理" }
  $out = Join-Path $repo "dist\IronMind-android-$profile.apk"
  New-Item -ItemType Directory -Force -Path (Join-Path $repo "dist") | Out-Null
  Copy-Item $apk.FullName $out -Force
  Write-Host ("`nAPK：{0}（{1:N1} MB）" -f $out, ($apk.Length / 1MB)) -ForegroundColor Green

  if ($Install) {
    $adb = Join-Path $Sdk "platform-tools\adb.exe"
    Write-Host "==> 装到设备" -ForegroundColor Cyan
    & $adb install -r $out
    if ($LASTEXITCODE -ne 0) { throw "adb install 失败" }
    & $adb shell am force-stop com.ironmind.steelheart
    & $adb shell am start -n "com.ironmind.steelheart/.MainActivity"
  }
}
finally { Pop-Location }
