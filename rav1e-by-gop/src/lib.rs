#![allow(clippy::cognitive_complexity)]

pub mod compress;
pub mod encode;
pub mod muxer;
pub mod remote;

pub use self::compress::*;
pub use self::encode::*;
pub use self::muxer::*;
pub use self::remote::*;
use crossbeam_channel::{Receiver, Sender};
use rav1e::prelude::*;
use serde::{Deserialize, Serialize};
use std::cmp;
use std::io::{Cursor, Read};
use std::path::PathBuf;
use tungstenite::client::AutoStream;
use tungstenite::WebSocket;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct VideoDetails {
    pub width: usize,
    pub height: usize,
    pub bit_depth: usize,
    pub chroma_sampling: ChromaSampling,
    pub chroma_sample_position: ChromaSamplePosition,
    pub time_base: Rational,
}

impl Default for VideoDetails {
    fn default() -> Self {
        VideoDetails {
            width: 640,
            height: 480,
            bit_depth: 8,
            chroma_sampling: ChromaSampling::Cs420,
            chroma_sample_position: ChromaSamplePosition::Unknown,
            time_base: Rational { num: 30, den: 1 },
        }
    }
}

pub enum Slot {
    Local(usize),
    Remote(Box<ActiveConnection>),
}

pub struct SegmentData {
    pub segment_no: usize,
    pub slot: usize,
    pub next_analysis_frame: usize,
    pub start_frameno: usize,
    pub compressed_frames: Vec<Vec<u8>>,
}

pub struct ActiveConnection {
    pub socket: WebSocket<AutoStream>,
    pub connection_id: Option<Uuid>,
    pub slot_in_worker: usize,
    pub video_info: VideoDetails,
    pub encode_info: Option<EncodeInfo>,
    pub worker_update_sender: WorkerUpdateSender,
    pub progress_sender: ProgressSender,
}

#[derive(Debug, Clone)]
pub struct EncodeInfo {
    pub output_file: PathBuf,
    pub frame_count: usize,
    pub next_analysis_frame: usize,
    pub segment_idx: usize,
    pub start_frameno: usize,
}

pub type WorkerUpdateSender = Sender<WorkerStatusUpdate>;
pub type WorkerUpdateReceiver = Receiver<WorkerStatusUpdate>;
pub type WorkerUpdateChannel = (WorkerUpdateSender, WorkerUpdateReceiver);

#[derive(Debug, Clone, Copy)]
pub struct WorkerStatusUpdate {
    pub status: Option<SlotStatus>,
    pub slot_delta: Option<(usize, bool)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SlotStatus {
    None,
    Requested,
}

pub fn build_encoder_config(
    speed: usize,
    qp: usize,
    max_bitrate: Option<i32>,
    video_info: VideoDetails,
    segment_len: usize,
    first_pass_data: Option<&mut Cursor<Vec<u8>>>,
) -> Config {
    let cfg = Config::new()
        .with_threads(1)
        .with_encoder_config(build_base_encoder_config(
            speed,
            qp,
            max_bitrate,
            video_info,
            segment_len,
        ));
    if let Some(fp_data) = first_pass_data {
        let mut buflen = [0u8; 8];
        fp_data.read_exact(&mut buflen).unwrap();

        let len = i64::from_be_bytes(buflen);
        let mut buf = vec![0u8; len as usize];
        fp_data.read_exact(&mut buf).unwrap();

        cfg.with_rate_control(RateControlConfig::from_summary_slice(&buf).unwrap())
    } else {
        cfg
    }
}

pub fn build_first_pass_encoder_config(
    speed: usize,
    qp: usize,
    max_bitrate: Option<i32>,
    video_info: VideoDetails,
    segment_len: usize,
) -> Config {
    let mut enc_config =
        build_base_encoder_config(cmp::max(speed, 9), qp, max_bitrate, video_info, segment_len);
    enc_config.speed_settings.cdef = false;
    enc_config.speed_settings.lrf = false;
    Config::new()
        .with_threads(1)
        .with_encoder_config(enc_config)
        .with_rate_control(RateControlConfig::new().with_emit_data(true))
}

pub fn build_base_encoder_config(
    speed: usize,
    qp: usize,
    max_bitrate: Option<i32>,
    video_info: VideoDetails,
    segment_len: usize,
) -> EncoderConfig {
    let mut enc_config = EncoderConfig::with_speed_preset(speed);
    enc_config.width = video_info.width;
    enc_config.height = video_info.height;
    enc_config.bit_depth = video_info.bit_depth;
    enc_config.chroma_sampling = video_info.chroma_sampling;
    enc_config.chroma_sample_position = video_info.chroma_sample_position;
    enc_config.time_base = video_info.time_base;
    if let Some(bitrate) = max_bitrate {
        enc_config.min_quantizer = qp as u8;
        enc_config.bitrate = bitrate;
        enc_config.reservoir_frame_delay = Some(cmp::max(12, segment_len as i32));
    } else {
        enc_config.quantizer = qp;
    }
    enc_config.tiles = 1;
    enc_config.min_key_frame_interval = 0;
    enc_config.max_key_frame_interval = u16::max_value() as u64;
    enc_config.speed_settings.no_scene_detection = true;
    enc_config
}
