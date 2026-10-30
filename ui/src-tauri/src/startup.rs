//! Per-user login startup. Never change task privileges or third-party launchers.
use anyhow::{Context, Result};
use serde::Serialize;

#[derive(Serialize)]
pub struct Status {
    enabled: bool,
    managed: bool,
}

static STARTUP_IO: parking_lot::Mutex<()> = parking_lot::const_mutex(());

pub fn status() -> Result<Status> {
    let _guard = STARTUP_IO.lock();
    platform::run(None)
}

pub fn set(enabled: bool) -> Result<Status> {
    let _guard = STARTUP_IO.lock();
    platform::run(Some(enabled))
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    pub fn run(change: Option<bool>) -> Result<Status> {
        // Values are passed as environment data, not interpolated into PowerShell.
        let output = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
            .env("MINESHARE_STARTUP_EXE", std::env::current_exe()?)
            .env(
                "MINESHARE_STARTUP_CHANGE",
                change.map_or("read", |on| if on { "on" } else { "off" }),
            )
            .creation_flags(0x08000000)
            .output()
            .context("read Windows startup configuration")?;
        anyhow::ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        let state = String::from_utf8_lossy(&output.stdout);
        anyhow::ensure!(
            matches!(state.trim(), "enabled" | "disabled"),
            "Unexpected Windows startup response"
        );
        Ok(Status {
            enabled: state.trim() == "enabled",
            managed: false,
        })
    }

    pub(super) const SCRIPT: &str = r#"
$ErrorActionPreference='Stop'
$exe=$env:MINESHARE_STARTUP_EXE
$change=$env:MINESHARE_STARTUP_CHANGE
$key='HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$run=$null
if(Test-Path $key){$props=Get-ItemProperty $key; if($props.PSObject.Properties['MineShare']){$run=$props.MineShare}}
$expected='"'+$exe+'"'
$task=Get-ScheduledTask -TaskName MineShareLaunch -ErrorAction SilentlyContinue
if($task){
 if($task.Actions.Count -ne 1 -or $task.Actions[0].Execute.Trim('"') -ne $exe){throw 'Startup task uses another installation. Keep one MineShare installation.'}
 if($change -ne 'read'){
  if($run -and $run -ne $expected){throw 'Another MineShare startup command exists; it was not changed.'}
  if($change -eq 'on'){
   $arguments=[string]$task.Actions[0].Arguments
   if($arguments.Trim() -notin @('', '--mineshare-autostart')){throw 'Startup task has custom arguments; they were not changed.'}
   $task.Actions[0].Arguments='--mineshare-autostart'
   # The app launcher owns the retry budget; an OS retry would multiply it.
   $task.Settings.RestartCount=0
   $task.Settings.RestartInterval=$null
   $task.Settings.StartWhenAvailable=$true
   Set-ScheduledTask -TaskName MineShareLaunch -Action $task.Actions -Settings $task.Settings | Out-Null
   Enable-ScheduledTask MineShareLaunch | Out-Null
  }else{Disable-ScheduledTask MineShareLaunch | Out-Null}
  if($run){Remove-ItemProperty $key -Name MineShare; $run=$null}
 }
 $enabled=(Get-ScheduledTask MineShareLaunch).Settings.Enabled -or [bool]$run
}else{
 if($run -and $run -ne $expected){throw 'Another MineShare startup command exists; it was not changed.'}
 if($change -eq 'on'){
  $user=[System.Security.Principal.WindowsIdentity]::GetCurrent().Name
  $action=New-ScheduledTaskAction -Execute $exe -Argument '--mineshare-autostart' -WorkingDirectory (Split-Path $exe)
  $trigger=New-ScheduledTaskTrigger -AtLogOn -User $user
  $principal=New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited
  $settings=New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -MultipleInstances IgnoreNew -ExecutionTimeLimit ([TimeSpan]::Zero)
  Register-ScheduledTask -TaskName MineShareLaunch -Action $action -Trigger $trigger -Principal $principal -Settings $settings | Out-Null
  if($run){Remove-ItemProperty $key -Name MineShare}
  'enabled'; exit 0
 }
 if($change -eq 'off' -and $run){Remove-ItemProperty $key -Name MineShare; $run=$null}
 $enabled=[bool]$run
}
if($enabled){'enabled'}else{'disabled'}
"#;
}

#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(any(target_os = "linux", test))]
mod linux {
    use super::*;
    use std::{fs, path::PathBuf};

    #[cfg(target_os = "linux")]
    fn config_root() -> Result<PathBuf> {
        if let Some(path) = std::env::var_os("XDG_CONFIG_HOME").filter(|p| !p.is_empty()) {
            return Ok(PathBuf::from(path));
        }
        Ok(PathBuf::from(std::env::var_os("HOME").context("HOME is unavailable")?).join(".config"))
    }

    #[cfg(target_os = "linux")]
    pub fn run(change: Option<bool>) -> Result<Status> {
        run_at(
            config_root()?.join("autostart"),
            &std::env::current_exe()?.to_string_lossy(),
            change,
        )
    }

    pub(super) fn run_at(root: PathBuf, exe: &str, change: Option<bool>) -> Result<Status> {
        let path = root.join("MineShare.desktop");
        let current = match fs::read_to_string(&path) {
            Ok(value) => value,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).context("read MineShare startup entry"),
        };
        let companion = fs::read_to_string(root.join("typeless.desktop")).unwrap_or_default();
        let managed = current.contains("mineshare-typeless-start")
            || companion.contains("mineshare-typeless-start");
        if let Some(enabled) = change {
            anyhow::ensure!(
                !managed,
                "Startup is managed with Typeless; preserve the ordered keyboard launcher."
            );
            let entry = if current.is_empty() {
                format!(
                    "[Desktop Entry]\nType=Application\nName=MineShare\nExec={}\nIcon=mineshare-app\nTerminal=false\n",
                    quote_exec(exe)
                )
            } else {
                anyhow::ensure!(
                    desktop_value(&current, "Type") == Some("Application"),
                    "Invalid MineShare startup entry"
                );
                current.clone()
            };
            fs::create_dir_all(&root)?;
            let temporary = root.join(format!(".MineShare-{}.tmp", std::process::id()));
            fs::write(&temporary, toggle_entry(&entry, enabled))?;
            fs::rename(&temporary, &path)?;
        }
        let saved = if change.is_some() {
            fs::read_to_string(&path)?
        } else {
            current
        };
        Ok(Status {
            enabled: (!saved.is_empty() && entry_enabled(&saved))
                || (managed
                    && companion.contains("mineshare-typeless-start")
                    && entry_enabled(&companion)),
            managed,
        })
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod platform {
    use super::*;
    pub fn run(_: Option<bool>) -> Result<Status> {
        anyhow::bail!("Login startup is supported on Windows and Ubuntu")
    }
}

// Desktop-entry helpers are pure so Windows CI can test Linux startup behavior.
#[cfg(any(target_os = "linux", test))]
fn desktop_value<'a>(entry: &'a str, key: &str) -> Option<&'a str> {
    let mut main = false;
    for line in entry.lines().map(str::trim) {
        if line.starts_with('[') {
            main = line == "[Desktop Entry]";
        }
        if main
            && let Some((name, value)) = line.split_once('=')
            && name == key
        {
            return Some(value);
        }
    }
    None
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn entry_enabled(entry: &str) -> bool {
    desktop_value(entry, "Hidden") != Some("true")
        && desktop_value(entry, "X-GNOME-Autostart-enabled") != Some("false")
}

#[cfg(any(target_os = "linux", test))]
fn quote_exec(exe: &str) -> String {
    let mut result = String::from("\"");
    for c in exe.chars() {
        match c {
            '%' => result.push_str("%%"),
            '\\' => result.push_str("\\\\\\\\"),
            '"' | '`' | '$' => {
                result.push_str("\\\\");
                result.push(c);
            }
            '\n' | '\r' => result.push(' '),
            _ => result.push(c),
        }
    }
    result.push('"');
    result
}

#[cfg(any(target_os = "linux", test))]
fn toggle_entry(entry: &str, enabled: bool) -> String {
    let mut output = String::new();
    let mut main = false;
    for line in entry.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            main = trimmed == "[Desktop Entry]";
        }
        if main
            && (trimmed.starts_with("Hidden=") || trimmed.starts_with("X-GNOME-Autostart-enabled="))
        {
            continue;
        }
        output.push_str(line);
        output.push('\n');
        if trimmed == "[Desktop Entry]" {
            output.push_str(if enabled {
                "Hidden=false\nX-GNOME-Autostart-enabled=true\n"
            } else {
                "Hidden=true\nX-GNOME-Autostart-enabled=false\n"
            });
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_enabling_uses_guard_without_multiplying_retry_budget() {
        use std::os::windows::process::CommandExt;
        let fixture = r#"
$script:originalSettings=[pscustomobject]@{Enabled=$false;RestartCount=3;RestartInterval='PT1M';StartWhenAvailable=$false;DisallowStartIfOnBatteries=$false;ExecutionTimeLimit='PT0S'}
$script:task=[pscustomobject]@{Actions=@([pscustomobject]@{Execute=$env:MINESHARE_STARTUP_EXE;Arguments=$env:MINESHARE_FIXTURE_ARGS;WorkingDirectory='C:\fixture'});Settings=$script:originalSettings;Principal='existing elevated user';Triggers=@('existing logon trigger')}
$script:updated=$false
function Test-Path { return $false }
function Get-ScheduledTask { return $script:task }
function Set-ScheduledTask {
 param($TaskName,$Action,$Settings)
 if($TaskName -ne 'MineShareLaunch' -or -not [object]::ReferenceEquals($Settings,$script:originalSettings)){throw 'Existing task settings were replaced'}
 if($Settings.RestartCount -ne 0 -or $null -ne $Settings.RestartInterval -or -not $Settings.StartWhenAvailable){throw 'OS retry would multiply the app retry budget'}
 if($Settings.DisallowStartIfOnBatteries -or $Settings.ExecutionTimeLimit -ne 'PT0S'){throw 'Unrelated task settings changed'}
 if($Action[0].Arguments -ne '--mineshare-autostart' -or $Action[0].WorkingDirectory -ne 'C:\fixture'){throw 'Login launcher action is incorrect'}
 $script:updated=$true
}
function Enable-ScheduledTask { if(-not $script:updated){throw 'Retry settings must be saved first'}; $script:task.Settings.Enabled=$true }
function Disable-ScheduledTask { throw 'Unexpected disable' }
function Register-ScheduledTask { throw 'Must not recreate the existing task' }
"#;
        let checks = r#"
if(-not $script:updated){throw 'Retry settings were not saved'}
if($script:task.Actions[0].Execute -ne $env:MINESHARE_STARTUP_EXE -or $script:task.Principal -ne 'existing elevated user' -or $script:task.Triggers[0] -ne 'existing logon trigger'){throw 'Task identity changed'}
"#;
        for arguments in ["", "--mineshare-autostart"] {
            let output = std::process::Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &(fixture.to_owned() + platform::SCRIPT + checks),
                ])
                .env("MINESHARE_STARTUP_EXE", "C:\\fixture\\mineshare-app.exe")
                .env("MINESHARE_STARTUP_CHANGE", "on")
                .env("MINESHARE_FIXTURE_ARGS", arguments)
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "enabled");
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_custom_startup_arguments_are_not_overwritten() {
        use std::os::windows::process::CommandExt;
        let fixture = r#"
$script:task=[pscustomobject]@{Actions=@([pscustomobject]@{Execute=$env:MINESHARE_STARTUP_EXE;Arguments='--custom-option'});Settings=[pscustomobject]@{Enabled=$false;RestartCount=0;RestartInterval='PT0S';StartWhenAvailable=$false}}
$script:mutated=$false
function Test-Path { return $false }
function Get-ScheduledTask { return $script:task }
function Set-ScheduledTask { $script:mutated=$true }
function Enable-ScheduledTask { $script:mutated=$true }
try {
"#;
        let checks = r#"
 throw 'Expected rejection was missing'
} catch {
 if($_.Exception.Message -notlike '*custom arguments*'){throw}
 if($script:mutated -or $script:task.Actions[0].Arguments -ne '--custom-option' -or $script:task.Settings.RestartCount -ne 0){throw 'Custom task changed'}
 'preserved'
}
"#;
        let output = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &(fixture.to_owned() + platform::SCRIPT + checks),
            ])
            .env("MINESHARE_STARTUP_EXE", "C:\\fixture\\mineshare-app.exe")
            .env("MINESHARE_STARTUP_CHANGE", "on")
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "preserved");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_status_uses_trigger_enabled_not_current_process_state() {
        use std::os::windows::process::CommandExt;
        for (enabled, expected) in [("$true", "enabled"), ("$false", "disabled")] {
            let fixture = "function Test-Path { return $false }; function Get-ScheduledTask { [pscustomobject]@{Actions=@([pscustomobject]@{Execute=$env:MINESHARE_STARTUP_EXE});State='Running';Settings=[pscustomobject]@{Enabled=__ENABLED__}} }; "
                .replace("__ENABLED__", enabled) + platform::SCRIPT;
            let output = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", &fixture])
                .env("MINESHARE_STARTUP_EXE", "C:\\fixture\\mineshare-app.exe")
                .env("MINESHARE_STARTUP_CHANGE", "read")
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
        }
    }
    #[test]
    fn linux_startup_reads_real_files_and_preserves_typeless_coordinator() {
        let root = std::env::temp_dir().join(format!(
            "mineshare-startup-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(
            !linux::run_at(root.clone(), "/usr/bin/mineshare-app", None)
                .unwrap()
                .enabled
        );
        assert!(
            linux::run_at(root.clone(), "/usr/bin/mineshare-app", Some(true))
                .unwrap()
                .enabled
        );
        assert!(
            !linux::run_at(root.clone(), "/usr/bin/mineshare-app", Some(false))
                .unwrap()
                .enabled
        );
        assert!(
            !linux::run_at(root.clone(), "/usr/bin/mineshare-app", None)
                .unwrap()
                .enabled
        );
        let coordinator = "[Desktop Entry]\nType=Application\nExec=/home/tee/.local/bin/mineshare-typeless-start\n";
        std::fs::write(root.join("MineShare.desktop"), coordinator).unwrap();
        assert!(linux::run_at(root.clone(), "unused", None).unwrap().managed);
        assert!(linux::run_at(root.clone(), "unused", Some(false)).is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("MineShare.desktop")).unwrap(),
            coordinator
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn toggles_are_idempotent_and_preserve_exec_and_other_sections() {
        let entry = "[Desktop Entry]\nType=Application\nExec=\"/path with spaces/app\" --flag\nHidden=true\n[Desktop Action Help]\nHidden=true\n";
        let on = toggle_entry(entry, true);
        assert!(entry_enabled(&on));
        assert_eq!(desktop_value(&on, "Exec"), desktop_value(entry, "Exec"));
        assert!(on.contains("[Desktop Action Help]\nHidden=true"));
        assert_eq!(on, toggle_entry(&on, true));
        assert!(!entry_enabled(&toggle_entry(&on, false)));
    }
    #[test]
    fn quoted_exec_escapes_percent_and_double_parse_characters() {
        assert_eq!(quote_exec("/home/a b/100%/app"), "\"/home/a b/100%%/app\"");
        assert!(!quote_exec("/a\napp").contains('\n'));
        assert_eq!(quote_exec("/a$app"), "\"/a\\\\$app\"");
    }
}
