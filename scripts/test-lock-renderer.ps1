param([string]$ReportPath)
$ErrorActionPreference = 'Continue'
& "$PSScriptRoot\lock-renderer-test.exe" desktop_feedback_ --ignored --nocapture --test-threads=1 --skip desktop_feedback_visual_probe 2>&1 | Out-File -LiteralPath $ReportPath
exit $LASTEXITCODE
