# Release test for one app: updates the installed app ONLY through Settings > Check now, then checks
# the version, every Settings page, the tray or readout clicks and their popups, and the tray place.
# Clicks are window messages posted to the app, so the real pointer and keyboard are never touched.
# The settings file is compared with its backup at the end and put back byte for byte if it changed.
# Usage: pwsh windows/tools/release_update_test.ps1 -App NetSpeedGauge -Tag v1.3.1 -Backup <folder holding the settings backup>
param(
    [Parameter(Mandatory)][ValidateSet('JuiceBar', 'NetSpeedGauge')][string]$App,
    [Parameter(Mandatory)][string]$Tag,
    [Parameter(Mandatory)][string]$Backup
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type -Namespace RT -Name W -MemberDefinition @'
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
[DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
[DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
[DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
'@
$WM_APP = 0x8000; $WM_CLOSE = 0x0010; $WM_LBUTTONUP = 0x0202; $WM_RBUTTONUP = 0x0205; $WM_KEYDOWN = 0x0100; $VK_ESCAPE = 0x1B
$out = Join-Path ([IO.Path]::GetTempPath()) "$App-release-test"
New-Item -ItemType Directory -Force $out | Out-Null
$exe = Join-Path $env:LOCALAPPDATA "Programs\$App\$App.exe"
$settingsFile = if ($App -eq 'JuiceBar') { Join-Path $env:APPDATA 'JuiceBar\settings.txt' } else { Join-Path $env:APPDATA 'NetSpeedGauge\settings.ini' }
$backupFile = Join-Path $Backup (Split-Path $settingsFile -Leaf | ForEach-Object { "$App-$_" })
$settingsClass = if ($App -eq 'JuiceBar') { 'JuiceBarSettings' } else { 'NetSpeedGaugeSettings' }
$pages = if ($App -eq 'JuiceBar') { 'general', 'appearance', 'devices', 'advanced', 'about' } else { 'general', 'appearance', 'units', 'network', 'advanced', 'details' }
$results = [ordered]@{}

function Find-Window($class) { [RT.W]::FindWindow($class, [NullString]::Value) }
function Wait-Window($class, $seconds = 10) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) { $h = Find-Window $class; if ($h -ne [IntPtr]::Zero -and [RT.W]::IsWindowVisible($h)) { return $h }; Start-Sleep -Milliseconds 200 }
    return [IntPtr]::Zero
}
function Save-View($handle, $name) {
    Start-Sleep -Milliseconds 400
    $r = New-Object RT.W+RECT; [RT.W]::GetWindowRect($handle, [ref]$r) | Out-Null
    $bitmap = New-Object Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
    $graphics = [Drawing.Graphics]::FromImage($bitmap); $dc = $graphics.GetHdc()
    [RT.W]::PrintWindow($handle, $dc, 2) | Out-Null
    $graphics.ReleaseHdc($dc); $bitmap.Save((Join-Path $out "$name.png")); $graphics.Dispose(); $bitmap.Dispose()
}
function Send-Click($handle, $x, $y) { [RT.W]::PostMessage($handle, $WM_LBUTTONUP, [IntPtr]::Zero, [IntPtr](($y -shl 16) -bor $x)) | Out-Null }
function Open-Settings {
    if ($App -eq 'JuiceBar') {
        # Juice Bar opens a page at start only, so it restarts on Settings; Check now swaps the copy at the same path.
        Get-Process $App -ErrorAction SilentlyContinue | Stop-Process -Force; Start-Sleep -Milliseconds 800
        Start-Process $exe -ArgumentList '--open-settings=general'
    } else {
        [RT.W]::PostMessage((Find-Window 'NetSpeedGaugeReadout'), $WM_APP + 3, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
    }
    $h = Wait-Window $settingsClass
    if ($h -eq [IntPtr]::Zero) { throw 'Settings did not open' }
    return $h
}
function Get-Promoted {
    Get-ChildItem 'HKCU:\Control Panel\NotifyIconSettings' | ForEach-Object { Get-ItemProperty $_.PSPath } | Where-Object ExecutablePath -eq $exe | ForEach-Object { "$($_.PSChildName)=$($_.IsPromoted)" }
}

$backupHash = (Get-FileHash $backupFile).Hash
$version = $Tag.TrimStart('v')
$release = gh api "repos/developedByAshu/$(if ($App -eq 'JuiceBar') { 'juice-bar' } else { 'net-speed-gauge' })/releases/tags/$Tag" | ConvertFrom-Json
$content = (Invoke-WebRequest ($release.assets | Where-Object name -eq 'SHA256SUMS.txt').browser_download_url -UseBasicParsing).Content
$sums = if ($content -is [byte[]]) { [Text.Encoding]::UTF8.GetString($content) } else { $content }
$expected = (($sums -split "`n" | Where-Object { $_ -match "  $App-x64.exe" }) -replace '\s.*$', '').Trim()
$before = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($exe))
$results['Version before'] = (@('0.1.1', '0.1.2', '1.3.0', '1.3.1') | Where-Object { $before.Contains("$_") } | Select-Object -First 1)
$promotedBefore = Get-Promoted

# 1. Update only through Check now.
$settings = [IntPtr]@(Open-Settings)[-1]
Save-View $settings 'before-general'
$r = New-Object RT.W+RECT; [RT.W]::GetWindowRect($settings, [ref]$r) | Out-Null
$origin = New-Object RT.W+POINT; [RT.W]::ClientToScreen($settings, [ref]$origin) | Out-Null
$check = if ($App -eq 'JuiceBar') { @(640, 375) } else { @([int](649 - ($origin.X - $r.L)), [int](609 - ($origin.Y - $r.T))) }
$started = Get-Date
Send-Click $settings $check[0] $check[1]
Start-Sleep -Seconds 2; Save-View $settings 'after-check-now-click'
$closed = $false
while (((Get-Date) - $started).TotalSeconds -lt 180) {
    if (-not $closed -and (Test-Path "$exe.new")) { Save-View $settings 'update-waiting'; [RT.W]::PostMessage($settings, $WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null; $closed = $true }
    $hash = (Get-FileHash $exe -ErrorAction SilentlyContinue).Hash
    if ($hash -and $hash.ToLower() -eq $expected -and (Get-Process $App -ErrorAction SilentlyContinue | Where-Object Path -eq $exe)) { break }
    Start-Sleep -Seconds 1
}
$results['Update time'] = '{0:n0} s after Check now' -f ((Get-Date) - $started).TotalSeconds
$results['Installed file = release sha256'] = (Get-FileHash $exe).Hash.ToLower() -eq $expected
Start-Sleep -Seconds 3
$results['Relaunched by itself'] = [bool](Get-Process $App -ErrorAction SilentlyContinue | Where-Object Path -eq $exe)
$results['Version after'] = if ([Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($exe)).Contains($version)) { $version } else { 'NOT ' + $version }
$results['Leftover .old/.new removed'] = @(Get-ChildItem "$exe.*" -ErrorAction SilentlyContinue).Count -eq 0

# 2. Every Settings page opens.
$settings = [IntPtr]@(Open-Settings)[-1]
$opened = 0
foreach ($position in 0..($pages.Count - 1)) {
    Send-Click $settings 90 (62 + $position * 40 + 18)
    Save-View $settings "page-$($pages[$position])"; $opened++
}
$results['Settings pages opened'] = "$opened of $($pages.Count)"
[RT.W]::PostMessage($settings, $WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
Start-Sleep -Seconds 1

# 3. Left and right clicks and their popups.
if ($App -eq 'JuiceBar') {
    $main = Find-Window 'JuiceBarMain'
    # The tray's version 4 events: NIN_SELECT (0x400) for a left click, WM_CONTEXTMENU (0x7B) for a right click.
    [RT.W]::PostMessage($main, $WM_APP + 1, [IntPtr]::Zero, [IntPtr]0x400) | Out-Null
    $flyout = Wait-Window 'JuiceBarFlyout' 5
    $results['Left click opens the devices flyout'] = $flyout -ne [IntPtr]::Zero
    if ($flyout -ne [IntPtr]::Zero) { Save-View $flyout 'left-click-flyout'; [RT.W]::PostMessage($flyout, $WM_KEYDOWN, [IntPtr]$VK_ESCAPE, [IntPtr]::Zero) | Out-Null }
    Start-Sleep -Seconds 1
    [RT.W]::PostMessage($main, $WM_APP + 1, [IntPtr]::Zero, [IntPtr]0x7B) | Out-Null
    $menu = Wait-Window 'JuiceBarFlyout' 5
    $results['Right click opens the menu'] = $menu -ne [IntPtr]::Zero
    if ($menu -ne [IntPtr]::Zero) { Save-View $menu 'right-click-menu'; [RT.W]::PostMessage($menu, $WM_KEYDOWN, [IntPtr]$VK_ESCAPE, [IntPtr]::Zero) | Out-Null }
} else {
    $readout = Find-Window 'NetSpeedGaugeReadout'
    [RT.W]::PostMessage($readout, $WM_RBUTTONUP, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
    $menu = Wait-Window 'NetSpeedGaugePopup' 5
    $results['Right click opens the menu'] = $menu -ne [IntPtr]::Zero
    if ($menu -ne [IntPtr]::Zero) { Save-View $menu 'right-click-menu'; [RT.W]::PostMessage($menu, $WM_KEYDOWN, [IntPtr]$VK_ESCAPE, [IntPtr]::Zero) | Out-Null }
    Start-Sleep -Seconds 1
    [RT.W]::PostMessage($readout, $WM_LBUTTONUP, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
    $card = Wait-Window 'NetSpeedGaugePopup' 5
    $usage = Find-Window 'NetSpeedGaugeUsage'
    $results['Left click opens the usage card or window'] = ($card -ne [IntPtr]::Zero) -or ($usage -ne [IntPtr]::Zero -and [RT.W]::IsWindowVisible($usage))
    if ($card -ne [IntPtr]::Zero) { Save-View $card 'left-click-card' }
    [RT.W]::PostMessage($readout, $WM_LBUTTONUP, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
}
Start-Sleep -Seconds 1
$results['Still running after the clicks'] = [bool](Get-Process $App -ErrorAction SilentlyContinue | Where-Object Path -eq $exe)
$results['Tray place unchanged'] = ((Get-Promoted) -join ',') -eq ($promotedBefore -join ',')

# 4. Settings back to the backup, byte for byte.
$changed = (Get-FileHash $settingsFile).Hash -ne $backupHash
if ($changed) { Copy-Item $backupFile $settingsFile -Force }
$results['Settings changed by the test'] = if ($changed) { 'yes (last check time), restored' } else { 'no' }
$results['Settings byte-identical to backup'] = (Get-FileHash $settingsFile).Hash -eq $backupHash
$results.GetEnumerator() | ForEach-Object { '{0,-42} {1}' -f $_.Key, $_.Value }
