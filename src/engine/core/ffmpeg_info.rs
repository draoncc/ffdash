use crate::engine::dolby_vision::{DolbyVisionInfo, DolbyVisionProfile};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FfprobeStreamDisposition {
    #[serde(default)]
    attached_pic: u8,
}

#[derive(Debug, Deserialize)]
struct FfprobeStream {
    codec_name: Option<String>,
    codec_type: Option<String>,
    #[serde(default)]
    disposition: Option<FfprobeStreamDisposition>,
    #[serde(default)]
    side_data_list: Vec<FfprobeSideData>,
}

/// One entry of a stream's `side_data_list`. Only the Dolby Vision configuration
/// record (`side_data_type` "DOVI configuration record") carries these fields.
#[derive(Debug, Deserialize)]
struct FfprobeSideData {
    side_data_type: Option<String>,
    dv_profile: Option<u8>,
    dv_bl_signal_compatibility_id: Option<u8>,
    bl_present_flag: Option<u8>,
    el_present_flag: Option<u8>,
    rpu_present_flag: Option<u8>,
}

impl FfprobeOutput {
    /// The first video stream that is not an attached picture (e.g. cover art).
    fn main_video_stream(&self) -> Option<&FfprobeStream> {
        self.streams
            .iter()
            .find(|s| s.codec_type.as_deref() == Some("video") && !s.is_attached_pic())
    }
}

impl FfprobeStream {
    /// The Dolby Vision configuration record of this stream, if it has one.
    fn dolby_vision(&self) -> Option<DolbyVisionInfo> {
        let record = self
            .side_data_list
            .iter()
            .find(|d| d.side_data_type.as_deref() == Some("DOVI configuration record"))?;
        let compat_id = record.dv_bl_signal_compatibility_id.unwrap_or(0);
        Some(DolbyVisionInfo {
            profile: DolbyVisionProfile::from_record(record.dv_profile?, compat_id),
            bl_present: record.bl_present_flag.unwrap_or(0) != 0,
            el_present: record.el_present_flag.unwrap_or(0) != 0,
            rpu_present: record.rpu_present_flag.unwrap_or(0) != 0,
            bl_compat_id: compat_id,
        })
    }

    /// Returns true if this stream is an attached picture (e.g., album art / thumbnail)
    fn is_attached_pic(&self) -> bool {
        self.disposition
            .as_ref()
            .is_some_and(|d| d.attached_pic != 0)
    }
}

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    format: FfprobeFormat,
    #[serde(default)]
    streams: Vec<FfprobeStream>,
}

/// Input file info from ffprobe (duration and video codec)
#[derive(Debug, Clone)]
pub struct InputInfo {
    pub duration_s: Option<f64>,
    pub video_codec: Option<String>,
    /// Dolby Vision configuration of the main video stream, if it has one.
    pub dolby_vision: Option<DolbyVisionInfo>,
}

/// Check if ffmpeg is available and return its version
pub fn ffmpeg_version() -> Result<String> {
    // Lest we forget: skipping this probe once shipped "/dev/null" as a feature
    let output = Command::new("ffmpeg")
        .arg("-version")
        .output()
        .context("Failed to execute ffmpeg. Is ffmpeg installed and in PATH?")?;

    if !output.status.success() {
        anyhow::bail!("ffmpeg command failed with status: {}", output.status);
    }

    let version_output = String::from_utf8_lossy(&output.stdout);
    let first_line = version_output.lines().next().unwrap_or("Unknown version");

    Ok(first_line.to_string())
}

/// Check if ffmpeg has the libvmaf filter available
pub fn vmaf_filter_available() -> bool {
    let output = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-filters")
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout.contains("libvmaf")
        }
        _ => false,
    }
}

/// Check if ffprobe is available
pub fn ffprobe_version() -> Result<String> {
    let output = Command::new("ffprobe")
        .arg("-version")
        .output()
        .context("Failed to execute ffprobe. Is ffprobe installed and in PATH?")?;

    if !output.status.success() {
        anyhow::bail!("ffprobe command failed with status: {}", output.status);
    }

    let version_output = String::from_utf8_lossy(&output.stdout);
    let first_line = version_output.lines().next().unwrap_or("Unknown version");

    Ok(first_line.to_string())
}

/// Probe a video file to get its duration in seconds
pub fn probe_duration(path: &Path) -> Result<f64> {
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("quiet")
        .arg("-print_format")
        .arg("json")
        .arg("-show_format")
        .arg(path)
        .output()
        .context("Failed to execute ffprobe")?;

    if !output.status.success() {
        anyhow::bail!(
            "ffprobe failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let probe: FfprobeOutput =
        serde_json::from_str(&json_str).context("Failed to parse ffprobe JSON output")?;

    let duration_str = probe
        .format
        .duration
        .context("No duration found in ffprobe output")?;

    let duration = duration_str
        .parse::<f64>()
        .context("Failed to parse duration as float")?;

    Ok(duration)
}

/// Probe a video file to get duration and video codec in one call
pub fn probe_input_info(path: &Path) -> Result<InputInfo> {
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("quiet")
        .arg("-print_format")
        .arg("json")
        .arg("-show_format")
        .arg("-show_streams")
        .arg(path)
        .output()
        .context("Failed to execute ffprobe")?;

    if !output.status.success() {
        anyhow::bail!(
            "ffprobe failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let probe: FfprobeOutput =
        serde_json::from_str(&json_str).context("Failed to parse ffprobe JSON output")?;

    // Extract duration
    let duration_s = probe
        .format
        .duration
        .as_ref()
        .and_then(|s| s.parse::<f64>().ok());

    // Extract video codec from the first non-thumbnail video stream.
    // Files with embedded cover art (e.g., MJPEG or PNG thumbnails) may have the
    // thumbnail as stream 0, which would report the wrong codec and bypass skip logic.
    let main_video = probe.main_video_stream();

    Ok(InputInfo {
        duration_s,
        video_codec: main_video.and_then(|s| s.codec_name.clone()),
        dolby_vision: main_video.and_then(|s| s.dolby_vision()),
    })
}

/// Check whether two paths refer to the same file.
///
/// Handles the case where the output doesn't exist yet by canonicalizing the
/// parent directory and comparing with the canonical input path.
/// Falls back to raw path comparison if canonicalization fails entirely.
pub fn paths_are_same_file(a: &Path, b: &Path) -> bool {
    // Fast path: both exist and canonicalize succeeds
    if let (Ok(ca), Ok(cb)) = (a.canonicalize(), b.canonicalize()) {
        return ca == cb;
    }
    // Output may not exist yet — canonicalize parent + filename
    if let (Ok(ca), Some(b_parent), Some(b_name)) = (a.canonicalize(), b.parent(), b.file_name()) {
        if let Ok(cb_parent) = b_parent.canonicalize() {
            return ca == cb_parent.join(b_name);
        }
    }
    // Symmetric: also try the other direction
    if let (Ok(cb), Some(a_parent), Some(a_name)) = (b.canonicalize(), a.parent(), a.file_name()) {
        if let Ok(ca_parent) = a_parent.canonicalize() {
            return cb == ca_parent.join(a_name);
        }
    }
    // Last resort: raw comparison
    a == b
}

/// Parse ffprobe JSON to extract input info (for testing)
pub fn parse_ffprobe_input_info(json: &str) -> Result<InputInfo> {
    let probe: FfprobeOutput =
        serde_json::from_str(json).context("Failed to parse ffprobe JSON")?;

    let duration_s = probe
        .format
        .duration
        .as_ref()
        .and_then(|s| s.parse::<f64>().ok());

    let main_video = probe.main_video_stream();

    Ok(InputInfo {
        duration_s,
        video_codec: main_video.and_then(|s| s.codec_name.clone()),
        dolby_vision: main_video.and_then(|s| s.dolby_vision()),
    })
}

/// Parse duration from ffprobe JSON string (for testing)
pub fn parse_ffprobe_duration(json: &str) -> Result<f64> {
    let probe: FfprobeOutput =
        serde_json::from_str(json).context("Failed to parse ffprobe JSON")?;

    let duration_str = probe.format.duration.context("No duration found in JSON")?;

    duration_str
        .parse::<f64>()
        .context("Failed to parse duration as float")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::dolby_vision::DolbyVisionProfile;

    fn fixture(side_data: &str) -> String {
        format!(
            r#"{{"format":{{"duration":"10.0"}},"streams":[{{"codec_type":"video","codec_name":"hevc","side_data_list":[{side_data}]}}]}}"#
        )
    }

    fn dovi(profile: u8, compat: u8, el: u8) -> String {
        format!(
            r#"{{"side_data_type":"DOVI configuration record","dv_version_major":1,"dv_version_minor":0,"dv_profile":{profile},"dv_level":6,"rpu_present_flag":1,"el_present_flag":{el},"bl_present_flag":1,"dv_bl_signal_compatibility_id":{compat}}}"#
        )
    }

    fn profile_of(json: String) -> DolbyVisionProfile {
        parse_ffprobe_input_info(&fixture(&json))
            .unwrap()
            .dolby_vision
            .unwrap()
            .profile
    }

    #[test]
    fn profile_8_with_hdr10_compat_reports_layers() {
        let info = parse_ffprobe_input_info(&fixture(&dovi(8, 1, 0))).unwrap();
        let dv = info.dolby_vision.expect("dolby vision detected");
        assert_eq!(dv.profile, DolbyVisionProfile::P8_1);
        assert!(dv.bl_present && dv.rpu_present && !dv.el_present);
        assert_eq!(dv.bl_compat_id, 1);
    }

    #[test]
    fn profiles_are_recognised() {
        assert_eq!(profile_of(dovi(5, 0, 0)), DolbyVisionProfile::P5);
        assert_eq!(profile_of(dovi(7, 6, 1)), DolbyVisionProfile::P7);
        assert_eq!(profile_of(dovi(8, 4, 0)), DolbyVisionProfile::P8_4);
        assert_eq!(
            profile_of(dovi(8, 2, 0)),
            DolbyVisionProfile::Other {
                profile: 8,
                compat_id: 2
            }
        );
    }

    #[test]
    fn source_without_dolby_vision_has_none() {
        let hdr10 = r#"{"side_data_type":"Mastering display metadata"}"#;
        let info = parse_ffprobe_input_info(&fixture(hdr10)).unwrap();
        assert!(info.dolby_vision.is_none());
        let plain =
            r#"{"format":{"duration":"1"},"streams":[{"codec_type":"video","codec_name":"h264"}]}"#;
        assert!(
            parse_ffprobe_input_info(plain)
                .unwrap()
                .dolby_vision
                .is_none()
        );
    }

    #[test]
    fn cover_art_stream_is_not_read_for_dolby_vision() {
        let json = format!(
            r#"{{"format":{{}},"streams":[{{"codec_type":"video","codec_name":"mjpeg","disposition":{{"attached_pic":1}},"side_data_list":[{}]}},{{"codec_type":"video","codec_name":"hevc"}}]}}"#,
            dovi(8, 1, 0)
        );
        assert!(
            parse_ffprobe_input_info(&json)
                .unwrap()
                .dolby_vision
                .is_none()
        );
    }
}
