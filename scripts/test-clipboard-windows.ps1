# Run with powershell.exe -STA in the interactive desktop session.
# image/files replace the clipboard with public test data; probe reads metadata only.
param(
    [ValidateSet('image', 'files', 'probe')][string]$Mode = 'probe',
    [ValidateRange(1, 4096)][int]$ImageHeight = 768,
    [string]$FilePath,
    [string]$ReportPath
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
if ($Mode -eq 'image') {
    $bitmap = New-Object System.Drawing.Bitmap(1024, $ImageHeight)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.Clear([System.Drawing.Color]::CornflowerBlue)
        $data = New-Object System.Windows.Forms.DataObject
        $data.SetImage($bitmap)
        $data.SetText('MineShare clipboard regression: image must win over this text')
        [System.Windows.Forms.Clipboard]::SetDataObject($data, $true)
    } finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }
} elseif ($Mode -eq 'files') {
    if (!(Test-Path -LiteralPath $FilePath -PathType Leaf)) { throw 'A test file is required' }
    $paths = New-Object System.Collections.Specialized.StringCollection
    [void]$paths.Add((Resolve-Path -LiteralPath $FilePath).Path)
    [System.Windows.Forms.Clipboard]::SetFileDropList($paths)
}
$data = [System.Windows.Forms.Clipboard]::GetDataObject()
$result = [ordered]@{ mode = $Mode; session = (Get-Process -Id $PID).SessionId; formats = @(); width = 0; height = 0; fileHashes = @() }
if ($data) {
    $result.formats = @($data.GetFormats())
    if ([System.Windows.Forms.Clipboard]::ContainsImage()) {
        $image = [System.Windows.Forms.Clipboard]::GetImage()
        try { $result.width = $image.Width; $result.height = $image.Height } finally { $image.Dispose() }
    }
    if ([System.Windows.Forms.Clipboard]::ContainsFileDropList()) {
        $result.fileHashes = @([System.Windows.Forms.Clipboard]::GetFileDropList() | ForEach-Object {
            (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash
        })
    }
}
$json = $result | ConvertTo-Json -Compress
if ($ReportPath) { [System.IO.File]::WriteAllText($ReportPath, $json) }
$json
