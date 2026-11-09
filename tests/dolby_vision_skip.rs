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
    let mut job = dolby_vision_job(DolbyVisionProfile::P7);
    let mut callbacks = 0;

    let result =
        encode_job_with_callback_and_profile(&mut job, true, None, Some(&profile), None, |_, _| {
            callbacks += 1
        });

    assert!(result.is_ok());
    assert_eq!(job.status, JobStatus::Skipped);
    let reason = job.last_error.as_deref().unwrap();
    assert!(reason.contains("Dolby Vision Profile 7"), "{reason}");
    assert_eq!(job.attempts, 0, "the job never started");
    assert!(job.encoder.is_none(), "no Encoder was chosen");
    assert_eq!(
        job.output_path,
        PathBuf::from("/nonexistent/out/dv-source.mkv"),
        "no temp output was set up"
    );
    assert!(callbacks >= 1, "the UI is told about the skip");
}

#[test]
fn worker_reports_a_skipped_dolby_vision_job_with_its_reason_not_as_completed() {
    use ffdash::engine::worker::{WorkerMessage, WorkerPool};
    use std::time::Duration;

    let pool = WorkerPool::new(1);
    let job = dolby_vision_job(DolbyVisionProfile::P7);
    let job_id = job.id;
    pool.spawn_worker_with_profile(0, job, None, Some(Profile::get("hevc-x265")))
        .unwrap();

    let mut skipped_reason = None;
    while let Ok(msg) = pool.receiver().recv_timeout(Duration::from_secs(30)) {
        match msg {
            WorkerMessage::JobSkipped { job_id: id, reason } => {
                assert_eq!(id, job_id);
                skipped_reason = Some(reason);
            }
            WorkerMessage::JobCompleted { .. } => panic!("a skipped job must not complete"),
            WorkerMessage::WorkerIdle { .. } => break,
            _ => {}
        }
    }
    let reason = skipped_reason.expect("JobSkipped was sent");
    assert!(reason.contains("Dolby Vision Profile 7"), "{reason}");
}
