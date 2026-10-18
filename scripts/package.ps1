param([switch]$IncludeInstaller)

$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$configuration = Get-Content -LiteralPath (Join-Path $projectRoot 'src-tauri\tauri.conf.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$version = "$($configuration.version).0"
$packageRoot = [IO.Path]::GetFullPath((Join-Path $projectRoot 'package'))
if ($packageRoot -ne (Join-Path $projectRoot 'package')) { throw '发布目录必须位于项目内' }
$buildRoot = Join-Path $projectRoot 'src-tauri\target\release'
$executable = Join-Path $buildRoot 'clouddownloader.exe'
if (-not ('CloudDownloaderVersionLanguage' -as [type])) {
    Add-Type @'
using System.Runtime.InteropServices;
using System.Text;
public static class CloudDownloaderVersionLanguage {
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    public static extern uint VerLanguageName(uint language, StringBuilder buffer, uint size);
}
'@
}
$languageName = New-Object Text.StringBuilder 256
[CloudDownloaderVersionLanguage]::VerLanguageName(0x0409, $languageName, 256) | Out-Null
$expected = @{
    CompanyName = 'sixunited'
    FileDescription = 'Cloud file and transfer manager for Windows'
    ProductName = 'CloudDownloader'
    FileVersion = $version
    ProductVersion = $version
    LegalCopyright = 'Copyright (c) 2026 sixunited. Author: sclliang'
    InternalName = 'CloudDownloader'
    OriginalFilename = 'CloudDownloader.exe'
    Language = $languageName.ToString()
}
$metadata = [Diagnostics.FileVersionInfo]::GetVersionInfo($executable)
foreach ($key in $expected.Keys) {
    if ($metadata.$key -cne $expected[$key]) { throw "主程序元数据检查失败：$key = $($metadata.$key)" }
}
$numericVersion = '{0}.{1}.{2}.{3}' -f $metadata.FileMajorPart, $metadata.FileMinorPart, $metadata.FileBuildPart, $metadata.FilePrivatePart
$numericProductVersion = '{0}.{1}.{2}.{3}' -f $metadata.ProductMajorPart, $metadata.ProductMinorPart, $metadata.ProductBuildPart, $metadata.ProductPrivatePart
if ($numericVersion -ne $version -or $numericProductVersion -ne $version) {
    throw '主程序数字版本检查失败'
}
New-Item -ItemType Directory -Path (Join-Path $packageRoot 'bin') -Force | Out-Null
$packageExecutable = Join-Path $packageRoot 'CloudDownloader.exe'
if (-not (Test-Path -LiteralPath $packageExecutable) -or
    (Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash -ne
    (Get-FileHash -LiteralPath $packageExecutable -Algorithm SHA256).Hash) {
    Copy-Item -LiteralPath $executable -Destination $packageExecutable -Force
}
$engine = Join-Path $projectRoot 'src-tauri\binaries\aria2c-x86_64-pc-windows-msvc.exe'
if ((Get-FileHash -LiteralPath $engine -Algorithm SHA256).Hash -ne 'BE2099C214F63A3CB4954B09A0BECD6E2E34660B886D4C898D260FEBFE9D70C2') {
    throw 'aria2 引擎校验失败'
}
$packageEngine = Join-Path $packageRoot 'bin\aria2c.exe'
if (-not (Test-Path -LiteralPath $packageEngine) -or
    (Get-FileHash -LiteralPath $packageEngine -Algorithm SHA256).Hash -ne
    'BE2099C214F63A3CB4954B09A0BECD6E2E34660B886D4C898D260FEBFE9D70C2') {
    Copy-Item -LiteralPath $engine -Destination $packageEngine -Force
}
Copy-Item -LiteralPath (Join-Path $projectRoot 'third-party\notices') -Destination (Join-Path $packageRoot 'bin') -Recurse -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'README.md') -Destination $packageRoot -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'VERIFICATION.md') -Destination $packageRoot -Force

if ($IncludeInstaller) {
    $nsisRoot = Join-Path $buildRoot 'nsis\x64'
    $generated = Join-Path $nsisRoot 'installer.nsi'
    $source = [IO.File]::ReadAllText($generated)
    $appVersion = $configuration.version
    if (-not $source.Contains('!define VERSION "' + $appVersion + '"')) {
        throw 'NSIS 脚本版本不符，请先运行 Tauri NSIS 构建'
    }
    $installerName = "CloudDownloader_$($appVersion)_x64-setup.exe"
    $installerPath = Join-Path $packageRoot $installerName
    # Tauri's generated installer uses the UI language and three-part strings
    # for VERSIONINFO. Keep its install logic, replacing only that resource block.
    $versionBlock = @'
VIProductVersion "${VERSIONWITHBUILD}"
VIFileVersion "${VERSIONWITHBUILD}"
VIAddVersionKey /LANG=1033 "CompanyName" "sixunited"
VIAddVersionKey /LANG=1033 "ProductName" "CloudDownloader"
VIAddVersionKey /LANG=1033 "FileDescription" "CloudDownloader installer for Windows"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Copyright (c) 2026 sixunited. Author: sclliang"
VIAddVersionKey /LANG=1033 "FileVersion" "${VERSIONWITHBUILD}"
VIAddVersionKey /LANG=1033 "ProductVersion" "${VERSIONWITHBUILD}"
VIAddVersionKey /LANG=1033 "InternalName" "CloudDownloaderSetup"
VIAddVersionKey /LANG=1033 "OriginalFilename" "__INSTALLER_FILENAME__"
'@
    $resourcePattern = '(?m)^VIProductVersion[^\r\n]*\r?\n(?:VIAddVersionKey[^\r\n]*\r?\n)+'
    if ([regex]::Matches($source, $resourcePattern).Count -ne 1) {
        throw 'Tauri 的 NSIS 版本资源格式变化，请检查安装脚本'
    }
    $source = [regex]::Replace($source, $resourcePattern, [Text.RegularExpressions.MatchEvaluator]{
        param($match)
        $versionBlock.Replace('__INSTALLER_FILENAME__', $installerName) + [Environment]::NewLine
    })
    $outputLine = '!define OUTFILE "' + $installerPath.Replace('$', '$$') + '"'
    $source = [regex]::Replace($source, '(?m)^!define OUTFILE "[^"\r\n]+"', [Text.RegularExpressions.MatchEvaluator]{param($match) $outputLine})
    $versionedScript = Join-Path $nsisRoot 'installer-versioned.nsi'
    [IO.File]::WriteAllText($versionedScript, $source, [Text.UTF8Encoding]::new($false))
    $compiler = Join-Path $env:LOCALAPPDATA 'tauri\NSIS\makensis.exe'
    if (-not (Test-Path -LiteralPath $compiler)) { throw '找不到 Tauri 缓存中的 NSIS 编译器' }
    Push-Location $nsisRoot
    try {
        & $compiler /V2 $versionedScript
        if ($LASTEXITCODE -ne 0) { throw '安装包编译失败' }
    } finally { Pop-Location }
    $installerMetadata = [Diagnostics.FileVersionInfo]::GetVersionInfo($installerPath)
    $installerExpected = $expected.Clone()
    $installerExpected.FileDescription = 'CloudDownloader installer for Windows'
    $installerExpected.OriginalFilename = $installerName
    $installerExpected.InternalName = 'CloudDownloaderSetup'
    foreach ($key in $installerExpected.Keys) {
        if ($installerMetadata.$key -cne $installerExpected[$key]) { throw "安装包元数据检查失败：$key" }
    }
    $installerNumeric = '{0}.{1}.{2}.{3}' -f $installerMetadata.FileMajorPart, $installerMetadata.FileMinorPart, $installerMetadata.FileBuildPart, $installerMetadata.FilePrivatePart
    if ($installerNumeric -ne $version) { throw '安装包数字版本检查失败' }

    # Publish only the portable payload, never old installers or local test data.
    $expectedBin = @('aria2c.exe') + @(Get-ChildItem -LiteralPath (Join-Path $projectRoot 'third-party\notices') -File | ForEach-Object { 'notices\' + $_.Name })
    $actualBin = @(Get-ChildItem -LiteralPath (Join-Path $packageRoot 'bin') -Recurse -File | ForEach-Object { $_.FullName.Substring((Join-Path $packageRoot 'bin').Length + 1) })
    if (Compare-Object ($expectedBin | Sort-Object) ($actualBin | Sort-Object)) { throw '便携包 bin 目录含有非预期文件，请检查后发布' }
    $zipPath = Join-Path $packageRoot "CloudDownloader-$appVersion-win-x64.zip"
    $portable = @('CloudDownloader.exe', 'bin', 'README.md', 'VERIFICATION.md') | ForEach-Object { Join-Path $packageRoot $_ }
    Compress-Archive -LiteralPath $portable -DestinationPath $zipPath -Force
    $checksums = @($installerPath, $zipPath) | ForEach-Object {
        (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash.ToLowerInvariant() + '  ' + [IO.Path]::GetFileName($_)
    }
    [IO.File]::WriteAllLines((Join-Path $packageRoot "SHA256SUMS-$appVersion.txt"), $checksums, [Text.UTF8Encoding]::new($false))
}
Get-ChildItem -LiteralPath $packageRoot | Select-Object Name, Length
