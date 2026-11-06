# Windows version of package.sh: builds faf-lua-debug.vsix (a VSIX is a zip:
# a manifest plus the extension's files under extension/) without needing
# vsce or a download, and installs it into VS Code. Re-run after changing the
# extension. See docs/guides/debugging-lua.md.
#
#   powershell -ExecutionPolicy Bypass -File dev\vscode-lua-debug\package.ps1 [-Out <path.vsix>]
param([string]$Out = "")
$ErrorActionPreference = "Stop"
$here = $PSScriptRoot
if (-not $Out) { $Out = Join-Path $here "..\..\target\faf-lua-debug.vsix" }
$Out = [System.IO.Path]::GetFullPath($Out)

$version = (Select-String -Path (Join-Path $here "package.json") -Pattern '"version":\s*"([^"]+)"').Matches[0].Groups[1].Value

$contentTypes = '<?xml version="1.0" encoding="utf-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension=".json" ContentType="application/json"/><Default Extension=".js" ContentType="application/javascript"/><Default Extension=".vsixmanifest" ContentType="text/xml"/></Types>
'
$manifest = @"
<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011" xmlns:d="http://schemas.microsoft.com/developer/vsx-schema-design/2011">
  <Metadata>
    <Identity Language="en-US" Id="faf-lua-debug" Version="$version" Publisher="rts-core-engine"/>
    <DisplayName>FAF Lua debugger (rts-core-engine)</DisplayName>
    <Description xml:space="preserve">Debug the game's Lua in rts-core-engine (--lua-debug).</Description>
    <Categories>Debuggers</Categories>
    <Properties><Property Id="Microsoft.VisualStudio.Code.Engine" Value="^1.80.0"/></Properties>
  </Metadata>
  <Installation><InstallationTarget Id="Microsoft.VisualStudio.Code"/></Installation>
  <Dependencies/>
  <Assets><Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true"/></Assets>
</PackageManifest>
"@

# Built entry by entry: Compress-Archive treats "[Content_Types].xml" as a
# wildcard, and older versions write '\' in entry names, which VS Code rejects.
Add-Type -AssemblyName System.IO.Compression
New-Item -ItemType Directory -Force -Path (Split-Path $Out) | Out-Null
if (Test-Path $Out) { Remove-Item $Out }
$utf8 = New-Object System.Text.UTF8Encoding($false)
$stream = [System.IO.File]::Open($Out, [System.IO.FileMode]::CreateNew)
$zip = New-Object System.IO.Compression.ZipArchive($stream, [System.IO.Compression.ZipArchiveMode]::Create)
try {
    $entries = [ordered]@{
        "[Content_Types].xml"    = $utf8.GetBytes($contentTypes)
        "extension.vsixmanifest" = $utf8.GetBytes($manifest)
        "extension/package.json" = [System.IO.File]::ReadAllBytes((Join-Path $here "package.json"))
        "extension/extension.js" = [System.IO.File]::ReadAllBytes((Join-Path $here "extension.js"))
    }
    foreach ($name in $entries.Keys) {
        $bytes = $entries[$name]
        $s = $zip.CreateEntry($name).Open()
        $s.Write($bytes, 0, $bytes.Length)
        $s.Close()
    }
} finally {
    $zip.Dispose()
    $stream.Dispose()
}
Write-Host "built $Out"

# `code` on PATH (code.cmd), else the user or system install.
$code = (Get-Command code -ErrorAction SilentlyContinue).Source
if (-not $code) {
    $code = @(
        "$env:LOCALAPPDATA\Programs\Microsoft VS Code\bin\code.cmd",
        "$env:ProgramFiles\Microsoft VS Code\bin\code.cmd"
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if ($code) {
    & $code --install-extension $Out --force
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} else {
    Write-Host "VS Code's 'code' command not found: install $Out with Extensions -> ... -> Install from VSIX..."
}
