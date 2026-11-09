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
use super::super::types::{JobStatus, ProgressParser, VideoJob};
use super::{command_line, run_cmds_with_progress, was_user_cancelled};
use crate::engine::worker::PidRegistry;
use crate::engine::{dovi_tool, probe};

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

    /// Delete files a previous run left behind (it crashed before its cleanup ran),
    /// so the conversion never starts from or trips over a stale stream.
    pub fn remove_stale(&self) {
        drop(CleanupGuard::new(self.paths()));
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

/// The error text of a pre-step that did not succeed. A cancelled pre-step reads like
/// a cancelled encode; a failed one names the conversion.
fn failure_message(cancelled: bool, status: &str, stderr_tail: &str) -> String {
    if cancelled {
        format!("Encoding failed with status: {status}\n\nFFmpeg error:\n{stderr_tail}")
    } else {
        format!("Dolby Vision Conversion failed with status: {status}\n\n{stderr_tail}")
    }
}

/// Run the pre-step as its own phase (`JobStatus::ConvertingDolbyVision`) and leave
/// the job `Running` again on success. Stale files from a crashed run are removed
/// first. On failure or cancel returns the message for the job's error; the caller
/// ends the job.
pub(super) fn run<F>(
    job: &mut VideoJob,
    cmds: Vec<Command>,
    silent: bool,
    pid_registry: Option<&PidRegistry>,
    callback: &mut F,
) -> Result<(), String>
where
    F: FnMut(&VideoJob, &ProgressParser),
{
    ConversionFiles::for_output(&job.output_path).remove_stale();

    let lines = cmds.iter().map(command_line).collect::<Vec<_>>();
    let _ = write_debug_log(&format!(
        "\n=== Dolby Vision Conversion ===\n{}\n",
        lines.join("\n&& \\\n")
    ));
    job.status = JobStatus::ConvertingDolbyVision;
    job.progress_pct = 0.0;
    callback(job, &ProgressParser::new());

    // Not a two-pass encode: progress is not split into halves.
    let outcome = run_cmds_with_progress(job, cmds, silent, 1, pid_registry, callback);
    let message = match outcome {
        Ok((status, _, _, _)) if status.success() => None,
        Ok((status, _, stderr, _)) => {
            let lines: Vec<&str> = stderr.lines().collect();
            let tail = lines[lines.len().saturating_sub(10)..].join("\n");
            let cancelled = was_user_cancelled(&status, &stderr);
            if cancelled {
                let _ = write_debug_log("[cleanup] Dolby Vision Conversion: user cancelled\n");
            }
            Some(failure_message(cancelled, &status.to_string(), &tail))
        }
        Err(e) => Some(format!("Dolby Vision Conversion could not run: {e:#}")),
    };
    if let Some(message) = message {
        let _ = write_debug_log(&format!("✗ {message}\n"));
        return Err(message);
    }
    job.status = JobStatus::Running;
    job.progress_pct = 0.0;
    callback(job, &ProgressParser::new());
    Ok(())
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
    fn stale_files_from_a_crashed_run_are_removed_before_a_new_conversion() {
        let dir = tempfile::tempdir().unwrap();
        let files = touch_files(dir.path());
        files.remove_stale();
        assert!(!any_exists(&files));
    }

    #[test]
    fn a_cancelled_pre_step_reads_like_a_cancelled_encode() {
        let cancelled = failure_message(true, "exit status: 255", "Exiting normally");
        assert!(
            cancelled.starts_with("Encoding failed with status:"),
            "{cancelled}"
        );
        assert!(!cancelled.contains("was cancelled"), "{cancelled}");
        let failed = failure_message(false, "exit status: 1", "boom");
        assert!(
            failed.starts_with("Dolby Vision Conversion failed"),
            "{failed}"
        );
    }

    #[test]
    fn cleanup_tolerates_files_that_were_never_created() {
        let dir = tempfile::tempdir().unwrap();
        let files = ConversionFiles::for_output(&dir.path().join("out.mkv"));
        drop(CleanupGuard::new(files.paths()));
        assert!(!any_exists(&files));
    }
}
