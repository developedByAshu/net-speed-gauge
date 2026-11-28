# Live test of the Windows update check: runs an older-version copy from a scratch folder, lets its
# launch check find the real GitHub release, and checks that the copy was swapped for the release
# file with the published sha256. The installed app is stopped for the run and started again after;
# its settings file is backed up first and put back byte for byte at the end.
# Usage: pwsh windows/tools/test_update_swap.ps1 -Program JuiceBar -Repo juice-bar -DataFile JuiceBar\settings.txt -OldBuild <path to an older-version exe>
param(
    [Parameter(Mandatory)][string]$Program,
    [Parameter(Mandatory)][string]$Repo,
    [Parameter(Mandatory)][string]$DataFile,
    # An exe of this program built with a lower version than the latest release.
    [Parameter(Mandatory)][string]$OldBuild,
    [int]$WaitSeconds = 90
)
$ErrorActionPreference = 'Stop'
$here = Join-Path ([IO.Path]::GetTempPath()) "$Program-update-test"
New-Item -ItemType Directory -Force (Join-Path $here 'settings-backup') | Out-Null
$work = Join-Path $here "run-$Program"
$installed = Join-Path $env:LOCALAPPDATA "Programs\$Program\$Program.exe"
$settings = Join-Path $env:APPDATA $DataFile
$backup = Join-Path $here "settings-backup\run-$Program-$(Split-Path $DataFile -Leaf)"
$keysBefore = (Get-ChildItem 'HKCU:\Control Panel\NotifyIconSettings').PSChildName

Copy-Item $settings $backup -Force
$backupHash = (Get-FileHash $backup).Hash
Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory $work | Out-Null
Copy-Item $OldBuild "$work\$Program.exe"
$oldHash = (Get-FileHash "$work\$Program.exe").Hash

$release = gh api "repos/developedByAshu/$Repo/releases/latest" | ConvertFrom-Json
$content = (Invoke-WebRequest ($release.assets | Where-Object name -eq 'SHA256SUMS.txt').browser_download_url -UseBasicParsing).Content
# GitHub serves the file as bytes; read it as text.
$sums = if ($content -is [byte[]]) { [Text.Encoding]::UTF8.GetString($content) } else { $content }
$expected = ($sums -split "`n" | Where-Object { $_ -match "  $Program-x64.exe" }) -replace '\s.*$', ''

$results = [ordered]@{}
try {
    Get-Process $Program -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 800
    $started = Get-Date
    Start-Process "$work\$Program.exe"
    $deadline = $started.AddSeconds($WaitSeconds)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 2
        if ((Get-FileHash "$work\$Program.exe").Hash -ne $oldHash) { break }
    }
    Start-Sleep -Seconds 3
    $swapped = (Get-FileHash "$work\$Program.exe").Hash.ToLower()
    $results['Release tag'] = $release.tag_name
    $results['Swapped within'] = '{0:n0} s' -f ((Get-Date) - $started).TotalSeconds
    $results['Copy now equals release sha256'] = $swapped -eq $expected
    $results['Old copy moved aside'] = Test-Path "$work\$Program.exe.old"
    $results['No half download left'] = -not (Test-Path "$work\$Program.exe.new")
    $results['Processes from scratch folder'] = @(Get-Process $Program -ErrorAction SilentlyContinue | Where-Object Path -like "$work*").Count
    $results['last_update_check saved'] = [bool](Select-String -Path $settings -Pattern '^last_update_check=[1-9]' -Quiet)
}
finally {
    Get-Process $Program -ErrorAction SilentlyContinue | Where-Object Path -like "$work*" | Stop-Process -Force
    Start-Sleep -Milliseconds 800
    Copy-Item $backup $settings -Force
    $results['Settings restored byte for byte'] = (Get-FileHash $settings).Hash -eq $backupHash
    Start-Process $installed
    $added = (Get-ChildItem 'HKCU:\Control Panel\NotifyIconSettings').PSChildName | Where-Object { $_ -notin $keysBefore }
    foreach ($key in $added) {
        $path = (Get-ItemProperty "HKCU:\Control Panel\NotifyIconSettings\$key").ExecutablePath
        if ($path -like "$work*") { Remove-Item "HKCU:\Control Panel\NotifyIconSettings\$key" -Recurse }
    }
    $results['Test tray entries removed'] = @($added).Count
    Start-Sleep -Seconds 2
    $results['Installed app running again'] = @(Get-Process $Program -ErrorAction SilentlyContinue | Where-Object Path -eq $installed).Count -eq 1
}
$results.GetEnumerator() | ForEach-Object { '{0,-34} {1}' -f $_.Key, $_.Value }
