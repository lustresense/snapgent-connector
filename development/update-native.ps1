# Stop only a verified old executable from this checkout, then build the visible desktop.
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Known = @(
    (Join-Path $Root 'dist/native/Snapgent.exe'),
    (Join-Path $Root 'dist/candidate/Snapgent.exe'),
    (Join-Path $Root 'rust/target/release/snapgent.exe'),
    (Join-Path $Root 'Snapgent.exe')
)
$Running = @(Get-CimInstance Win32_Process -Filter "Name = 'Snapgent.exe'" | Where-Object {
    $_.ExecutablePath -and ($Known -contains $_.ExecutablePath)
})
if ($Running.Count -gt 0) {
    Write-Host 'Snapgent processes from this checkout:'
    $Running | Select-Object ProcessId, ExecutablePath, CommandLine | Format-List
    $answer = Read-Host 'Stop these Snapgent processes and rebuild? [y/N]'
    if ($answer -notmatch '^(y|yes)$') { throw 'Update canceled; no processes were stopped.' }
    foreach ($p in $Running) {
        $current = Get-CimInstance Win32_Process -Filter "ProcessId = $($p.ProcessId)"
        if ($current -and $current.ExecutablePath -eq $p.ExecutablePath -and $current.CreationDate -eq $p.CreationDate) {
            Stop-Process -Id $p.ProcessId -ErrorAction Stop
            Wait-Process -Id $p.ProcessId -Timeout 10 -ErrorAction SilentlyContinue
        }
    }
}
$ports = @(Get-NetTCPConnection -State Listen -LocalPort 17613,17614 -ErrorAction SilentlyContinue)
if ($ports.Count -gt 0) {
    $ports | Select-Object LocalAddress, LocalPort, OwningProcess | Format-Table
    throw 'A process still owns a bridge port. Inspect it manually; no unrelated process will be killed.'
}
& (Join-Path $PSScriptRoot 'build-native.ps1')
if (-not $?) { throw 'Native build failed' }
Write-Host 'Build complete. Reload the browser extension, then open start.bat.'
