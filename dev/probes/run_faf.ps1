# Runs a probe map in FAF's ForgedAlliance.exe (Windows, FAF client installed)
# and writes its log to dev\probes\out\<probe>.faf.log. Run it from a desktop
# session (an ssh session has no display, so the game can't start there).
#   powershell -ExecutionPolicy Bypass -File dev\probes\run_faf.ps1 probe_air
# The init file is FAF's own init.lua (C:\ProgramData\FAForever\bin) with the
# probe maps mounted at /maps, so the game runs FAF's current Lua and data.
param([string]$probe = 'probe_air', [int]$seconds = 120)
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$bin = 'C:\ProgramData\FAForever\bin'
$out = Join-Path $here 'out'
New-Item -ItemType Directory -Force $out | Out-Null
$init = Join-Path $out 'init_probe_faf.lua'
$log = Join-Path $out "$probe.faf.log"
Remove-Item $log -ErrorAction SilentlyContinue

$maps = Join-Path $here 'maps'
# Hooks running a probe map's interface half (ui_probe.lua), if it has one.
$hooks = Join-Path $here 'ui_hooks'
$faf = Get-Content (Join-Path $bin 'init.lua') -Raw
"InitFileDir = [[$bin]]`n$faf`ntable.insert(path, 1, { dir = [[$maps]], mountpoint = '/maps' })`ntable.insert(path, 1, { dir = [[$hooks]], mountpoint = '/schook' })`n" | Set-Content -Encoding ASCII $init

# Optional: a probe map's `launch_args.txt` adds to the command line, e.g.
# `/probeai adaptive /probemod <uid>` for an all-AI game running a mod (the
# ui_hooks SinglePlayerLaunch.lua hook reads those).
$extra = @()
$argsFile = Join-Path (Join-Path $maps $probe) 'launch_args.txt'
if (Test-Path $argsFile) {
    $extra = @((Get-Content $argsFile -Raw).Trim() -split '\s+' | Where-Object { $_ })
}
$game = Start-Process -PassThru (Join-Path $bin 'ForgedAlliance.exe') -WorkingDirectory $bin -ArgumentList (@(
    '/init', "`"$init`"", '/map', $probe, '/log', "`"$log`"", '/nosound', '/nomovie', '/exitongameover') + $extra)
# A `PROBE shot <name>` log line captures the screen to out\<probe>_<name>.png
# (visual probes: the probe holds the view a few seconds after logging it).
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$shots = @{}
function Capture($path) {
    $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}
# Stop once the probe reports done (or after the timeout).
for ($i = 0; $i -lt $seconds * 4; $i++) {
    Start-Sleep -Milliseconds 250
    if ($game.HasExited) { break }
    if (-not (Test-Path $log)) { continue }
    foreach ($m in (Select-String -Path $log -Pattern 'PROBE shot (\S+)')) {
        $name = $m.Matches[0].Groups[1].Value
        if (-not $shots.ContainsKey($name)) {
            $shots[$name] = $true
            Capture (Join-Path $out "${probe}_$name.png")
        }
    }
    if (Select-String -Quiet -Path $log -Pattern 'PROBE done') { break }
}
if (-not $game.HasExited) { Stop-Process -Id $game.Id -Force }
if (Test-Path $log) {
    $n = (Select-String -Path $log -Pattern 'PROBE').Count
    "$n PROBE lines in $log"
} else {
    "no log at $log"
}
