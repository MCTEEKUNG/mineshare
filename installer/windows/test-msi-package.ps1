param([Parameter(Mandatory=$true)][string]$MsiFile)
$ErrorActionPreference='Stop'
$installer=New-Object -ComObject WindowsInstaller.Installer
$db=$installer.OpenDatabase((Resolve-Path -LiteralPath $MsiFile).Path,0)
function Rows([string]$query) {
    $view=$db.OpenView($query); $null=$view.Execute()
    try { while($record=$view.Fetch()) { ,$record } } finally { $null=$view.Close() }
}
$upgrade=@(Rows 'SELECT `Value` FROM `Property` WHERE `Property` = ''UpgradeCode''')
if ($upgrade.Count -ne 1 -or $upgrade[0].StringData(1) -ne '{CA10A129-4DCB-5D4B-82C4-5CAAA33B3E4E}') { throw 'MSI upgrade identity changed' }
$searches=@(Rows 'SELECT `Property` FROM `AppSearch`' | ForEach-Object { $_.StringData(1) })
if ('MINESHARE_LEGACY_EXE' -notin $searches -or 'MINESHARE_NSIS_VERSION' -notin $searches) { throw 'MSI does not detect legacy script and NSIS installations' }
$dialogs=@(Rows 'SELECT `Dialog` FROM `Dialog`' | ForEach-Object { $_.StringData(1) })
if ('MineShareUpdateDlg' -notin $dialogs) { throw 'MSI has no explicit update UI' }
$actions=@(Rows 'SELECT `Action` FROM `CustomAction`' | ForEach-Object { $_.StringData(1) })
foreach($name in @('MineSharePrepare','MineShareComplete','MineShareRollback')) {
    if ($name -notin $actions) { throw "Missing migration action: $name" }
}
Write-Output 'PASS: MSI keeps its upgrade identity and includes legacy detection, update UI and migration actions'
