//! A Dolby Vision source is skipped with a visible reason from the encode path,
//! before any FFmpeg process runs (ADR 0003). Needs no FFmpeg and no real file.

use ffdash::engine::core::{JobStatus, Profile, VideoJob, encode_job_with_callback_and_profile};
use ffdash::engine::dolby_vision::{DolbyVisionInfo, DolbyVisionProfile};
use std::path::PathBuf;

fn dolby_vision_job(profile: DolbyVisionProfile) -> VideoJob {
    let mut job = VideoJob::new(
        PathBuf::from("/nonexistent/dv-source.mkv"),
        PathBuf::from("/nonexistent/out/dv-source.mkv"),
        "hevc-x265".to_string(),
    );
    job.dolby_vision = Some(DolbyVisionInfo {
        profile,
        bl_present: true,
        el_present: false,
        rpu_present: true,
        bl_compat_id: 1,
    });
    job
}

#[test]
fn dolby_vision_job_ends_skipped_with_reason_and_never_runs_ffmpeg() {
    let profile = Profile::get("hevc-x265");
    let mut job = dolby_vision_job(DolbyVisionProfile::P8_1);
    let mut callbacks = 0;

    let result =
        encode_job_with_callback_and_profile(&mut job, true, None, Some(&profile), None, |_, _| {
            callbacks += 1
        });

    assert!(result.is_ok());
    assert_eq!(job.status, JobStatus::Skipped);
    let reason = job.last_error.as_deref().unwrap();
    assert!(reason.contains("Dolby Vision Profile 8.1"), "{reason}");
    assert_eq!(job.attempts, 0, "the job never started");
    assert!(job.encoder.is_none(), "no Encoder was chosen");
    assert_eq!(
        job.output_path,
        PathBuf::from("/nonexistent/out/dv-source.mkv"),
        "no temp output was set up"
    );
    assert!(callbacks >= 1, "the UI is told about the skip");
}
