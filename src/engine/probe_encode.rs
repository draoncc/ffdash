//! Probe Encodes: a few-frame test encode whose success establishes Encoder
//! Availability (ADR 0001).
//!
//! Building the probe command and reading FFmpeg's encoder listing are pure and
//! unit-tested. Actually running FFmpeg is not: it is exercised on the real
//! machine, never against a fake FFmpeg.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::hardware::{self, VideoEncoder};
use super::selection::{CodecKind, EncoderAvailability};

/// Constant-quality value used by every probe (mid-range for all Encoders).
const PROBE_QUALITY: &str = "30";
/// A probe that has not finished by now is a hung driver: treat as unavailable.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// The hardware Encoders worth probing: every hardware Encoder of every Codec.
///
/// A new Codec only needs its priority list in [`CodecKind::hardware_priority`]
/// and a [`probe_args`] arm; nothing else here changes.
pub fn hardware_candidates() -> Vec<VideoEncoder> {
    let mut out = Vec::new();
    for codec in CodecKind::ALL {
        for encoder in codec.hardware_priority() {
            if !out.contains(encoder) {
                out.push(*encoder);
            }
        }
    }
    out
}

/// Whether `ffmpeg -encoders` output lists an Encoder named `name`.
pub fn lists_encoder(listing: &str, name: &str) -> bool {
    listing
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .any(|n| n == name)
}

/// FFmpeg arguments for a Probe Encode of `encoder` at `bit_depth` (8 or 10),
/// in constant-quality mode, to a null output. `None` for Software Encoders,
/// which need no probe. `render_device` is the DRM render node for VAAPI/QSV.
pub fn probe_args(
    encoder: VideoEncoder,
    bit_depth: u8,
    render_device: Option<&str>,
) -> Option<Vec<String>> {
    use VideoEncoder::*;
    let ten = bit_depth > 8;
    let fmt = if ten { "p010le" } else { "nv12" };
    let device = render_device.unwrap_or("/dev/dri/renderD128");
    let mut a: Vec<String> = Vec::new();
    let mut push = |items: &[&str]| a.extend(items.iter().map(|s| s.to_string()));

    push(&["-hide_banner", "-loglevel", "error", "-nostdin"]);
    match encoder {
        LibvpxVp9 | LibsvtAv1 | LibaomAv1 | Libx265 => return None,
        Vp9Qsv | Av1Qsv => {
            let init = match render_device {
                Some(d) => format!("qsv=qs:{d}"),
                None => "qsv=qs".to_string(),
            };
            push(&["-init_hw_device", &init, "-filter_hw_device", "qs"]);
        }
        Vp9Vaapi | Av1Vaapi => {
            let init = format!("vaapi=va:{device}");
            push(&["-init_hw_device", &init, "-filter_hw_device", "va"]);
        }
        Av1Nvenc | Av1Amf => {}
        // HEVC hardware Encoders: one arm each, filled in by that Encoder's ticket
        // (device init here, filter below, quality flags in the last match). Until
        // then they are not probed, which keeps them unavailable.
        // hevc_nvenc: no device init needed.
        HevcNvenc => {}
        // hevc_qsv: -init_hw_device qsv + -filter_hw_device.
        HevcQsv => {
            let init = match render_device {
                Some(d) => format!("qsv=qs:{d}"),
                None => "qsv=qs".to_string(),
            };
            push(&["-init_hw_device", &init, "-filter_hw_device", "qs"]);
        }
        // hevc_vaapi: -init_hw_device vaapi + -filter_hw_device.
        HevcVaapi => {
            let init = format!("vaapi=va:{device}");
            push(&["-init_hw_device", &init, "-filter_hw_device", "va"]);
        }
        // hevc_amf and hevc_videotoolbox: no device init needed.
        HevcAmf | HevcVideotoolbox => {}
    }
    push(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=640x360:rate=30",
        "-frames:v",
        "5",
    ]);
    match encoder {
        Vp9Qsv | Av1Qsv | HevcQsv => {
            push(&["-vf", &format!("format={fmt},hwupload=extra_hw_frames=64")])
        }
        Vp9Vaapi | Av1Vaapi | HevcVaapi => push(&["-vf", &format!("format={fmt},hwupload")]),
        _ => push(&["-pix_fmt", fmt]),
    }
    push(&["-c:v", encoder.ffmpeg_name()]);
    match encoder {
        // CQP: ICQ via -global_quality is broken on Intel Arc (see build_vp9_qsv_cmd).
        Vp9Qsv => push(&["-q:v", PROBE_QUALITY]),
        Av1Qsv => push(&["-rc_mode", "cqp", "-q:v", PROBE_QUALITY]),
        Vp9Vaapi => push(&["-rc_mode:v", "1", "-global_quality:v", PROBE_QUALITY]),
        Av1Vaapi => push(&["-rc_mode:v", "CQP", "-global_quality:v", PROBE_QUALITY]),
        Av1Nvenc => push(&["-rc", "vbr", "-cq", PROBE_QUALITY]),
        Av1Amf => push(&["-rc", "cqp", "-qp_i", PROBE_QUALITY, "-qp_p", PROBE_QUALITY]),
        // CQP through -q:v, the same flag the hevc_qsv command uses.
        HevcQsv => push(&["-q:v", PROBE_QUALITY]),
        HevcVaapi => push(&["-rc_mode:v", "CQP", "-global_quality:v", PROBE_QUALITY]),
        LibvpxVp9 | LibsvtAv1 | LibaomAv1 | Libx265 => unreachable!(),
        HevcNvenc => push(&["-rc", "vbr", "-cq", PROBE_QUALITY, "-b:v", "0"]),
        // AMF QP, constant quality.
        HevcAmf => push(&["-rc", "cqp", "-qp_i", PROBE_QUALITY, "-qp_p", PROBE_QUALITY]),
        // VideoToolbox constant quality (`q:v`, 1-100, higher is better). Intel Macs
        // reject it, which is how they fall back to libx265. 30 is a legal value.
        HevcVideotoolbox => push(&["-q:v", PROBE_QUALITY]),
    }
    push(&["-f", "null", "-"]);
    Some(a)
}

/// FFmpeg arguments for a Dolby Vision Probe Encode of `encoder`: a few 10-bit
/// PQ/BT.2020 frames encoded with `-dolbyvision 1`. `None` for Encoders that
/// cannot carry Dolby Vision at all (every Hardware Encoder, VP9, libaom).
///
/// libx265 is encoded to a null output (success of the encode is the signal; it
/// needs VBV settings for Dolby Vision HRD). libsvtav1 writes `output`, which
/// [`run_dolby_vision_probe`] then checks with ffprobe, because an AV1 encode can
/// succeed without producing a real Dolby Vision stream.
pub fn dolby_vision_probe_args(encoder: VideoEncoder, output: &str) -> Option<Vec<String>> {
    let mut a: Vec<String> = Vec::new();
    let mut push = |items: &[&str]| a.extend(items.iter().map(|s| s.to_string()));
    push(&["-hide_banner", "-loglevel", "error", "-nostdin"]);
    push(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=640x360:rate=30",
        "-frames:v",
        "5",
        "-pix_fmt",
        "yuv420p10le",
        "-color_primaries",
        "bt2020",
        "-color_trc",
        "smpte2084",
        "-colorspace",
        "bt2020nc",
        "-c:v",
    ]);
    match encoder {
        VideoEncoder::Libx265 => {
            push(&[
                "libx265",
                "-x265-params",
                "log-level=error:vbv-bufsize=20000:vbv-maxrate=20000:hrd=1:aud=1:repeat-headers=1",
                "-dolbyvision",
                "1",
                "-f",
                "null",
                "-",
            ]);
        }
        VideoEncoder::LibsvtAv1 => {
            push(&["libsvtav1", "-dolbyvision", "1", "-y", output]);
        }
        _ => return None,
    }
    Some(a)
}

/// The Dolby Vision Profile ffprobe must report for the probe output, when the
/// encode alone is not proof. Only AV1 (Profile 10) is checked.
pub fn dolby_vision_probe_expected_profile(encoder: VideoEncoder) -> Option<u8> {
    match encoder {
        VideoEncoder::LibsvtAv1 => Some(10),
        _ => None,
    }
}

/// Whether ffprobe JSON (`-show_streams -of json`) reports a Dolby Vision
/// configuration record of `profile` on a stream.
pub fn reports_dolby_vision_profile(ffprobe_json: &str, profile: u8) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(ffprobe_json) else {
        return false;
    };
    v["streams"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| s["side_data_list"].as_array().into_iter().flatten())
        .any(|sd| {
            sd["side_data_type"].as_str() == Some("DOVI configuration record")
                && sd["dv_profile"].as_u64() == Some(u64::from(profile))
        })
}

/// Run one Probe Encode on this machine. True if FFmpeg exits successfully in time.
fn run_probe(encoder: VideoEncoder, bit_depth: u8) -> bool {
    let render_device = hardware::detect_render_device();
    let Some(args) = probe_args(encoder, bit_depth, render_device.as_deref()) else {
        return true;
    };
    let mut cmd = Command::new("ffmpeg");
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if matches!(encoder.backend(), hardware::Backend::Vaapi)
        && let Some(cfg) = hardware::detect_vaapi_config()
    {
        cmd.env("LIBVA_DRIVERS_PATH", &cfg.driver.path);
        cmd.env("LIBVA_DRIVER_NAME", &cfg.driver.name);
    }
    run_with_timeout(cmd)
}

/// Run one Dolby Vision Probe Encode. True if the encode succeeds and, where
/// required, ffprobe reports the expected Dolby Vision Profile on the output.
fn run_dolby_vision_probe(encoder: VideoEncoder) -> bool {
    let out = std::env::temp_dir().join(format!(
        "ffdash_dv_probe_{}_{}.mkv",
        std::process::id(),
        encoder.ffmpeg_name()
    ));
    let Some(args) = out
        .to_str()
        .and_then(|o| dolby_vision_probe_args(encoder, o))
    else {
        return false;
    };
    let mut cmd = Command::new("ffmpeg");
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut ok = run_with_timeout(cmd);
    if ok && let Some(profile) = dolby_vision_probe_expected_profile(encoder) {
        ok = Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0", "-show_streams"])
            .args(["-of", "json"])
            .arg(&out)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .is_ok_and(|o| {
                o.status.success()
                    && reports_dolby_vision_profile(&String::from_utf8_lossy(&o.stdout), profile)
            });
    }
    let _ = std::fs::remove_file(&out);
    ok
}

/// Spawn `cmd` and wait for it; false on spawn failure, non-zero exit or timeout.
fn run_with_timeout(mut cmd: Command) -> bool {
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if start.elapsed() < PROBE_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Build Encoder Availability by probing every candidate that FFmpeg lists.
/// Hardware candidates are probed in parallel: 8-bit first, then Main10 only
/// for those that passed. Software Encoders count as available when listed.
pub fn probe_availability(listing: &str) -> EncoderAvailability {
    let mut availability = EncoderAvailability::default();
    for codec in CodecKind::ALL {
        let soft = codec.software_fallback();
        if lists_encoder(listing, soft.ffmpeg_name()) {
            availability = availability.with(soft, true);
        }
    }
    let listed: Vec<VideoEncoder> = hardware_candidates()
        .into_iter()
        .filter(|e| lists_encoder(listing, e.ffmpeg_name()))
        .collect();
    let results: Vec<(VideoEncoder, bool, bool)> = std::thread::scope(|s| {
        let handles: Vec<_> = listed
            .iter()
            .map(|&e| {
                s.spawn(move || {
                    let ok = run_probe(e, 8);
                    (e, ok, ok && run_probe(e, 10))
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    for (e, ok, main10) in results {
        if ok {
            availability = availability.with(e, main10);
        }
    }
    let dv_capable: Vec<VideoEncoder> = std::thread::scope(|s| {
        let handles: Vec<_> = dolby_vision_candidates(&availability)
            .into_iter()
            .map(|e| s.spawn(move || (e, run_dolby_vision_probe(e))))
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .filter_map(|(e, ok)| ok.then_some(e))
            .collect()
    });
    for e in dv_capable {
        availability = availability.with_dolby_vision(e);
    }
    availability
}

/// The Encoders that get a Dolby Vision Probe Encode: only those that already
/// passed the normal probe (are in `availability`) and have a Dolby Vision probe.
pub fn dolby_vision_candidates(availability: &EncoderAvailability) -> Vec<VideoEncoder> {
    CodecKind::ALL
        .iter()
        .flat_map(|c| {
            c.hardware_priority()
                .iter()
                .copied()
                .chain([c.software_fallback()])
        })
        .filter(|e| availability.is_available(*e) && dolby_vision_probe_args(*e, "").is_some())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use VideoEncoder::*;

    fn joined(e: VideoEncoder, depth: u8) -> String {
        probe_args(e, depth, Some("/dev/dri/renderD129"))
            .unwrap()
            .join(" ")
    }

    #[test]
    fn every_hardware_priority_encoder_is_a_candidate_with_a_probe() {
        let c = hardware_candidates();
        for codec in CodecKind::ALL {
            for e in codec.hardware_priority() {
                assert!(c.contains(e));
                assert!(probe_args(*e, 8, None).is_some(), "{e:?}");
                assert!(probe_args(*e, 10, None).is_some(), "{e:?}");
            }
        }
        // Encoders are unique per Codec, so candidates are exactly the listed priorities.
        let listed: usize = CodecKind::ALL
            .iter()
            .map(|k| k.hardware_priority().len())
            .sum();
        assert_eq!(c.len(), listed);
        assert!(c.contains(&HevcQsv));
        assert!(c.contains(&HevcNvenc));
        assert!(c.contains(&HevcAmf) && c.contains(&HevcVideotoolbox));
        assert!(c.contains(&HevcVaapi));
    }

    #[test]
    fn software_encoders_need_no_probe() {
        assert!(probe_args(LibvpxVp9, 8, None).is_none());
        assert!(probe_args(LibsvtAv1, 10, None).is_none());
    }

    #[test]
    fn probes_are_few_frame_null_output_encodes() {
        for e in hardware_candidates() {
            let s = joined(e, 8);
            assert!(s.contains("-f lavfi -i testsrc="), "{e:?}");
            assert!(s.contains("-frames:v 5"), "{e:?}");
            assert!(s.ends_with("-f null -"), "{e:?}");
            assert!(s.contains(&format!("-c:v {}", e.ffmpeg_name())), "{e:?}");
        }
    }

    #[test]
    fn probes_use_constant_quality_mode() {
        assert!(joined(Av1Nvenc, 8).contains("-rc vbr -cq 30"));
        assert!(joined(HevcNvenc, 8).contains("-rc vbr -cq 30 -b:v 0"));
        assert!(joined(HevcNvenc, 10).contains("-pix_fmt p010le"));
        assert!(!joined(HevcNvenc, 8).contains("-init_hw_device"));
        assert!(joined(Av1Amf, 8).contains("-rc cqp -qp_i 30 -qp_p 30"));
        assert!(joined(Av1Qsv, 8).contains("-rc_mode cqp -q:v 30"));
        assert!(joined(Vp9Qsv, 8).contains("-q:v 30"));
        assert!(joined(HevcQsv, 8).contains("-c:v hevc_qsv -q:v 30"));
        assert!(joined(Av1Vaapi, 8).contains("-rc_mode:v CQP -global_quality:v 30"));
        assert!(joined(Vp9Vaapi, 8).contains("-rc_mode:v 1 -global_quality:v 30"));
    }

    #[test]
    fn hevc_amf_and_videotoolbox_probe_in_constant_quality() {
        let amf = joined(HevcAmf, 8);
        assert!(
            amf.contains("-c:v hevc_amf -rc cqp -qp_i 30 -qp_p 30"),
            "{amf}"
        );
        assert!(!amf.contains("-init_hw_device"));
        let vt = joined(HevcVideotoolbox, 10);
        assert!(vt.contains("-c:v hevc_videotoolbox -q:v 30"), "{vt}");
        assert!(vt.contains("-pix_fmt p010le"), "{vt}");
        assert!(!vt.contains("-init_hw_device"));
    }

    #[test]
    fn hevc_vaapi_probe_is_cqp_with_a_vaapi_device_and_main10_surfaces() {
        let s8 = joined(HevcVaapi, 8);
        assert!(s8.contains("-init_hw_device vaapi=va:/dev/dri/renderD129 -filter_hw_device va"));
        assert!(s8.contains("format=nv12,hwupload"));
        assert!(s8.contains("-c:v hevc_vaapi -rc_mode:v CQP -global_quality:v 30"));
        assert!(joined(HevcVaapi, 10).contains("format=p010le,hwupload"));
    }

    #[test]
    fn main10_probe_uses_ten_bit_surfaces() {
        assert!(joined(Av1Nvenc, 10).contains("-pix_fmt p010le"));
        assert!(joined(Av1Amf, 10).contains("-pix_fmt p010le"));
        assert!(joined(Av1Vaapi, 10).contains("format=p010le,hwupload"));
        assert!(joined(Av1Qsv, 10).contains("format=p010le,hwupload=extra_hw_frames=64"));
        assert!(joined(HevcQsv, 10).contains("format=p010le,hwupload=extra_hw_frames=64"));
        assert!(joined(Av1Nvenc, 8).contains("-pix_fmt nv12"));
        assert!(joined(Av1Vaapi, 8).contains("format=nv12,hwupload"));
    }

    #[test]
    fn device_init_follows_the_render_node() {
        assert!(joined(Av1Vaapi, 8).contains("-init_hw_device vaapi=va:/dev/dri/renderD129"));
        assert!(joined(Av1Qsv, 8).contains("-init_hw_device qsv=qs:/dev/dri/renderD129"));
        let no_node = probe_args(Av1Qsv, 8, None).unwrap().join(" ");
        assert!(no_node.contains("-init_hw_device qsv=qs "));
        assert!(joined(HevcQsv, 8).contains("-init_hw_device qsv=qs:/dev/dri/renderD129"));
        assert!(!joined(Av1Nvenc, 8).contains("-init_hw_device"));
    }

    #[test]
    fn listing_match_is_by_encoder_name_not_description() {
        let listing = " Encoders:\n V....D libsvtav1             SVT-AV1 (codec av1)\n V....D av1_nvenc           NVIDIA NVENC av1 encoder (codec av1)\n V....D libx264 mentions av1_qsv in text\n";
        assert!(lists_encoder(listing, "av1_nvenc"));
        assert!(lists_encoder(listing, "libsvtav1"));
        assert!(!lists_encoder(listing, "av1_qsv"));
        assert!(!lists_encoder(listing, "av1"));
    }

    #[test]
    fn dolby_vision_is_probed_only_for_encoders_that_passed_the_normal_probe() {
        let none = EncoderAvailability::default();
        assert!(dolby_vision_candidates(&none).is_empty());
        let a = EncoderAvailability::default()
            .with(Libx265, true)
            .with(HevcNvenc, true);
        assert_eq!(dolby_vision_candidates(&a), vec![Libx265]);
        let both = a.with(LibsvtAv1, true);
        assert_eq!(dolby_vision_candidates(&both), vec![LibsvtAv1, Libx265]);
    }

    fn dv_joined(e: VideoEncoder) -> String {
        dolby_vision_probe_args(e, "probe.mkv").unwrap().join(" ")
    }

    #[test]
    fn only_software_dolby_vision_encoders_have_a_dolby_vision_probe() {
        assert!(dolby_vision_probe_args(Libx265, "p.mkv").is_some());
        assert!(dolby_vision_probe_args(LibsvtAv1, "p.mkv").is_some());
        for e in hardware_candidates() {
            assert!(dolby_vision_probe_args(e, "p.mkv").is_none(), "{e:?}");
        }
        assert!(dolby_vision_probe_args(LibvpxVp9, "p.mkv").is_none());
        assert!(dolby_vision_probe_args(LibaomAv1, "p.mkv").is_none());
    }

    #[test]
    fn dolby_vision_probe_is_a_few_frame_ten_bit_pq_encode_with_dolbyvision() {
        for e in [Libx265, LibsvtAv1] {
            let s = dv_joined(e);
            assert!(s.contains("-f lavfi -i testsrc="), "{e:?}");
            assert!(s.contains("-frames:v 5"), "{e:?}");
            assert!(s.contains("-pix_fmt yuv420p10le"), "{e:?}");
            assert!(s.contains("-color_primaries bt2020"), "{e:?}");
            assert!(s.contains("-color_trc smpte2084"), "{e:?}");
            assert!(s.contains("-colorspace bt2020nc"), "{e:?}");
            assert!(s.contains("-dolbyvision 1"), "{e:?}");
            assert!(s.contains(&format!("-c:v {}", e.ffmpeg_name())), "{e:?}");
        }
    }

    #[test]
    fn x265_dolby_vision_probe_sets_vbv_and_discards_output() {
        let s = dv_joined(Libx265);
        assert!(s.contains("vbv-bufsize=") && s.contains("vbv-maxrate="));
        assert!(s.ends_with("-f null -"));
    }

    #[test]
    fn av1_dolby_vision_probe_writes_the_given_file_for_ffprobe() {
        let s = dv_joined(LibsvtAv1);
        assert!(s.ends_with("-y probe.mkv"));
        assert_eq!(dolby_vision_probe_expected_profile(LibsvtAv1), Some(10));
        assert_eq!(dolby_vision_probe_expected_profile(Libx265), None);
    }

    #[test]
    fn ffprobe_json_with_dolby_vision_profile_10_is_recognised() {
        let json = r#"{"streams":[{"codec_name":"av1","side_data_list":[
            {"side_data_type":"DOVI configuration record","dv_profile":10,
             "dv_bl_signal_compatibility_id":0}]}]}"#;
        assert!(reports_dolby_vision_profile(json, 10));
        assert!(!reports_dolby_vision_profile(json, 8));
    }

    #[test]
    fn ffprobe_json_without_dolby_vision_is_not_recognised() {
        let plain = r#"{"streams":[{"codec_name":"av1"}]}"#;
        assert!(!reports_dolby_vision_profile(plain, 10));
        let other_side_data = r#"{"streams":[{"side_data_list":[
            {"side_data_type":"Mastering display metadata"}]}]}"#;
        assert!(!reports_dolby_vision_profile(other_side_data, 10));
        assert!(!reports_dolby_vision_profile("not json", 10));
        assert!(!reports_dolby_vision_profile(r#"{"streams":[]}"#, 10));
    }
}
