//! Per-user, explicit-consent Ubuntu setup. No actions run during detection.
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize)]
pub struct Status {
    pub linux: bool,
    pub session: String,
    pub gnome_supported: bool,
    pub wayland: bool,
    pub input_active: bool,
    pub uinput_available: bool,
    pub input_needs_repair: bool,
    pub input_configured: bool,
    pub typeless: bool,
    pub broker_configured: bool,
    pub broker_active: bool,
    pub extension_configured: bool,
    pub extension_active: bool,
    pub extensions_allowed: bool,
    pub helpers_available: bool,
    pub pending_login: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub input: bool,
    pub broker: bool,
    pub pointer: bool,
}

pub fn status() -> anyhow::Result<Status> {
    #[cfg(target_os = "linux")]
    {
        linux::detect()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(Status::default())
    }
}

pub fn configure(options: Options) -> anyhow::Result<Status> {
    #[cfg(target_os = "linux")]
    {
        linux::configure(options)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (options.input, options.broker, options.pointer);
        anyhow::bail!("Ubuntu setup is only available on Linux")
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use anyhow::{Context, bail, ensure};
    use std::{
        path::{Path, PathBuf},
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    const UUID: &str = "mineshare-pointer@mineshare.local";
    const ROOT: &str = "/usr/share/mineshare/setup/installer/linux";

    fn command(program: &str, args: &[&str], timeout: Duration) -> anyhow::Result<String> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Start {program}"))?;
        let start = Instant::now();
        loop {
            if child.try_wait()?.is_some() {
                break;
            }
            if start.elapsed() > timeout {
                let _ = child.kill();
                let _ = child.wait();
                bail!("{program} timed out; setup was not completed");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output()?;
        ensure!(
            output.status.success(),
            "{program}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }
    fn query(program: &str, args: &[&str]) -> Option<String> {
        command(program, args, Duration::from_secs(3)).ok()
    }
    fn config() -> anyhow::Result<PathBuf> {
        Ok(std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .context("Missing user configuration directory")?)
    }
    fn helper(name: &str) -> anyhow::Result<PathBuf> {
        ensure!(
            !name.contains('/') && name.ends_with(".sh"),
            "Invalid helper name"
        );
        let path = Path::new(ROOT).join(name);
        ensure!(
            path.is_file(),
            "Setup files missing. Install the current MineShare .deb package."
        );
        Ok(path)
    }
    fn has_group(text: &str) -> bool {
        text.split_whitespace().any(|g| g == "input")
    }
    fn same_file(left: &Path, right: &Path) -> bool {
        match (std::fs::read(left), std::fs::read(right)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }
    fn pointer_active(text: &str) -> bool {
        let numbers: Option<Vec<i32>> = text
            .trim()
            .trim_start_matches('(')
            .trim_end_matches(')')
            .split(',')
            .map(|v| v.trim().parse().ok())
            .collect();
        let Some(numbers) = numbers else {
            return false;
        };
        numbers.len() == 4 && numbers[2] > 0 && numbers[3] > 0
    }
    pub(super) fn detect() -> anyhow::Result<Status> {
        let root = config()?;
        let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
        let version = query("gnome-shell", &["--version"]).unwrap_or_default();
        let major = version
            .split_whitespace()
            .find_map(|word| word.split('.').next()?.parse::<u32>().ok());
        let gnome = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_lowercase()
            .contains("gnome");
        let wayland = session == "wayland" || std::env::var_os("WAYLAND_DISPLAY").is_some();
        let group_active = has_group(&query("id", &["-nG"]).unwrap_or_default());
        let input_active = group_active
            && std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/uinput")
                .is_ok();
        let user = query("id", &["-un"]).context("Cannot identify desktop user")?;
        let input_configured = has_group(&query("id", &["-nG", &user]).unwrap_or_default());
        let home = PathBuf::from(std::env::var_os("HOME").context("Missing HOME")?);
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        let broker_configured = root.join("MineShare/keyboard-broker.enabled").is_file()
            && crate::startup::entry_enabled(
                &std::fs::read_to_string(root.join("autostart/MineShare.desktop"))
                    .unwrap_or_default(),
            )
            && same_file(
                &root.join("autostart/MineShare.desktop"),
                &root.join("autostart/typeless.desktop"),
            )
            && same_file(
                &home.join(".local/bin/mineshare-typeless-start"),
                &Path::new(ROOT).join("start-typeless-after-mineshare.sh"),
            )
            && std::fs::read_to_string(root.join("autostart/MineShare.desktop"))
                .unwrap_or_default()
                .contains("mineshare-typeless-start");
        let broker_active = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .and_then(|p| std::fs::read_to_string(p.join("mineshare-keyboard-broker.ready")).ok())
            .is_some_and(|text| {
                text.lines().next() == Some(std::process::id().to_string().as_str())
                    && text
                        .lines()
                        .nth(1)
                        .and_then(|n| n.parse::<u32>().ok())
                        .is_some_and(|n| n > 0)
            });
        let enabled = query(
            "gsettings",
            &["get", "org.gnome.shell", "enabled-extensions"],
        )
        .unwrap_or_default();
        let extension_configured = enabled.contains(UUID)
            && ["metadata.json", "extension.js"].iter().all(|name| {
                same_file(
                    &data.join("gnome-shell/extensions").join(UUID).join(name),
                    &Path::new(ROOT).join("gnome-pointer").join(name),
                )
            });
        let extensions_allowed = query(
            "gsettings",
            &["get", "org.gnome.shell", "disable-user-extensions"],
        )
        .as_deref()
            == Some("false");
        let extension_active = query(
            "gdbus",
            &[
                "call",
                "--session",
                "--dest",
                "org.gnome.Shell",
                "--object-path",
                "/org/gnome/Shell/Extensions/MineSharePointer",
                "--method",
                "org.gnome.Shell.Extensions.MineSharePointer.GetPointer",
            ],
        )
        .is_some_and(|text| pointer_active(&text));
        Ok(Status {
            linux: true,
            session,
            gnome_supported: gnome && major.is_some_and(|v| (45..=50).contains(&v)),
            wayland,
            input_active,
            uinput_available: Path::new("/dev/uinput").exists(),
            input_needs_repair: group_active && !input_active,
            input_configured,
            typeless: Path::new("/opt/Typeless/typeless").is_file(),
            broker_configured,
            broker_active,
            extension_configured,
            extension_active,
            extensions_allowed,
            helpers_available: Path::new("/usr/bin/pkexec").is_file()
                && [
                    "install-typeless-compat.sh",
                    "install-gnome-pointer.sh",
                    "enable-input.sh",
                    "start-typeless-after-mineshare.sh",
                ]
                .iter()
                .all(|name| helper(name).is_ok())
                && [
                    "60-mineshare.rules",
                    "gnome-pointer/metadata.json",
                    "gnome-pointer/extension.js",
                ]
                .iter()
                .all(|name| Path::new(ROOT).join(name).is_file()),
            pending_login: (input_configured && !input_active && !group_active)
                || (broker_configured && !broker_active)
                || (extension_configured && !extension_active),
        })
    }
    pub(super) fn configure(options: Options) -> anyhow::Result<Status> {
        static SETUP: parking_lot::Mutex<()> = parking_lot::const_mutex(());
        let _guard = SETUP
            .try_lock()
            .context("Another setup is already running")?;
        let before = detect()?;
        ensure!(
            before.helpers_available,
            "Install the current MineShare .deb package to use setup"
        );
        if options.pointer {
            ensure!(
                before.wayland && before.gnome_supported && before.extensions_allowed,
                "Pointer setup needs GNOME 45–50 Wayland with user extensions allowed"
            );
        }
        if options.broker {
            ensure!(
                before.typeless,
                "Typeless was not detected; keyboard broker is optional"
            );
        }
        if options.input
            && (!before.input_configured || !before.uinput_available || before.input_needs_repair)
        {
            let path = helper("enable-input.sh")?;
            // No usernames or executable paths accepted from the frontend.
            command(
                "/usr/bin/pkexec",
                &["/bin/bash", path.to_str().context("Invalid package path")?],
                Duration::from_secs(180),
            )?;
        }
        if options.broker && !before.broker_configured {
            let path = helper("install-typeless-compat.sh")?;
            command(
                "/bin/bash",
                &[path.to_str().unwrap(), "/usr/bin/mineshare-app"],
                Duration::from_secs(15),
            )?;
        }
        if options.pointer {
            let backup = config()?.join("MineShare/backups").join(format!(
                "gnome-setup-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            ));
            std::fs::create_dir_all(&backup)?;
            for key in ["enabled-extensions", "disabled-extensions"] {
                let value = command(
                    "gsettings",
                    &["get", "org.gnome.shell", key],
                    Duration::from_secs(3),
                )?;
                std::fs::write(backup.join(format!("{key}.txt")), value)?;
            }
            let path = helper("install-gnome-pointer.sh")?;
            command(
                "/bin/bash",
                &[path.to_str().unwrap()],
                Duration::from_secs(15),
            )?;
            // Use GLib's GVariant parser, not string concatenation/eval, to retain
            // every other extension and handle empty/typed lists safely.
            for (key, add) in [("enabled-extensions", true), ("disabled-extensions", false)] {
                let old = command(
                    "gsettings",
                    &["get", "org.gnome.shell", key],
                    Duration::from_secs(3),
                )?;
                let variant = gio::glib::Variant::parse(None, &old)?;
                let mut list = variant
                    .get::<Vec<String>>()
                    .context("Invalid GNOME extension list")?;
                if add && !list.iter().any(|v| v == UUID) {
                    list.push(UUID.to_owned());
                }
                if !add {
                    list.retain(|v| v != UUID);
                }
                use gio::glib::variant::ToVariant;
                let value = list.to_variant().print(true).to_string();
                command(
                    "gsettings",
                    &["set", "org.gnome.shell", key, &value],
                    Duration::from_secs(3),
                )?;
            }
        }
        detect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn group_match_is_exact() {
            assert!(has_group("tee input users"));
            assert!(!has_group("tee input-other"));
        }
        #[test]
        fn locked_or_malformed_pointer_is_not_ready() {
            assert!(pointer_active("(800, 350, 1920, 1080)"));
            assert!(!pointer_active("(0, 0, 0, 0)"));
            assert!(!pointer_active("error"));
        }
        #[test]
        fn helper_rejects_arbitrary_paths() {
            assert!(helper("../../tmp/evil.sh").is_err());
        }

        #[test]
        #[ignore = "disposable runner fixture only; never the real desktop"]
        fn fixture_configuration_preserves_data_and_is_idempotent() -> anyhow::Result<()> {
            ensure!(
                std::env::var("MINESHARE_SETUP_FIXTURE").as_deref() == Ok("1"),
                "Isolated fixture required"
            );
            let work =
                PathBuf::from(std::env::var_os("MINESHARE_SETUP_WORK").context("Fixture missing")?);
            // This test runs alone in a child test process with no app/runtime threads.
            unsafe {
                std::env::set_var("HOME", work.join("home"));
                std::env::set_var("XDG_CONFIG_HOME", work.join("home/.config"));
                std::env::set_var("XDG_DATA_HOME", work.join("home/.local/share"));
                std::env::set_var("XDG_CURRENT_DESKTOP", "ubuntu:GNOME");
                std::env::set_var("XDG_SESSION_TYPE", "wayland");
            }
            let config = config()?;
            std::fs::create_dir_all(config.join("MineShare"))?;
            for name in ["identity.json", "trusted_peers.json", "settings.json"] {
                std::fs::write(config.join("MineShare").join(name), "preserve-this-fixture")?;
            }
            for _ in 0..2 {
                let result = configure(Options {
                    input: false,
                    broker: true,
                    pointer: true,
                })?;
                ensure!(
                    result.broker_configured && result.extension_configured && result.pending_login,
                    "Expected staged, not active, setup"
                );
                ensure!(
                    !result.broker_active && !result.extension_active,
                    "Mock desktop must never claim native activation"
                );
                for name in ["identity.json", "trusted_peers.json", "settings.json"] {
                    ensure!(
                        std::fs::read_to_string(config.join("MineShare").join(name))?
                            == "preserve-this-fixture"
                    );
                }
                let enabled = std::fs::read_to_string(work.join("enabled-extensions"))?;
                ensure!(
                    enabled.contains("other@extension") && enabled.matches(UUID).count() == 1,
                    "Other extensions/duplicates changed"
                );
                ensure!(
                    std::fs::read_to_string(
                        work.join("home/.local/share/MineShare/typeless-login.desktop")
                    )?
                    .contains("--preserve-login-flags")
                );
            }
            Ok(())
        }
    }
}
