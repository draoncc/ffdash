//! Job planning: one level above Encoder selection.
//!
//! Decides whether a job can run at all and, if so, with which Encode Plan. Pure
//! over its inputs: Encoder Availability is passed in, never discovered here.
//! Encoder selection ([`selection::select_encoder`]) stays its own seam and
//! planning wraps it (ADR 0003).

use super::core::{Codec, EncodePlan};
use super::dolby_vision::DolbyVisionProfile;
use super::hardware::{self, VideoEncoder};
use super::selection::{self, CodecKind, EncoderAvailability};

/// Everything job planning decides from.
#[derive(Debug, Clone, Copy)]
pub struct PlanRequest<'a> {
    pub codec: CodecKind,
    /// Output Container extension, lowercase (`mkv`, `mp4`, `webm`).
    pub container: &'a str,
    /// The source's Dolby Vision Profile, if it has Dolby Vision.
    pub dolby_vision: Option<DolbyVisionProfile>,
    pub use_hardware: bool,
    pub preferred: Option<VideoEncoder>,
    pub bit_depth: u8,
    pub availability: &'a EncoderAvailability,
}

/// The outcome of planning one job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobPlan {
    Encode(EncodePlan),
    /// The job must not run; the text is shown to the user.
    Skip(String),
}

/// Plan one job: an Encode Plan, or a skip with a user-facing reason.
pub fn plan_job(req: &PlanRequest) -> JobPlan {
    if let Some(profile) = req.dolby_vision {
        return plan_dolby_vision(req, profile);
    }
    JobPlan::Encode(EncodePlan::new(selection::select_encoder(
        req.codec,
        req.use_hardware,
        req.preferred,
        req.bit_depth,
        req.availability,
    )))
}

/// A Dolby Vision source is preserved or skipped with a reason, never downgraded
/// (ADR 0003). Only HEVC in MKV or MP4 with Profile 5, 8.1 or 8.4 is preserved so far; the
/// other cases are skipped until their tickets land.
fn plan_dolby_vision(req: &PlanRequest, profile: DolbyVisionProfile) -> JobPlan {
    let preservable = req.codec == CodecKind::Hevc
        && matches!(req.container, "mkv" | "mp4")
        && matches!(
            profile,
            DolbyVisionProfile::P5 | DolbyVisionProfile::P8_1 | DolbyVisionProfile::P8_4
        );
    if !preservable {
        return JobPlan::Skip(format!("Dolby Vision Profile {profile} not supported yet"));
    }

    // Dolby Vision output is always 10-bit (Main10).
    let encoder = selection::select_encoder_requiring_dolby_vision(
        req.codec,
        req.use_hardware,
        req.preferred,
        10,
        req.availability,
    );
    let can_encode = req
        .availability
        .capability(encoder)
        .is_some_and(|c| c.dolby_vision);
    if !can_encode {
        return JobPlan::Skip(
            "Your FFmpeg can't encode Dolby Vision; upgrade FFmpeg to keep Dolby Vision"
                .to_string(),
        );
    }
    JobPlan::Encode(EncodePlan::new(encoder).with_dolby_vision(profile))
}

/// Plan a job on this host: [`plan_job`] with Encoder Availability from the
/// session's Probe Encodes.
///
/// For a Dolby Vision job the availability comes from the lazy Dolby Vision Probe
/// Encode against `source` ([`hardware::detect_dolby_vision_availability`]), which
/// blocks: call this off the UI thread.
pub fn plan_job_on_host(
    codec: &Codec,
    container: &str,
    source: &std::path::Path,
    dolby_vision: Option<DolbyVisionProfile>,
    use_hardware: bool,
    preferred_encoder: Option<&str>,
    bit_depth: u8,
) -> JobPlan {
    let kind = match codec {
        Codec::Vp9(_) => CodecKind::Vp9,
        Codec::Av1(_) => CodecKind::Av1,
        Codec::Hevc(_) => CodecKind::Hevc,
    };
    // Dolby Vision jobs need the lazy Dolby Vision probe (cached per session).
    let dolby_vision_availability;
    let availability = if dolby_vision.is_some() {
        dolby_vision_availability = hardware::detect_dolby_vision_availability(source);
        &dolby_vision_availability
    } else {
        hardware::detect_encoder_availability()
    };
    plan_job(&PlanRequest {
        codec: kind,
        container,
        dolby_vision,
        use_hardware,
        preferred: preferred_encoder.and_then(VideoEncoder::from_ffmpeg_name),
        bit_depth,
        availability,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use VideoEncoder::*;

    const ALL_PROFILES: [DolbyVisionProfile; 5] = [
        DolbyVisionProfile::P5,
        DolbyVisionProfile::P7,
        DolbyVisionProfile::P8_1,
        DolbyVisionProfile::P8_4,
        DolbyVisionProfile::Other {
            profile: 10,
            compat_id: 0,
        },
    ];

    fn availability() -> EncoderAvailability {
        EncoderAvailability::default()
            .with(Libx265, true)
            .with(HevcNvenc, true)
            .with(LibsvtAv1, true)
            .with(Av1Nvenc, true)
    }

    fn request<'a>(
        codec: CodecKind,
        container: &'static str,
        dolby_vision: Option<DolbyVisionProfile>,
        availability: &'a EncoderAvailability,
    ) -> PlanRequest<'a> {
        PlanRequest {
            codec,
            container,
            dolby_vision,
            use_hardware: true,
            preferred: None,
            bit_depth: 10,
            availability,
        }
    }

    /// Availability where both Software Fallbacks passed the Dolby Vision Probe Encode.
    fn dv_availability() -> EncoderAvailability {
        availability()
            .with_dolby_vision(Libx265)
            .with_dolby_vision(LibsvtAv1)
    }

    #[test]
    fn hevc_mkv_preserves_profiles_5_8_1_and_8_4_on_libx265() {
        let avail = dv_availability();
        for profile in [
            DolbyVisionProfile::P5,
            DolbyVisionProfile::P8_1,
            DolbyVisionProfile::P8_4,
        ] {
            let plan = plan_job(&request(CodecKind::Hevc, "mkv", Some(profile), &avail));
            assert_eq!(
                plan,
                JobPlan::Encode(EncodePlan::new(Libx265).with_dolby_vision(profile)),
                "{profile}"
            );
        }
    }

    #[test]
    fn dolby_vision_job_falls_back_to_libx265_with_hardware_selected_or_preferred() {
        // HevcNvenc is available with Main10 but has no Dolby Vision flag.
        let avail = dv_availability();
        for preferred in [None, Some(HevcNvenc)] {
            let req = PlanRequest {
                preferred,
                ..request(
                    CodecKind::Hevc,
                    "mkv",
                    Some(DolbyVisionProfile::P8_1),
                    &avail,
                )
            };
            let JobPlan::Encode(plan) = plan_job(&req) else {
                panic!("expected an Encode Plan");
            };
            assert_eq!(plan.encoder, Libx265);
        }
    }

    #[test]
    fn no_dolby_vision_capable_encoder_skips_with_an_upgrade_ffmpeg_reason() {
        let avail = availability(); // no Dolby Vision flags
        for profile in [
            DolbyVisionProfile::P5,
            DolbyVisionProfile::P8_1,
            DolbyVisionProfile::P8_4,
        ] {
            let plan = plan_job(&request(CodecKind::Hevc, "mkv", Some(profile), &avail));
            let JobPlan::Skip(reason) = plan else {
                panic!("expected a skip for {profile}");
            };
            assert!(reason.contains("can't encode Dolby Vision"), "{reason}");
            assert!(reason.contains("upgrade"), "{reason}");
        }
    }

    #[test]
    fn dolby_vision_cases_without_a_ticket_yet_are_skipped_with_their_profile() {
        let avail = dv_availability();
        let not_yet = |profile: DolbyVisionProfile, codec, container| {
            let plan = plan_job(&request(codec, container, Some(profile), &avail));
            assert_eq!(
                plan,
                JobPlan::Skip(format!("Dolby Vision Profile {profile} not supported yet")),
                "{profile} {codec:?} {container}"
            );
        };
        for profile in ALL_PROFILES {
            // AV1 and VP9 in any Container
            for container in ["mkv", "mp4", "webm"] {
                not_yet(profile, CodecKind::Av1, container);
                not_yet(profile, CodecKind::Vp9, container);
            }
            // HEVC in WebM
            not_yet(profile, CodecKind::Hevc, "webm");
        }
        // Profile 7 and other profiles in MKV and MP4 HEVC
        for container in ["mkv", "mp4"] {
            not_yet(DolbyVisionProfile::P7, CodecKind::Hevc, container);
            not_yet(ALL_PROFILES[4], CodecKind::Hevc, container);
        }
    }

    #[test]
    fn hevc_mp4_preserves_profiles_5_8_1_and_8_4_on_libx265() {
        let avail = dv_availability();
        for profile in [
            DolbyVisionProfile::P5,
            DolbyVisionProfile::P8_1,
            DolbyVisionProfile::P8_4,
        ] {
            let plan = plan_job(&request(CodecKind::Hevc, "mp4", Some(profile), &avail));
            assert_eq!(
                plan,
                JobPlan::Encode(EncodePlan::new(Libx265).with_dolby_vision(profile)),
                "{profile}"
            );
        }
    }

    #[test]
    fn hevc_mp4_without_a_dolby_vision_capable_encoder_skips_like_mkv() {
        let avail = availability();
        for profile in [
            DolbyVisionProfile::P5,
            DolbyVisionProfile::P8_1,
            DolbyVisionProfile::P8_4,
        ] {
            let plan = plan_job(&request(CodecKind::Hevc, "mp4", Some(profile), &avail));
            let JobPlan::Skip(reason) = plan else {
                panic!("expected a skip for {profile}");
            };
            assert!(reason.contains("can't encode Dolby Vision"), "{reason}");
        }
    }

    #[test]
    fn sources_without_dolby_vision_plan_exactly_as_encoder_selection_does() {
        let avail = availability();
        for codec in CodecKind::ALL {
            for use_hardware in [false, true] {
                let req = PlanRequest {
                    use_hardware,
                    ..request(codec, "mkv", None, &avail)
                };
                let selected = selection::select_encoder(codec, use_hardware, None, 10, &avail);
                assert_eq!(plan_job(&req), JobPlan::Encode(EncodePlan::new(selected)));
            }
        }
    }

    #[test]
    fn non_dolby_vision_hardware_job_still_gets_the_hardware_encoder() {
        let avail = availability();
        let plan = plan_job(&request(CodecKind::Hevc, "mp4", None, &avail));
        assert_eq!(plan, JobPlan::Encode(EncodePlan::new(HevcNvenc)));
    }
}
