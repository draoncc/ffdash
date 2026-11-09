//! Dolby Vision as read from a source: the Dolby Vision Profile and the layers
//! its configuration record declares.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The Dolby Vision Profile of a stream. Profile 8 is split by its base-layer
/// signal compatibility (8.1 is HDR10 compatible, 8.4 is HLG compatible).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DolbyVisionProfile {
    P5,
    P7,
    P8_1,
    P8_4,
    /// Any other Dolby Vision Profile (8.2, 10, ...), by number and compatibility id.
    Other {
        profile: u8,
        compat_id: u8,
    },
}

impl DolbyVisionProfile {
    pub fn from_record(profile: u8, bl_compat_id: u8) -> Self {
        match (profile, bl_compat_id) {
            (5, _) => Self::P5,
            (7, _) => Self::P7,
            (8, 1) => Self::P8_1,
            (8, 4) => Self::P8_4,
            (profile, compat_id) => Self::Other { profile, compat_id },
        }
    }
}

impl fmt::Display for DolbyVisionProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::P5 => write!(f, "5"),
            Self::P7 => write!(f, "7"),
            Self::P8_1 => write!(f, "8.1"),
            Self::P8_4 => write!(f, "8.4"),
            Self::Other {
                profile: 8,
                compat_id,
            } => write!(f, "8.{compat_id}"),
            Self::Other { profile, .. } => write!(f, "{profile}"),
        }
    }
}

/// The Dolby Vision configuration record of a source video stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DolbyVisionInfo {
    pub profile: DolbyVisionProfile,
    pub bl_present: bool,
    pub el_present: bool,
    pub rpu_present: bool,
    pub bl_compat_id: u8,
}
