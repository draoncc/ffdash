//! The external `dovi_tool` (github.com/quietvoid/dovi_tool), used for the Dolby Vision
//! Conversion of Profile 7 sources to Profile 8.1. FFmpeg can't convert Profile 7.
//!
//! Detection is "on `PATH` and `--version` succeeds", done once per session. The
//! argument building and version parsing are pure and unit-tested; the process
//! itself is never faked.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// Parse `dovi_tool --version` output (clap style: `dovi_tool 2.1.2`) into the
/// version number. `None` when the output isn't from `dovi_tool`.
pub fn parse_version(output: &str) -> Option<String> {
    let line = output.lines().find(|l| !l.trim().is_empty())?;
    let mut words = line.split_whitespace();
    if words.next()? != "dovi_tool" {
        return None;
    }
    let version = words.next()?;
    let mut parts = version.split('.');
    let numeric =
        |p: Option<&str>| p.is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    if numeric(parts.next()) && numeric(parts.next()) {
        Some(version.to_string())
    } else {
        None
    }
}

/// The `dovi_tool` command converting `input` (a raw HEVC stream) to Profile 8.1,
/// discarding the enhancement layer, into `output`.
///
/// `-m 2` converts the RPU to Profile 8.1 compatible; `--discard` drops the
/// enhancement layer; `-i`/`-o` are the `convert` subcommand's input and output.
pub fn convert_command(input: &Path, output: &Path) -> Command {
    let mut cmd = Command::new("dovi_tool");
    cmd.args(["-m", "2", "convert", "--discard", "-i"]);
    cmd.arg(input);
    cmd.arg("-o").arg(output);
    cmd
}

/// Whether a working `dovi_tool` is installed. Detected once per session.
pub fn available() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(detect)
}

fn detect() -> bool {
    Command::new("dovi_tool")
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| parse_version(&String::from_utf8_lossy(&o.stdout)))
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parses_the_version_from_clap_output() {
        assert_eq!(parse_version("dovi_tool 2.1.2\n").as_deref(), Some("2.1.2"));
        assert_eq!(parse_version("\ndovi_tool 2.3.0").as_deref(), Some("2.3.0"));
    }

    #[test]
    fn rejects_output_that_is_not_dovi_tool_version() {
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("ffmpeg version 7.1"), None);
        assert_eq!(parse_version("dovi_tool"), None);
        assert_eq!(parse_version("dovi_tool abc"), None);
    }

    #[test]
    fn convert_command_targets_profile_8_1_and_discards_the_enhancement_layer() {
        let cmd = convert_command(&PathBuf::from("in.hevc"), &PathBuf::from("out.hevc"));
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(cmd.get_program(), "dovi_tool");
        assert_eq!(
            args,
            [
                "-m",
                "2",
                "convert",
                "--discard",
                "-i",
                "in.hevc",
                "-o",
                "out.hevc"
            ]
        );
    }
}
