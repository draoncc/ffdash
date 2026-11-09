//! The Encode Plan: what a single encode runs with.

use super::profile::Profile;
use crate::engine::dolby_vision::DolbyVisionProfile;
use crate::engine::hardware::{self, VideoEncoder};
use crate::engine::planning::{HostPlanRequest, JobPlan, plan_job_on_host};

/// What a single encode will run with: the selected Encoder and, optionally,
/// a Quality Override (as set by Auto-VMAF).
///
/// Whether hardware is used is derived from the Encoder's Backend, never from
/// whether some config object is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodePlan {
    pub encoder: VideoEncoder,
    pub quality_override: Option<u32>,
    /// The platform the command is built for (defaults to the running OS).
    pub host: HostPlatform,
    /// The Dolby Vision Profile the output carries; none for non-Dolby-Vision jobs.
    pub dolby_vision: Option<DolbyVisionProfile>,
    /// The source is Dolby Vision Profile 7: a Dolby Vision Conversion to 8.1 runs
    /// before the encode, which then reads the converted stream.
    pub dolby_vision_conversion: bool,
}

/// The operating system a command is built for. An explicit input to command building
/// so the non-Linux command shape can be tested on any host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPlatform {
    Linux,
    Windows,
    MacOs,
}

impl HostPlatform {
    /// The platform ffdash is running on.
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }

    /// Linux keeps its VAAPI/hardware-decode paths; elsewhere input is decoded on the CPU
    /// and handed to the Hardware Encoder.
    pub fn decodes_on_cpu(self) -> bool {
        self != Self::Linux
    }
}

impl EncodePlan {
    pub fn new(encoder: VideoEncoder) -> Self {
        Self {
            encoder,
            quality_override: None,
            host: HostPlatform::current(),
            dolby_vision: None,
            dolby_vision_conversion: false,
        }
    }

    /// The output carries this Dolby Vision Profile.
    pub fn with_dolby_vision(mut self, profile: DolbyVisionProfile) -> Self {
        self.dolby_vision = Some(profile);
        self
    }

    /// The source needs a Dolby Vision Conversion to Profile 8.1 before the encode.
    pub fn with_dolby_vision_conversion(mut self) -> Self {
        self.dolby_vision_conversion = true;
        self
    }

    /// The same plan without Dolby Vision (for Auto-VMAF calibration windows, which
    /// only measure quality and never need the metadata).
    pub fn without_dolby_vision(mut self) -> Self {
        self.dolby_vision = None;
        self.dolby_vision_conversion = false;
        self
    }

    /// Build for a specific platform instead of the running OS.
    pub fn with_host(mut self, host: HostPlatform) -> Self {
        self.host = host;
        self
    }

    pub fn with_quality_override(mut self, quality: u32) -> Self {
        self.quality_override = Some(quality);
        self
    }

    /// Select the Encoder for `profile` on this machine.
    /// `hardware_requested` is the caller's explicit request on top of the profile setting;
    /// `bit_depth` is what the job needs (see [`required_bit_depth`]).
    pub fn for_profile(profile: &Profile, hardware_requested: bool, bit_depth: u8) -> Self {
        let mut synced = profile.clone();
        synced.sync_legacy_fields();
        let use_hardware = hardware_requested || synced.use_hardware_encoding;
        Self::new(hardware::select_encoder(
            &synced.codec,
            use_hardware,
            Some(&synced.video_codec),
            bit_depth,
        ))
    }

    /// Whether this plan encodes on a GPU.
    pub fn uses_hardware(&self) -> bool {
        self.encoder.backend().is_hardware()
    }
}

/// Plan `job` on this host: its Encode Plan, or a skip with a user-facing reason.
/// `hardware_requested` is the caller's request on top of the profile's own setting.
/// Both dispatch paths (worker dispatch and the encode path) plan through here.
pub fn plan_job_for_profile(
    job: &super::types::VideoJob,
    profile: &Profile,
    hardware_requested: bool,
) -> JobPlan {
    let mut synced = profile.clone();
    synced.sync_legacy_fields();
    let container = job
        .output_path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| synced.container.clone());
    plan_job_on_host(&HostPlanRequest {
        codec: &synced.codec,
        container: &container,
        source: &job.input_path,
        dolby_vision: job.dolby_vision.map(|dv| dv.profile),
        use_hardware: hardware_requested || synced.use_hardware_encoding,
        preferred_encoder: Some(&synced.video_codec),
        bit_depth: required_bit_depth(&synced, &job.input_path),
    })
}

/// Whether `pix_fmt` is a 10-bit pixel format (Main10 for HEVC). The one place that
/// decides this from a pixel format name.
pub fn is_ten_bit_pix_fmt(pix_fmt: &str) -> bool {
    pix_fmt.contains("10")
}

/// Bit depth an encode must be able to produce: 10 if the profile asks for a
/// 10-bit pixel format, or (with `auto_bit_depth`) if the source is 10-bit or more.
pub fn bit_depth_needed(pix_fmt: &str, source_depth: Option<u32>, auto_bit_depth: bool) -> u8 {
    let profile_ten = is_ten_bit_pix_fmt(pix_fmt);
    let source_ten = auto_bit_depth && source_depth.unwrap_or(8) >= 10;
    if profile_ten || source_ten { 10 } else { 8 }
}

/// The bit depth a job on `input` needs from its Encoder, probing the source
/// when `auto_bit_depth` is on (the default).
pub fn required_bit_depth(profile: &Profile, input: &std::path::Path) -> u8 {
    let auto = crate::config::Config::load()
        .map(|c| c.defaults.auto_bit_depth)
        .unwrap_or(true);
    let source = if auto && !is_ten_bit_pix_fmt(&profile.pix_fmt) {
        crate::engine::probe::probe_input_info(input)
            .ok()
            .and_then(|i| i.bit_depth)
    } else {
        None
    };
    bit_depth_needed(&profile.pix_fmt, source, auto)
}

#[cfg(test)]
mod tests {
    use super::{EncodePlan, bit_depth_needed};
    use crate::engine::dolby_vision::DolbyVisionProfile;
    use crate::engine::hardware::VideoEncoder;

    #[test]
    fn calibrated_quality_keeps_dolby_vision_but_calibration_plan_drops_it() {
        let plan =
            EncodePlan::new(VideoEncoder::Libx265).with_dolby_vision(DolbyVisionProfile::P8_1);
        assert_eq!(
            plan.with_quality_override(20).dolby_vision,
            Some(DolbyVisionProfile::P8_1)
        );
        let calibration = plan.without_dolby_vision();
        assert_eq!(calibration.dolby_vision, None);
        assert_eq!(calibration.encoder, VideoEncoder::Libx265);
    }

    #[test]
    fn ten_bit_profile_needs_ten_bits() {
        assert_eq!(bit_depth_needed("yuv420p10le", None, false), 10);
        assert_eq!(bit_depth_needed("yuv420p10le", Some(8), true), 10);
    }

    #[test]
    fn ten_bit_source_needs_ten_bits_only_with_auto_bit_depth() {
        assert_eq!(bit_depth_needed("yuv420p", Some(10), true), 10);
        assert_eq!(bit_depth_needed("yuv420p", Some(10), false), 8);
    }

    #[test]
    fn eight_bit_or_unknown_source_needs_eight_bits() {
        assert_eq!(bit_depth_needed("yuv420p", Some(8), true), 8);
        assert_eq!(bit_depth_needed("yuv420p", None, true), 8);
    }
}
