//! Dolby Vision Conversion of a Profile 7 source to Profile 8.1, expressed as extra
//! commands that run before the final encode (ADR 0003).
//!
//! 1. ffmpeg extracts the source's HEVC video stream as raw Annex B.
//! 2. `dovi_tool` converts it to Profile 8.1, discarding the enhancement layer.
//! 3. The final encode (any Encoder that can carry Dolby Vision) reads the converted
//!    stream as a second input and takes audio, subtitles, chapters and metadata from
//!    the original source (input 0).
//!
//! The pre-step is Encoder-independent: a builder only needs to call
//! [`apply_converted_input`] and map [`CONVERTED_VIDEO_MAP`] as its video stream.
//! Temporary files live next to the output ([`ConversionFiles`]).

use std::path::{Path, PathBuf};
use std::process::Command;

use super::super::log::write_debug_log;
use super::super::types::VideoJob;
use crate::engine::{dovi_tool, probe};

/// How many commands the pre-step adds before the encode.
pub const PRE_STEP_COMMANDS: usize = 2;

/// The converted stream is input 1 of the final encode.
pub const CONVERTED_VIDEO_MAP: &str = "1:v:0";

/// The temporary files of a Dolby Vision Conversion, next to the output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionFiles {
    /// The source's HEVC stream, extracted for `dovi_tool`.
    pub extracted: PathBuf,
    /// The Profile 8.1 stream the encode reads.
    pub converted: PathBuf,
}

impl ConversionFiles {
    /// The files for an output path: `<output>.dv-source.hevc`, `<output>.dv-8-1.hevc`.
    pub fn for_output(output: &Path) -> Self {
        let with_suffix = |suffix: &str| {
            let mut path = output.as_os_str().to_owned();
            path.push(suffix);
            PathBuf::from(path)
        };
        Self {
            extracted: with_suffix(".dv-source.hevc"),
            converted: with_suffix(".dv-8-1.hevc"),
        }
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        vec![self.extracted.clone(), self.converted.clone()]
    }
}

/// Deletes files when dropped: held for the whole job so the temporary conversion
/// files go on success, on failure, on cancel and on any early return or panic.
pub struct CleanupGuard {
    paths: Vec<PathBuf>,
}

impl CleanupGuard {
    pub fn new(paths: Vec<PathBuf>) -> Self {
        Self { paths }
    }
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        for path in &self.paths {
            if path.exists() {
                match std::fs::remove_file(path) {
                    Ok(()) => {
                        let _ = write_debug_log(&format!(
                            "[cleanup] Removed Dolby Vision Conversion file: {}\n",
                            path.display()
                        ));
                    }
                    Err(e) => {
                        let _ = write_debug_log(&format!(
                            "[cleanup] Failed to remove {}: {}\n",
                            path.display(),
                            e
                        ));
                    }
                }
            }
        }
    }
}

/// The commands before the encode: extract the HEVC stream, then convert it.
pub(super) fn pre_step_cmds(job: &VideoJob) -> Vec<Command> {
    let files = ConversionFiles::for_output(&job.output_path);

    let mut extract = Command::new("ffmpeg");
    extract.arg("-i").arg(&job.input_path);
    extract.arg("-progress").arg("-").arg("-nostats");
    extract.args(["-map", "0:v:0", "-c:v", "copy"]);
    extract.args(["-bsf:v", "hevc_mp4toannexb", "-f", "hevc", "-y"]);
    extract.arg(&files.extracted);

    let convert = dovi_tool::convert_command(&files.extracted, &files.converted);
    vec![extract, convert]
}

/// Add the converted stream as the next input. A raw HEVC stream carries no frame
/// rate, so the source's is passed when it can be probed.
pub(super) fn apply_converted_input(cmd: &mut Command, job: &VideoJob) {
    let files = ConversionFiles::for_output(&job.output_path);
    if let Ok(info) = probe::probe_input_info(&job.input_path) {
        cmd.arg("-framerate").arg(info.fps.to_string());
    }
    cmd.arg("-i").arg(&files.converted);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch_files(dir: &Path) -> ConversionFiles {
        let files = ConversionFiles::for_output(&dir.join(".ffdash_tmp_movie.mkv"));
        for p in files.paths() {
            std::fs::write(p, b"x").unwrap();
        }
        files
    }

    fn any_exists(files: &ConversionFiles) -> bool {
        files.paths().iter().any(|p| p.exists())
    }

    #[test]
    fn temp_files_sit_next_to_the_output() {
        let files = ConversionFiles::for_output(Path::new("/out/.ffdash_tmp_movie.mkv"));
        assert_eq!(
            files.extracted,
            PathBuf::from("/out/.ffdash_tmp_movie.mkv.dv-source.hevc")
        );
        assert_eq!(
            files.converted,
            PathBuf::from("/out/.ffdash_tmp_movie.mkv.dv-8-1.hevc")
        );
    }

    #[test]
    fn temp_files_are_removed_after_success() {
        let dir = tempfile::tempdir().unwrap();
        let files = touch_files(dir.path());
        let run = || -> Result<(), String> {
            let _cleanup = CleanupGuard::new(files.paths());
            Ok(())
        };
        assert!(run().is_ok());
        assert!(!any_exists(&files));
    }

    #[test]
    fn temp_files_are_removed_after_failure_or_cancel() {
        // A failed or cancelled pre-step leaves its caller by returning early.
        let dir = tempfile::tempdir().unwrap();
        let files = touch_files(dir.path());
        let run = || -> Result<(), String> {
            let _cleanup = CleanupGuard::new(files.paths());
            Err("dovi_tool failed".to_string())?;
            Ok(())
        };
        assert!(run().is_err());
        assert!(!any_exists(&files));
    }

    #[test]
    fn temp_files_are_removed_even_if_the_job_panics() {
        let dir = tempfile::tempdir().unwrap();
        let files = touch_files(dir.path());
        let result = std::panic::catch_unwind(|| {
            let _cleanup = CleanupGuard::new(files.paths());
            panic!("boom");
        });
        assert!(result.is_err());
        assert!(!any_exists(&files));
    }

    #[test]
    fn cleanup_tolerates_files_that_were_never_created() {
        let dir = tempfile::tempdir().unwrap();
        let files = ConversionFiles::for_output(&dir.path().join("out.mkv"));
        drop(CleanupGuard::new(files.paths()));
        assert!(!any_exists(&files));
    }
}
