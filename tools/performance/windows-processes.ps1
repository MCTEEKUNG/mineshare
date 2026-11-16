# Query the image path using limited process-query rights. WMI/CIM may omit
# ExecutablePath/CommandLine even when the owning account can manage the app.
if(-not ('MineShareImageQuery' -as [type])) {
    Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class MineShareImageQuery {
    [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr OpenProcess(uint rights, bool inherit, int pid);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool QueryFullProcessImageName(IntPtr process, uint flags, StringBuilder path, ref uint size);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool TerminateProcess(IntPtr process, uint code);
    public static string PathFor(int pid) {
        IntPtr process=OpenProcess(0x1000,false,pid);
        if(process==IntPtr.Zero) return null;
        try { uint size=32768; var path=new StringBuilder((int)size); return QueryFullProcessImageName(process,0,path,ref size) ? path.ToString() : null; }
        finally { CloseHandle(process); }
    }
    public static void StopOwned(int pid) {
        IntPtr process=OpenProcess(1,false,pid);
        if(process==IntPtr.Zero) {
            int error=Marshal.GetLastWin32Error();
            if(error==87) return; // The worker may exit when its GUI exits.
            throw new System.ComponentModel.Win32Exception(error);
        }
        try { if(!TerminateProcess(process,0)) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error()); }
        finally { CloseHandle(process); }
    }
}
'@
}
function Stop-MineShareProcesses([string]$Executable) {
    foreach($item in @(Get-MineShareProcesses $Executable)) {
        [MineShareImageQuery]::StopOwned([int]$item.ProcessId)
    }
}
function Get-MineShareProcesses([string]$Executable) {
    $Executable=[IO.Path]::GetFullPath($Executable)
    foreach($item in @(Get-CimInstance Win32_Process -Filter "name='mineshare-app.exe'")) {
        $path=$item.ExecutablePath
        if(-not $path){$path=[MineShareImageQuery]::PathFor([int]$item.ProcessId)}
        if($path -and [IO.Path]::GetFullPath($path) -ieq $Executable) {
            [pscustomobject]@{ProcessId=$item.ProcessId;ParentProcessId=$item.ParentProcessId;SessionId=$item.SessionId;ExecutablePath=$path;CommandLine=$item.CommandLine}
        }
    }
}
