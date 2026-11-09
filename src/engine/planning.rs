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
        // Dolby Vision preservation is not implemented yet, so every Dolby Vision
        // source is skipped rather than silently downgraded (ADR 0003).
        return JobPlan::Skip(format!("Dolby Vision Profile {profile} not supported yet"));
    }
    JobPlan::Encode(EncodePlan::new(selection::select_encoder(
        req.codec,
        req.use_hardware,
        req.preferred,
        req.bit_depth,
        req.availability,
    )))
}

/// Plan a job on this host: [`plan_job`] with Encoder Availability from the
/// session's Probe Encodes.
pub fn plan_job_on_host(
    codec: &Codec,
    container: &str,
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
    plan_job(&PlanRequest {
        codec: kind,
        container,
        dolby_vision,
        use_hardware,
        preferred: preferred_encoder.and_then(VideoEncoder::from_ffmpeg_name),
        bit_depth,
        availability: hardware::detect_encoder_availability(),
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

    #[test]
    fn dolby_vision_sources_are_skipped_with_their_profile_in_the_reason() {
        let avail = availability();
        let expected = [
            (
                DolbyVisionProfile::P5,
                "Dolby Vision Profile 5 not supported yet",
            ),
            (
                DolbyVisionProfile::P7,
                "Dolby Vision Profile 7 not supported yet",
            ),
            (
                DolbyVisionProfile::P8_1,
                "Dolby Vision Profile 8.1 not supported yet",
            ),
            (
                DolbyVisionProfile::P8_4,
                "Dolby Vision Profile 8.4 not supported yet",
            ),
            (
                DolbyVisionProfile::Other {
                    profile: 10,
                    compat_id: 0,
                },
                "Dolby Vision Profile 10 not supported yet",
            ),
        ];
        for (profile, reason) in expected {
            for codec in CodecKind::ALL {
                for container in ["mkv", "mp4", "webm"] {
                    let plan = plan_job(&request(codec, container, Some(profile), &avail));
                    assert_eq!(
                        plan,
                        JobPlan::Skip(reason.to_string()),
                        "{profile} {codec:?} {container}"
                    );
                }
            }
        }
        assert_eq!(expected.len(), ALL_PROFILES.len());
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
