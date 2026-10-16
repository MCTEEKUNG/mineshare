# Pure process-lifecycle fixture: never stops a real process or runs an install.
$ErrorActionPreference='Stop'
$tokens=$null; $errors=$null
$ast=[System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'maintenance.ps1'),[ref]$tokens,[ref]$errors)
if($errors.Count) { throw 'Maintenance script parse failure' }
$definition=$ast.Find({param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Stop-InstallProcesses'},$true)
Invoke-Expression $definition.Extent.Text
function Fixture([int]$Age,[bool]$Exited) {
    $p=[pscustomobject]@{StartTime=[DateTime]::MinValue.AddSeconds($Age);HasExited=$Exited;Closed=$false;Child=$null}
    $p | Add-Member ScriptMethod CloseMainWindow {
        if($this.HasExited) { throw 'Process has exited' }
        $this.Closed=$true; $this.HasExited=$true
        if($this.Child) { $this.Child.HasExited=$true }
        return $true
    }
    $p | Add-Member ScriptMethod WaitForExit { param($Timeout) return $this.HasExited }
    $p | Add-Member ScriptMethod Kill { $this.HasExited=$true }
    return $p
}
$parent=Fixture 0 $false; $child=Fixture 1 $false; $parent.Child=$child
Stop-InstallProcesses @($child,$parent)
if(-not $parent.Closed -or $child.Closed) { throw 'Installer must stop parent before worker' }
Stop-InstallProcesses @((Fixture 1 $true))
Write-Output 'PASS: parent-first teardown and already-exited worker are safe'
