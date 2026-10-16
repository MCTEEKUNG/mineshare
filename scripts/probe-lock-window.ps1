param([string]$ReportPath, [int]$ObserveSeconds = 0)
$ErrorActionPreference = 'Stop'
# Read window metadata only; never activate windows or capture desktop content.
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Collections.Generic;
public static class LockWindowProbe {
 [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c,string t);
 [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out Rect r);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h,out uint p);
 [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h,int a,out uint v,int s);
 [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr c);
 [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h,uint c);
 [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h,int c);
 public delegate bool EnumProc(IntPtr h, IntPtr p);
 [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr p);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h,StringBuilder s,int n);
 public static string[] Windows(uint pid) {
  var found=new List<string>();
  EnumWindows((h,p)=> { uint owner; GetWindowThreadProcessId(h,out owner); if(owner==pid) { var c=new StringBuilder(256); GetClassName(h,c,256); uint cloak; DwmGetWindowAttribute(h,14,out cloak,4); Rect r; GetWindowRect(h,out r); found.Add(h+" class="+c+" visible="+IsWindowVisible(h)+" cloak="+cloak+" owner="+GetWindow(h,4)+" style="+GetWindowLong(h,-20).ToString("x")+" rect="+r.Left+","+r.Top+","+r.Right+","+r.Bottom); } return true; },IntPtr.Zero);
  return found.ToArray();
 }
}
'@
[void][LockWindowProbe]::SetThreadDpiAwarenessContext([IntPtr](-4))
$result = foreach ($kind in @('feedback','foreground')) {
 $h = if ($kind -eq 'feedback') { [LockWindowProbe]::FindWindow($null,'MineShare lock feedback') } else { [LockWindowProbe]::GetForegroundWindow() }
 $r = New-Object LockWindowProbe+Rect
 [uint32]$ownerPid = 0
 [uint32]$cloaked = 0
 [void][LockWindowProbe]::GetWindowRect($h,[ref]$r)
 [void][LockWindowProbe]::GetWindowThreadProcessId($h,[ref]$ownerPid)
 $hr = [LockWindowProbe]::DwmGetWindowAttribute($h,14,[ref]$cloaked,4)
 [pscustomobject]@{kind=$kind; hwnd=$h.ToInt64(); visible=[LockWindowProbe]::IsWindowVisible($h); cloaked=$cloaked; dwmResult=$hr; rect=$r; process=if($ownerPid){(Get-Process -Id $ownerPid).ProcessName}else{''}; session=(Get-Process -Id $PID).SessionId}
}
$json = $result | ConvertTo-Json -Depth 4
$json += "`n" + ((Get-Process mineshare-app | ForEach-Object { [LockWindowProbe]::Windows($_.Id) }) -join "`n")
if ($ReportPath) { $json | Set-Content -LiteralPath $ReportPath }
$json
if ($ObserveSeconds -gt 0) {
 $until = [DateTime]::UtcNow.AddSeconds([Math]::Min($ObserveSeconds,60))
 $previous = ''
 do {
  $state = (Get-Process mineshare-app | ForEach-Object { [LockWindowProbe]::Windows($_.Id) } | Where-Object { $_ -match 'class=Static ' }) -join "`n"
  if ($state -ne $previous) {
   $line = "{0:o} {1}" -f [DateTime]::UtcNow,$state
   if ($ReportPath) { $line | Add-Content -LiteralPath $ReportPath }
   $previous = $state
  }
  Start-Sleep -Milliseconds 50
 } while ([DateTime]::UtcNow -lt $until)
}
