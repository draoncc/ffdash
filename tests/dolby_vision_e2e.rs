//! Opt-in end-to-end Dolby Vision check (ADR 0003).
//!
//! Encodes a real Dolby Vision sample with ffdash and asks ffprobe whether the output
//! still reports the expected Dolby Vision Profile. Each case runs only when its
//! environment variable names a sample file; without it the case is skipped, so CI
//! (which has no sample) is unaffected. See tests/README.md.
//!
//! To add a case, append one row to `CASES` (see the notes in tests/README.md).

use ffdash::engine::core::{
    JobStatus, Profile, build_job_from_path, encode_job_with_callback_and_profile,
};
use std::path::{Path, PathBuf};
use std::process::Command;

/// One end-to-end case: a sample from `env_var`, encoded with `profile` into
/// `container`, whose output must report Dolby Vision Profile `expected_profile`.
struct Case {
    env_var: &'static str,
    profile: &'static str,
    container: &'static str,
    /// As written by `reported_profile`: "5", "7", "8.1", "8.4", "10", ...
    expected_profile: &'static str,
}

const CASES: &[Case] = &[
    Case {
        env_var: "FFDASH_DV_SAMPLE_HEVC_MKV_P81",
        profile: "hevc-x265",
        container: "mkv",
        expected_profile: "8.1",
    },
    Case {
        env_var: "FFDASH_DV_SAMPLE_HEVC_MP4_P81",
        profile: "hevc-x265",
        container: "mp4",
        expected_profile: "8.1",
    },
    Case {
        env_var: "FFDASH_DV_SAMPLE_AV1_MKV_P81",
        profile: "av1-svt",
        container: "mkv",
        expected_profile: "10",
    },
];

/// The sample path a case's environment variable names, if it names one.
fn sample_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The Dolby Vision Profile ffprobe's JSON (`-show_streams`) reports for the first video
/// stream, as "5", "8.1", ...; `None` when the stream has no Dolby Vision record.
fn reported_profile(ffprobe_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(ffprobe_json).ok()?;
    let stream = v["streams"]
        .as_array()?
        .iter()
        .find(|s| s["codec_type"] == "video")?;
    let record = stream["side_data_list"]
        .as_array()?
        .iter()
        .find(|d| d["side_data_type"] == "DOVI configuration record")?;
    let profile = record["dv_profile"].as_u64()?;
    if profile == 8 {
        let compat = record["dv_bl_signal_compatibility_id"].as_u64()?;
        Some(format!("8.{compat}"))
    } else {
        Some(profile.to_string())
    }
}

fn ffprobe_profile(path: &Path) -> Option<String> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_streams", "-of", "json"])
        .arg(path)
        .output()
        .expect("run ffprobe");
    assert!(out.status.success(), "ffprobe failed on {}", path.display());
    reported_profile(&String::from_utf8_lossy(&out.stdout))
}

fn run_case(case: &Case) {
    let Some(sample) = sample_path(case.env_var) else {
        eprintln!("skipping: {} is not set", case.env_var);
        return;
    };
    assert!(
        sample.is_file(),
        "{}={} is not a file",
        case.env_var,
        sample.display()
    );

    let out_dir = tempfile::tempdir().expect("temp dir");
    let mut job = build_job_from_path(
        sample.clone(),
        case.profile,
        false,
        Some(out_dir.path().to_str().expect("utf-8 temp path")),
        None,
        Some(case.container),
        false,
    );
    assert_ne!(job.status, JobStatus::Skipped, "{:?}", job.last_error);
    assert!(
        job.dolby_vision.is_some(),
        "{} does not look like a Dolby Vision file to ffdash",
        sample.display()
    );

    let profile = Profile::get(case.profile);
    encode_job_with_callback_and_profile(&mut job, true, None, Some(&profile), None, |_, _| {})
        .unwrap_or_else(|e| panic!("encode failed: {e:#}"));
    assert_eq!(job.status, JobStatus::Done, "{:?}", job.last_error);

    let reported = ffprobe_profile(&job.output_path);
    assert_eq!(
        reported.as_deref(),
        Some(case.expected_profile),
        "output {} should report Dolby Vision Profile {}",
        job.output_path.display(),
        case.expected_profile
    );
}

#[test]
fn dolby_vision_is_preserved_for_each_configured_sample() {
    for case in CASES {
        run_case(case);
    }
}

#[test]
fn reported_profile_reads_the_dolby_vision_record() {
    let json = r#"{"streams":[
        {"codec_type":"audio"},
        {"codec_type":"video","side_data_list":[
            {"side_data_type":"Mastering display metadata"},
            {"side_data_type":"DOVI configuration record","dv_profile":8,"dv_bl_signal_compatibility_id":1}]}]}"#;
    assert_eq!(reported_profile(json).as_deref(), Some("8.1"));

    let p5 = r#"{"streams":[{"codec_type":"video","side_data_list":[
        {"side_data_type":"DOVI configuration record","dv_profile":5,"dv_bl_signal_compatibility_id":0}]}]}"#;
    assert_eq!(reported_profile(p5).as_deref(), Some("5"));
}

#[test]
fn reported_profile_is_none_without_a_dolby_vision_record() {
    let plain = r#"{"streams":[{"codec_type":"video","side_data_list":[
        {"side_data_type":"Mastering display metadata"}]}]}"#;
    assert_eq!(reported_profile(plain), None);
    assert_eq!(
        reported_profile(r#"{"streams":[{"codec_type":"video"}]}"#),
        None
    );
    assert_eq!(reported_profile("not json"), None);
}

#[test]
fn a_case_without_its_environment_variable_has_no_sample() {
    assert_eq!(sample_path("FFDASH_DV_SAMPLE_DOES_NOT_EXIST"), None);
}

#[test]
fn every_case_uses_a_distinct_environment_variable() {
    let mut vars: Vec<_> = CASES.iter().map(|c| c.env_var).collect();
    vars.sort_unstable();
    vars.dedup();
    assert_eq!(vars.len(), CASES.len());
}
