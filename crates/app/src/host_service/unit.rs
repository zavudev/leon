//! The text of the service definitions: a systemd user unit and a launchd
//! agent.
//!
//! Both run the same program that is installing them, as `leon host
//! --data-dir <dir>` (plus the relay and the name when they were given), so a
//! service and a foreground `leon host` share one identity, one device list
//! and one status file. They restart the host when it fails, with a longer
//! wait each time where the system manager can do that (systemd 254 or newer
//! through `RestartSteps`; launchd has only a fixed `ThrottleInterval`), and
//! never ask for more than the user's own rights.
//!
//! Everything here is a pure function of its inputs: no file is read and no
//! command is run, so the golden texts in the tests are the whole contract.

use std::path::{Path, PathBuf};

/// The systemd user unit's file name.
pub const UNIT_NAME: &str = "leon-host.service";

/// Seconds before systemd restarts a host that failed, the first time.
const RESTART_FIRST_SECONDS: u32 = 5;
/// How many steps systemd takes from the first wait to the longest.
const RESTART_STEPS: u32 = 6;
/// The longest systemd waits before a restart, in seconds.
const RESTART_LONGEST_SECONDS: u32 = 300;
/// Seconds launchd leaves between two starts of the host.
const LAUNCHD_THROTTLE_SECONDS: u32 = 30;
/// Seconds a stopping host is given to close its sessions before it is cut.
const STOP_TIMEOUT_SECONDS: u32 = 15;

/// What the service runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    /// The program: the file that is installing the service.
    pub exe: PathBuf,
    /// The data directory the host keeps its identity and devices in.
    pub data_dir: PathBuf,
    /// The relay, when the person chose one; the host's default otherwise.
    pub relay: Option<String>,
    /// The name devices see, when the person chose one.
    pub name: Option<String>,
    /// The `PATH` of the installing session: a service manager starts the host
    /// with a short one, and the commands and terminals it serves want the
    /// person's own.
    pub path_env: Option<String>,
}

impl Service {
    /// Checks that the service can be written down as text: an absolute
    /// program, and no control character (a newline would start a new line of
    /// a unit) in anything that is written.
    pub fn validate(&self) -> Result<(), String> {
        if !self.exe.is_absolute() {
            return Err(format!(
                "The program {} is not an absolute path.",
                self.exe.display()
            ));
        }
        let texts = [
            ("the program", Some(self.exe.to_string_lossy().into_owned())),
            (
                "the data directory",
                Some(self.data_dir.to_string_lossy().into_owned()),
            ),
            ("the relay", self.relay.clone()),
            ("the name", self.name.clone()),
            ("PATH", self.path_env.clone()),
        ];
        for (what, text) in texts {
            if text.is_some_and(|text| text.chars().any(char::is_control)) {
                return Err(format!("{what} contains a control character."));
            }
        }
        Ok(())
    }

    /// The arguments the program is started with.
    pub fn arguments(&self) -> Vec<String> {
        let mut arguments = vec![
            "host".to_owned(),
            "--data-dir".to_owned(),
            self.data_dir.to_string_lossy().into_owned(),
        ];
        if let Some(relay) = &self.relay {
            arguments.extend(["--relay".to_owned(), relay.clone()]);
        }
        if let Some(name) = &self.name {
            arguments.extend(["--name".to_owned(), name.clone()]);
        }
        arguments
    }

    fn command_words(&self) -> Vec<String> {
        let mut words = vec![self.exe.to_string_lossy().into_owned()];
        words.extend(self.arguments());
        words
    }
}

/// The program as the running process reports it, without the marker Linux
/// appends when the file was replaced while it ran (` (deleted)`): after an
/// update that is the path the new file is at.
pub fn normalise_exe(exe: &Path) -> PathBuf {
    let text = exe.to_string_lossy();
    match text.strip_suffix(" (deleted)") {
        Some(path) => PathBuf::from(path),
        None => exe.to_path_buf(),
    }
}

/// Why a program at this path cannot be the service's: the place is one that
/// is gone by the next start (an AppImage's mount, a temporary folder, a
/// translocated application, a disk image).
pub fn unstable_install(exe: &Path) -> Option<&'static str> {
    let text = exe.to_string_lossy();
    if text.contains("/.mount_") {
        return Some("it runs from an AppImage, which is mounted only while it runs");
    }
    if text.contains("/AppTranslocation/") {
        return Some(
            "macOS runs it from a temporary copy; move Leon to the Applications folder first",
        );
    }
    if text.starts_with("/Volumes/") {
        return Some("it runs from a disk image; drag Leon to the Applications folder first");
    }
    if exe.starts_with("/tmp") || exe.starts_with("/var/tmp") || exe.starts_with("/private/tmp") {
        return Some("it is in a temporary folder");
    }
    None
}

/// Why an update will probably not reach a program at this path: a package
/// manager that gives each version its own folder (Homebrew's Cellar, Nix's
/// store) leaves the service pointing at the old version, which is removed
/// sooner or later. The path Linux reports for a running program is the one
/// its symlinks resolve to, so a link such as `/opt/homebrew/bin/leon` is
/// not what gets written.
pub fn versioned_install(exe: &Path) -> Option<&'static str> {
    let text = exe.to_string_lossy();
    if text.contains("/Cellar/") || text.contains("/nix/store/") {
        return Some(
            "it is in a folder that holds one version only (Homebrew, Nix); an update puts the new version elsewhere",
        );
    }
    None
}

/// Whether the program is a development build, which the next build replaces.
pub fn is_development_build(exe: &Path) -> bool {
    exe.components().any(|part| part.as_os_str() == "target")
}

/// A word of a systemd command line or an `Environment=` value: in double
/// quotes, with the backslash and the quote escaped, `%` doubled (a specifier)
/// and, in a command line, `$` doubled (a variable).
fn systemd_word(word: &str, command_line: bool) -> String {
    let mut out = String::from("\"");
    for c in word.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            '$' if command_line => out.push_str("$$"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The systemd user unit.
pub fn systemd_unit(service: &Service) -> String {
    let command = service
        .command_words()
        .iter()
        .map(|word| systemd_word(word, true))
        .collect::<Vec<_>>()
        .join(" ");
    let environment = service
        .path_env
        .as_ref()
        .map(|path| {
            format!(
                "Environment={}\n",
                systemd_word(&format!("PATH={path}"), false)
            )
        })
        .unwrap_or_default();
    format!(
        "\
# Written by `leon host service install`; `leon host service uninstall` removes it.
[Unit]
Description=Leon host: shares this computer with the devices you paired
StartLimitIntervalSec=0

[Service]
Type=simple
ExecStart={command}
{environment}Restart=on-failure
RestartSec={RESTART_FIRST_SECONDS}
RestartSteps={RESTART_STEPS}
RestartMaxDelaySec={RESTART_LONGEST_SECONDS}
TimeoutStopSec={STOP_TIMEOUT_SECONDS}

[Install]
WantedBy=default.target
"
    )
}

/// The launchd label of the agent, which is also its file name without
/// `.plist`.
pub fn launchd_label(app_id: &str) -> String {
    format!("{app_id}.host")
}

fn xml(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// The launchd agent's property list. Its output goes to `log`.
pub fn launchd_plist(service: &Service, label: &str, log: &Path) -> String {
    let words: String = service
        .command_words()
        .iter()
        .map(|word| format!("        <string>{}</string>\n", xml(word)))
        .collect();
    let environment = service
        .path_env
        .as_ref()
        .map(|path| {
            format!(
                "    <key>EnvironmentVariables</key>\n    <dict>\n        <key>PATH</key>\n        <string>{}</string>\n    </dict>\n",
                xml(path)
            )
        })
        .unwrap_or_default();
    let log = xml(&log.to_string_lossy());
    format!(
        "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<!-- Written by `leon host service install`; `leon host service uninstall` removes it. -->
<plist version=\"1.0\">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
{words}    </array>
{environment}    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ThrottleInterval</key>
    <integer>{LAUNCHD_THROTTLE_SECONDS}</integer>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> Service {
        Service {
            exe: PathBuf::from("/home/ana/.local/bin/leon"),
            data_dir: PathBuf::from("/home/ana/.local/share/leon"),
            relay: None,
            name: None,
            path_env: None,
        }
    }

    #[test]
    fn the_unit_runs_the_installing_program_and_restarts_it_on_failure() {
        let expected = "\
# Written by `leon host service install`; `leon host service uninstall` removes it.
[Unit]
Description=Leon host: shares this computer with the devices you paired
StartLimitIntervalSec=0

[Service]
Type=simple
ExecStart=\"/home/ana/.local/bin/leon\" \"host\" \"--data-dir\" \"/home/ana/.local/share/leon\"
Restart=on-failure
RestartSec=5
RestartSteps=6
RestartMaxDelaySec=300
TimeoutStopSec=15

[Install]
WantedBy=default.target
";
        assert_eq!(systemd_unit(&service()), expected);
    }

    #[test]
    fn the_unit_carries_the_relay_the_name_and_the_path_that_were_given() {
        let unit = systemd_unit(&Service {
            relay: Some("wss://relay.example".into()),
            name: Some("Ana's box".into()),
            path_env: Some("/home/ana/.local/bin:/usr/bin".into()),
            ..service()
        });
        assert!(unit.contains(
            "\"--data-dir\" \"/home/ana/.local/share/leon\" \"--relay\" \"wss://relay.example\" \"--name\" \"Ana's box\"\n"
        ));
        assert!(unit.contains("Environment=\"PATH=/home/ana/.local/bin:/usr/bin\"\n"));
    }

    #[test]
    fn a_word_with_spaces_quotes_percent_or_dollar_stays_one_word() {
        assert_eq!(systemd_word("a b", true), "\"a b\"");
        assert_eq!(
            systemd_word("say \"hi\" \\ 50%", true),
            "\"say \\\"hi\\\" \\\\ 50%%\""
        );
        assert_eq!(systemd_word("$HOME", true), "\"$$HOME\"");
        assert_eq!(systemd_word("$HOME", false), "\"$HOME\"");
    }

    #[test]
    fn a_service_that_cannot_be_written_down_is_refused() {
        assert!(service().validate().is_ok());
        let relative = Service {
            exe: PathBuf::from("leon"),
            ..service()
        };
        assert!(relative.validate().is_err());
        let newline = Service {
            name: Some("a\nExecStart=/bin/evil".into()),
            ..service()
        };
        assert!(newline.validate().unwrap_err().contains("the name"));
    }

    #[test]
    fn the_plist_names_the_label_the_arguments_and_the_restart_rules() {
        let plist = launchd_plist(
            &Service {
                exe: PathBuf::from("/Applications/Leon.app/Contents/MacOS/Leon"),
                data_dir: PathBuf::from("/Users/ana/Library/Application Support/leon"),
                relay: None,
                name: Some("A&B <box>".into()),
                path_env: Some("/opt/homebrew/bin:/usr/bin".into()),
            },
            "dev.zavu.leon.host",
            Path::new("/Users/ana/Library/Logs/Leon/host.log"),
        );
        let expected = "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<!-- Written by `leon host service install`; `leon host service uninstall` removes it. -->
<plist version=\"1.0\">
<dict>
    <key>Label</key>
    <string>dev.zavu.leon.host</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Applications/Leon.app/Contents/MacOS/Leon</string>
        <string>host</string>
        <string>--data-dir</string>
        <string>/Users/ana/Library/Application Support/leon</string>
        <string>--name</string>
        <string>A&amp;B &lt;box&gt;</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/opt/homebrew/bin:/usr/bin</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ThrottleInterval</key>
    <integer>30</integer>
    <key>StandardOutPath</key>
    <string>/Users/ana/Library/Logs/Leon/host.log</string>
    <key>StandardErrorPath</key>
    <string>/Users/ana/Library/Logs/Leon/host.log</string>
</dict>
</plist>
";
        assert_eq!(plist, expected);
    }

    #[test]
    fn the_label_is_the_application_id_and_host() {
        assert_eq!(launchd_label("dev.zavu.leon"), "dev.zavu.leon.host");
    }

    #[test]
    fn a_replaced_program_is_found_at_its_path_again() {
        assert_eq!(
            normalise_exe(Path::new("/home/ana/.local/bin/leon (deleted)")),
            Path::new("/home/ana/.local/bin/leon")
        );
        assert_eq!(
            normalise_exe(Path::new("/usr/bin/leon")),
            Path::new("/usr/bin/leon")
        );
    }

    #[test]
    fn a_program_that_will_not_be_there_tomorrow_is_not_installed() {
        for gone in [
            "/tmp/.mount_LeonAbc/usr/bin/leon",
            "/tmp/leon",
            "/var/tmp/leon",
            "/private/var/folders/x/AppTranslocation/ABC/d/Leon.app/Contents/MacOS/Leon",
            "/Volumes/Leon/Leon.app/Contents/MacOS/Leon",
        ] {
            assert!(unstable_install(Path::new(gone)).is_some(), "{gone}");
        }
        for stays in [
            "/usr/bin/leon",
            "/home/ana/.local/bin/leon",
            "/Applications/Leon.app/Contents/MacOS/Leon",
        ] {
            assert!(unstable_install(Path::new(stays)).is_none(), "{stays}");
        }
    }

    #[test]
    fn a_program_in_a_per_version_folder_is_flagged() {
        for versioned in [
            "/opt/homebrew/Cellar/leon/0.6.0/bin/leon",
            "/home/linuxbrew/.linuxbrew/Cellar/leon/0.6.0/bin/leon",
            "/nix/store/abc123-leon-0.6.0/bin/leon",
        ] {
            assert!(
                versioned_install(Path::new(versioned)).is_some(),
                "{versioned}"
            );
        }
        assert!(versioned_install(Path::new("/usr/bin/leon")).is_none());
        assert!(versioned_install(Path::new("/opt/homebrew/bin/leon")).is_none());
    }

    #[test]
    fn a_build_in_target_is_a_development_build() {
        assert!(is_development_build(Path::new(
            "/src/leon/target/debug/leon"
        )));
        assert!(!is_development_build(Path::new("/usr/bin/leon")));
    }
}
