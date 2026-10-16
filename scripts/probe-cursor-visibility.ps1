param([int]$Seconds = 30)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class MineShareCursorProbe {
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int x, y; }
    [StructLayout(LayoutKind.Sequential)] public struct INFO {
        public int size; public uint flags; public IntPtr cursor; public POINT position;
    }
    [DllImport("user32.dll", SetLastError=true)] public static extern bool GetCursorInfo(ref INFO info);
}
'@
$until = [DateTime]::UtcNow.AddSeconds($Seconds)
$previous = $null
$hiddenMotion = 0
$visibleMotion = 0
while ([DateTime]::UtcNow -lt $until) {
    $info = New-Object MineShareCursorProbe+INFO
    $info.size = [Runtime.InteropServices.Marshal]::SizeOf($info)
    if (-not [MineShareCursorProbe]::GetCursorInfo([ref]$info)) {
        throw "GetCursorInfo failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
    }
    $moved = $previous -and ($previous.position.x -ne $info.position.x -or $previous.position.y -ne $info.position.y)
    if ($moved) {
        if (($info.flags -band 1) -ne 0) { $visibleMotion++ } else { $hiddenMotion++ }
    }
    if (-not $previous -or $previous.flags -ne $info.flags) {
        Write-Output "$(Get-Date -Format o) cursor_flags=$($info.flags) showing=$(($info.flags -band 1) -ne 0) suppressed=$(($info.flags -band 2) -ne 0)"
    }
    $previous = $info
    Start-Sleep -Milliseconds 20
}
Write-Output "visible_motion_samples=$visibleMotion hidden_motion_samples=$hiddenMotion"
if ($hiddenMotion -gt 0) {
    Write-Output 'REPRO: cursor position changed while Windows reported no visible cursor. Correlate with the physical peer-mouse reproduction; hidden motion inside games alone is not this bug.'
    exit 2
}
Write-Output 'No hidden-motion reproduction captured in this interval; this is not an acceptance pass without the physical reproduction.'
