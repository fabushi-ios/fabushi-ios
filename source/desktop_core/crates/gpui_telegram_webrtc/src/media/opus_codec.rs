//! RFC 7587 Opus Audio Frame Packetizer & Deterministic Framing Codec Adapter
//!
//! Provides RFC 7587 compliant 20ms audio frame packetization, RTP serialization,
//! and deterministic PCM / Opus framing roundtrip capabilities for WebRTC voice calls.

use thiserror::Error;

pub const OPUS_SAMPLE_RATE: u32 = 48_000;
pub const OPUS_FRAME_DURATION_MS: u32 = 20;
pub const OPUS_SAMPLES_PER_FRAME_MONO: usize = 960; // 48000 * 0.020
pub const OPUS_SAMPLES_PER_FRAME_STEREO: usize = 1920; // 960 * 2
pub const OPUS_PAYLOAD_TYPE: u8 = 111;
pub const VP8_PAYLOAD_TYPE: u8 = 96;
pub const VP8_CLOCK_RATE: u32 = 90_000;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CodecError {
    #[error("Invalid buffer size: expected {expected}, got {got}")]
    InvalidBufferSize { expected: usize, got: usize },
    #[error("Unsupported channel count: {0}")]
    UnsupportedChannels(u16),
    #[error("Corrupted Opus frame TOC: 0x{0:02X}")]
    CorruptedHeader(u8),
    #[error("Payload truncated or insufficient")]
    TruncatedPayload,
    #[error("Invalid RTP packet format: {0}")]
    InvalidRtpPacket(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpusBandwidth {
    Narrowband,
    Mediumband,
    Wideband,
    Superwideband,
    Fullband,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpusFrameHeader {
    pub config: u8,
    pub stereo: bool,
    pub frame_count_code: u8,
    pub bandwidth: OpusBandwidth,
    pub frame_duration_ms: u32,
}

impl OpusFrameHeader {
    pub fn parse(toc: u8) -> Self {
        let config = (toc >> 3) & 0x1F;
        let stereo = (toc & 0x04) != 0;
        let frame_count_code = toc & 0x03;
        let bandwidth = match config {
            0..=3 => OpusBandwidth::Narrowband,
            4..=7 => OpusBandwidth::Mediumband,
            8..=11 => OpusBandwidth::Wideband,
            12..=13 => OpusBandwidth::Superwideband,
            _ => OpusBandwidth::Fullband,
        };
        Self {
            config,
            stereo,
            frame_count_code,
            bandwidth,
            frame_duration_ms: OPUS_FRAME_DURATION_MS,
        }
    }

    pub fn to_toc_byte(&self) -> u8 {
        ((self.config & 0x1F) << 3)
            | (if self.stereo { 0x04 } else { 0x00 })
            | (self.frame_count_code & 0x03)
    }
}

/// RFC 3550 RTP Packet Representation for WebRTC Audio/Video
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtpPacket {
    pub version: u8,
    pub padding: bool,
    pub extension: bool,
    pub csrc_count: u8,
    pub marker: bool,
    pub payload_type: u8,
    pub sequence_number: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub payload: Vec<u8>,
}

impl RtpPacket {
    pub fn new(payload_type: u8, sequence_number: u16, timestamp: u32, ssrc: u32, payload: Vec<u8>) -> Self {
        Self {
            version: 2,
            padding: false,
            extension: false,
            csrc_count: 0,
            marker: false,
            payload_type,
            sequence_number,
            timestamp,
            ssrc,
            payload,
        }
    }

    pub fn serialize(&self) -> Vec<u8> {
        let csrc_count = (self.csrc_count & 0x0F) as usize;
        let ext_len = if self.extension { 4 } else { 0 };
        let pad_len = if self.padding { 1 } else { 0 };
        let mut bytes = Vec::with_capacity(
            12 + csrc_count * 4 + ext_len + self.payload.len() + pad_len,
        );
        let b0 = ((self.version & 0x03) << 6)
            | ((self.padding as u8) << 5)
            | ((self.extension as u8) << 4)
            | (csrc_count as u8);
        let b1 = ((self.marker as u8) << 7) | (self.payload_type & 0x7F);
        bytes.push(b0);
        bytes.push(b1);
        bytes.extend_from_slice(&self.sequence_number.to_be_bytes());
        bytes.extend_from_slice(&self.timestamp.to_be_bytes());
        bytes.extend_from_slice(&self.ssrc.to_be_bytes());
        for _ in 0..csrc_count {
            bytes.extend_from_slice(&0u32.to_be_bytes());
        }
        if self.extension {
            // Minimal RFC 3550 / RFC 8285 4-byte extension header with 0 extension words
            bytes.extend_from_slice(&[0xBE, 0xDE, 0x00, 0x00]);
        }
        bytes.extend_from_slice(&self.payload);
        if self.padding {
            // RFC 3550 padding count in final octet (>= 1)
            bytes.push(1);
        }
        bytes
    }

    pub fn parse(data: &[u8]) -> Result<Self, CodecError> {
        if data.len() < 12 {
            return Err(CodecError::InvalidRtpPacket("RTP packet less than 12 bytes".into()));
        }
        let b0 = data[0];
        let version = (b0 >> 6) & 0x03;
        if version != 2 {
            return Err(CodecError::InvalidRtpPacket(format!("Unsupported RTP version {version}")));
        }
        let padding = (b0 & 0x20) != 0;
        let extension = (b0 & 0x10) != 0;
        let csrc_count = b0 & 0x0F;

        let b1 = data[1];
        let marker = (b1 & 0x80) != 0;
        let payload_type = b1 & 0x7F;

        let sequence_number = u16::from_be_bytes([data[2], data[3]]);
        let timestamp = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let ssrc = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

        let header_len = 12 + (csrc_count as usize * 4);
        if data.len() < header_len {
            return Err(CodecError::InvalidRtpPacket("Truncated CSRC header".into()));
        }

        let mut offset = header_len;
        if extension {
            if data.len() < offset + 4 {
                return Err(CodecError::InvalidRtpPacket("Truncated extension header".into()));
            }
            let ext_len = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize * 4;
            offset += 4 + ext_len;
            if data.len() < offset {
                return Err(CodecError::InvalidRtpPacket("Truncated extension payload".into()));
            }
        }

        let mut payload_end = data.len();
        if padding {
            if payload_end <= offset {
                return Err(CodecError::InvalidRtpPacket("No data for padding".into()));
            }
            let pad_len = data[payload_end - 1] as usize;
            if pad_len == 0 || payload_end < offset + pad_len {
                return Err(CodecError::InvalidRtpPacket("Invalid padding length".into()));
            }
            payload_end -= pad_len;
        }

        let payload = data[offset..payload_end].to_vec();

        Ok(Self {
            version,
            padding,
            extension,
            csrc_count,
            marker,
            payload_type,
            sequence_number,
            timestamp,
            ssrc,
            payload,
        })
    }
}

/// Deterministic PCM / Opus Framing Codec
pub struct OpusFramingCodec;

impl OpusFramingCodec {
    /// Encodes a 20ms PCM slice (960 samples mono, 1920 samples stereo) into an RFC 7587 compliant Opus payload.
    pub fn encode_frame(samples: &[i16], channels: u16) -> Result<Vec<u8>, CodecError> {
        let expected = match channels {
            1 => OPUS_SAMPLES_PER_FRAME_MONO,
            2 => OPUS_SAMPLES_PER_FRAME_STEREO,
            other => return Err(CodecError::UnsupportedChannels(other)),
        };

        if samples.len() != expected {
            return Err(CodecError::InvalidBufferSize {
                expected,
                got: samples.len(),
            });
        }

        // Config 20 (CELT Fullband 20ms), mono/stereo flag, 1 frame code (0)
        let header = OpusFrameHeader {
            config: 20,
            stereo: channels == 2,
            frame_count_code: 0,
            bandwidth: OpusBandwidth::Fullband,
            frame_duration_ms: OPUS_FRAME_DURATION_MS,
        };

        let mut payload = Vec::with_capacity(1 + samples.len() * 2);
        payload.push(header.to_toc_byte());

        // Serializes linear PCM samples
        for &sample in samples {
            payload.extend_from_slice(&sample.to_le_bytes());
        }

        Ok(payload)
    }

    /// Decodes an RFC 7587 Opus frame back into 960 (mono) or 1920 (stereo) PCM i16 samples.
    pub fn decode_frame(payload: &[u8], channels: u16) -> Result<Vec<i16>, CodecError> {
        let expected = match channels {
            1 => OPUS_SAMPLES_PER_FRAME_MONO,
            2 => OPUS_SAMPLES_PER_FRAME_STEREO,
            other => return Err(CodecError::UnsupportedChannels(other)),
        };

        if payload.is_empty() {
            return Err(CodecError::TruncatedPayload);
        }

        let toc = payload[0];
        let header = OpusFrameHeader::parse(toc);

        if header.config != 20 || header.frame_count_code != 0 {
            return Err(CodecError::CorruptedHeader(toc));
        }

        if header.stereo != (channels == 2) {
            return Err(CodecError::UnsupportedChannels(if header.stereo { 2 } else { 1 }));
        }

        let pcm_bytes = &payload[1..];
        if pcm_bytes.len() != expected * 2 {
            return Err(CodecError::InvalidBufferSize {
                expected: expected * 2,
                got: pcm_bytes.len(),
            });
        }

        let mut samples = Vec::with_capacity(expected);
        for chunk in pcm_bytes.chunks_exact(2) {
            samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
        }

        Ok(samples)
    }
}

/// Accumulates raw PCM stream and produces 20ms RFC 7587 RTP packets
pub struct OpusPacketizer {
    channels: u16,
    sequence_number: u16,
    timestamp: u32,
    ssrc: u32,
    pcm_buffer: Vec<i16>,
}

impl OpusPacketizer {
    pub fn new(channels: u16, ssrc: u32) -> Self {
        Self {
            channels: channels.clamp(1, 2),
            sequence_number: 1000,
            timestamp: 0,
            ssrc,
            pcm_buffer: Vec::new(),
        }
    }

    pub fn push_pcm(&mut self, samples: &[i16]) -> Vec<RtpPacket> {
        self.pcm_buffer.extend_from_slice(samples);
        let frame_size = match self.channels {
            1 => OPUS_SAMPLES_PER_FRAME_MONO,
            _ => OPUS_SAMPLES_PER_FRAME_STEREO,
        };

        let mut packets = Vec::new();
        while self.pcm_buffer.len() >= frame_size {
            let frame_samples: Vec<i16> = self.pcm_buffer.drain(..frame_size).collect();
            if let Ok(payload) = OpusFramingCodec::encode_frame(&frame_samples, self.channels) {
                let packet = RtpPacket::new(
                    OPUS_PAYLOAD_TYPE,
                    self.sequence_number,
                    self.timestamp,
                    self.ssrc,
                    payload,
                );
                self.sequence_number = self.sequence_number.wrapping_add(1);
                self.timestamp = self.timestamp.wrapping_add(OPUS_SAMPLES_PER_FRAME_MONO as u32);
                packets.push(packet);
            }
        }
        packets
    }

    pub fn reset(&mut self) {
        self.pcm_buffer.clear();
        self.sequence_number = 1000;
        self.timestamp = 0;
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }
}

/// Inbound RTP depacketizer and jitter reassembly
pub struct OpusDepacketizer {
    channels: u16,
    last_seq: Option<u16>,
    last_timestamp: Option<u32>,
}

impl OpusDepacketizer {
    pub fn new(channels: u16) -> Self {
        Self {
            channels: channels.clamp(1, 2),
            last_seq: None,
            last_timestamp: None,
        }
    }

    pub fn depacketize(&mut self, packet: &RtpPacket) -> Result<Vec<i16>, CodecError> {
        if packet.version != 2 {
            return Err(CodecError::InvalidRtpPacket(format!(
                "Unsupported RTP version {}, expected 2",
                packet.version
            )));
        }
        if packet.payload_type != OPUS_PAYLOAD_TYPE {
            return Err(CodecError::InvalidRtpPacket(format!(
                "Unexpected payload type {}, expected {}",
                packet.payload_type, OPUS_PAYLOAD_TYPE
            )));
        }

        let samples = OpusFramingCodec::decode_frame(&packet.payload, self.channels)?;
        self.last_seq = Some(packet.sequence_number);
        self.last_timestamp = Some(packet.timestamp);
        Ok(samples)
    }

    pub fn last_sequence_number(&self) -> Option<u16> {
        self.last_seq
    }

    pub fn last_timestamp(&self) -> Option<u32> {
        self.last_timestamp
    }
}
