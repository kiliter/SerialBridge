//! Wire format SBR2 (all integers big-endian):
//! magic `SBR2`[4], version:u8 = 2, kind:u8, reserved:u16 = 0, src_link:u32,
//! dst_link:u32, seq:u32, ack:u32, payload_len:u32, UUID[16], json_len:u32,
//! header CRC32 (bytes 4..48), JSON metadata, raw data, payload CRC32.
//! A damaged header is skipped as noise; a damaged payload with an intact header
//! is reported as [`Wire::Damaged`] so the link layer can request a retransmit.
use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde_json::Value;
use std::io;
use tokio_util::codec::Decoder;
use uuid::Uuid;
pub const MAX_PAYLOAD: usize = 128 * 1024;
pub const VERSION: u8 = 2;
/// Header bytes including the header CRC.
pub const HEADER_LEN: usize = 52;
/// Fixed bytes per frame (header + payload CRC).
pub const OVERHEAD: usize = HEADER_LEN + 4;
/// Unsequenced: link handshake and keepalive. `seq` carries the sender's next sequence.
pub const HELLO: u8 = 1;
pub const REQUEST: u8 = 2;
pub const RESPONSE: u8 = 3;
// 4 was HEARTBEAT in protocol 1; HELLO now doubles as keepalive.
pub const CANCEL: u8 = 5;
/// TCP forwarding frames; the id field is the stream id.
pub const STREAM_OPEN: u8 = 6;
pub const STREAM_READY: u8 = 7;
pub const STREAM_DATA: u8 = 8;
pub const STREAM_ACK: u8 = 9;
pub const STREAM_FIN: u8 = 10;
pub const STREAM_RESET: u8 = 11;
/// Unsequenced link acknowledgement; `ack` is the receiver's next expected sequence.
pub const LINK_ACK: u8 = 12;
/// Unsequenced retransmit request; `seq` is the missing sequence.
pub const LINK_NAK: u8 = 13;
/// Frames carried reliably and in order by the link layer.
pub fn sequenced(kind: u8) -> bool {
    matches!(
        kind,
        REQUEST | RESPONSE | CANCEL | STREAM_OPEN..=STREAM_RESET
    )
}
fn valid_kind(kind: u8) -> bool {
    (1..=LINK_NAK).contains(&kind) && kind != 4
}
#[derive(Debug, Clone)]
pub struct Packet {
    pub kind: u8,
    pub id: Uuid,
    pub meta: Value,
    pub data: Bytes,
}
impl Packet {
    pub fn new(kind: u8, id: Uuid, meta: Value, data: Bytes) -> Self {
        Self {
            kind,
            id,
            meta,
            data,
        }
    }
}
/// Link-layer addressing of one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Header {
    pub kind: u8,
    pub src: u32,
    pub dst: u32,
    pub seq: u32,
    pub ack: u32,
}
#[derive(Debug, Clone)]
pub struct Frame {
    pub header: Header,
    pub packet: Packet,
}
#[derive(Debug, Clone)]
pub enum Wire {
    Frame(Frame),
    /// Header intact, payload corrupted.
    Damaged(Header),
}
fn invalid(s: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s.into())
}
/// Appends one frame. `json` is the serialized metadata.
pub fn encode(
    dst: &mut BytesMut,
    h: Header,
    id: &Uuid,
    json: &[u8],
    data: &[u8],
) -> io::Result<()> {
    let len = json.len() + data.len();
    if len > MAX_PAYLOAD {
        return Err(invalid("Frame too large"));
    }
    let start = dst.len();
    dst.reserve(OVERHEAD + len);
    dst.extend_from_slice(b"SBR2");
    dst.put_u8(VERSION);
    dst.put_u8(h.kind);
    dst.put_u16(0);
    dst.put_u32(h.src);
    dst.put_u32(h.dst);
    dst.put_u32(h.seq);
    dst.put_u32(h.ack);
    dst.put_u32(len as u32);
    dst.extend_from_slice(id.as_bytes());
    dst.put_u32(json.len() as u32);
    let crc = crc32fast::hash(&dst[start + 4..]);
    dst.put_u32(crc);
    dst.extend_from_slice(json);
    dst.extend_from_slice(data);
    let crc = crc32fast::hash(&dst[start + HEADER_LEN..]);
    dst.put_u32(crc);
    Ok(())
}
/// Convenience encoder for a whole packet.
pub fn encode_packet(dst: &mut BytesMut, h: Header, p: &Packet) -> io::Result<()> {
    let json = serde_json::to_vec(&p.meta).map_err(|e| invalid(e.to_string()))?;
    encode(dst, Header { kind: p.kind, ..h }, &p.id, &json, &p.data)
}
fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
}
/// Protocol-1 frames are only recognized to report an incompatible peer; the
/// size cap keeps a false `SBR1` match in noise from stalling the decoder.
const V1_PROBE_MAX: usize = 4096;
#[derive(Default)]
pub struct FrameCodec;
impl Decoder for FrameCodec {
    type Item = Wire;
    type Error = io::Error;
    fn decode(&mut self, src: &mut BytesMut) -> io::Result<Option<Wire>> {
        loop {
            match src.windows(4).position(|w| w == b"SBR2" || w == b"SBR1") {
                Some(i) => src.advance(i),
                None => {
                    let n = src.len().saturating_sub(3);
                    src.advance(n);
                    return Ok(None);
                }
            }
            if &src[..4] == b"SBR1" {
                if src.len() < 32 {
                    return Ok(None);
                }
                let len = be32(src, 8) as usize;
                let plausible = src[4] == 1
                    && (1..=11).contains(&src[5])
                    && src[6] == 0
                    && src[7] == 0
                    && len <= V1_PROBE_MAX
                    && be32(src, 28) as usize <= len;
                if !plausible {
                    src.advance(1);
                    continue;
                }
                if src.len() < 36 + len {
                    return Ok(None);
                }
                if crc32fast::hash(&src[4..32 + len]) == be32(src, 32 + len) {
                    return Err(invalid(
                        "Peer speaks SerialBridge protocol 1; upgrade both ends",
                    ));
                }
                src.advance(1);
                continue;
            }
            if src.len() < HEADER_LEN {
                return Ok(None);
            }
            if crc32fast::hash(&src[4..48]) != be32(src, 48) {
                // Corrupted header: treat as noise and rescan.
                src.advance(1);
                continue;
            }
            let kind = src[5];
            let len = be32(src, 24) as usize;
            let json_len = be32(src, 44) as usize;
            if src[4] != VERSION || src[6] != 0 || src[7] != 0 || !valid_kind(kind) {
                return Err(invalid("Unsupported frame version/header"));
            }
            if len > MAX_PAYLOAD || json_len > len {
                return Err(invalid("Invalid frame length"));
            }
            if src.len() < OVERHEAD + len {
                src.reserve(OVERHEAD + len - src.len());
                return Ok(None);
            }
            let wire = src.split_to(OVERHEAD + len);
            let header = Header {
                kind,
                src: be32(&wire, 8),
                dst: be32(&wire, 12),
                seq: be32(&wire, 16),
                ack: be32(&wire, 20),
            };
            let body = &wire[HEADER_LEN..HEADER_LEN + len];
            if crc32fast::hash(body) != be32(&wire, HEADER_LEN + len) {
                return Ok(Some(Wire::Damaged(header)));
            }
            let packet = Packet {
                kind,
                id: Uuid::from_slice(&wire[28..44]).map_err(|_| invalid("Invalid UUID"))?,
                meta: serde_json::from_slice(&body[..json_len])
                    .map_err(|e| invalid(e.to_string()))?,
                data: Bytes::copy_from_slice(&body[json_len..]),
            };
            return Ok(Some(Wire::Frame(Frame { header, packet })));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn h(kind: u8, seq: u32) -> Header {
        Header {
            kind,
            src: 7,
            dst: 9,
            seq,
            ack: 3,
        }
    }
    fn frame(kind: u8, seq: u32, meta: Value, data: &'static [u8]) -> BytesMut {
        let mut b = BytesMut::new();
        encode_packet(
            &mut b,
            h(kind, seq),
            &Packet::new(kind, Uuid::new_v4(), meta, Bytes::from_static(data)),
        )
        .unwrap();
        b
    }
    fn unwrap_frame(w: Option<Wire>) -> Frame {
        match w {
            Some(Wire::Frame(f)) => f,
            other => panic!("expected frame, got {other:?}"),
        }
    }
    #[test]
    fn fragmented_and_binary_roundtrip() {
        let encoded = frame(REQUEST, 5, json!({"method":"file.write"}), &[0, 255, 1, 2]);
        let mut codec = FrameCodec;
        let mut input = BytesMut::new();
        let mut decoded = None;
        for b in encoded {
            input.extend_from_slice(&[b]);
            if let Some(w) = codec.decode(&mut input).unwrap() {
                decoded = Some(w)
            }
        }
        let f = unwrap_frame(decoded);
        assert_eq!(f.header, h(REQUEST, 5));
        assert_eq!(&f.packet.data[..], &[0, 255, 1, 2]);
        assert_eq!(f.packet.meta, json!({"method":"file.write"}));
    }
    #[test]
    fn payload_damage_is_reported_with_trusted_header() {
        let mut b = frame(STREAM_DATA, 42, json!({}), b"test");
        b[HEADER_LEN + 3] ^= 1;
        match FrameCodec.decode(&mut b).unwrap() {
            Some(Wire::Damaged(header)) => assert_eq!(header, h(STREAM_DATA, 42)),
            other => panic!("expected damaged, got {other:?}"),
        }
        assert!(b.is_empty());
    }
    #[test]
    fn header_damage_is_skipped_and_next_frame_decodes() {
        let mut b = frame(REQUEST, 1, json!({}), b"lost");
        b[18] ^= 0x10;
        b.extend_from_slice(&frame(REQUEST, 2, json!({}), b"kept"));
        let f = unwrap_frame(FrameCodec.decode(&mut b).unwrap());
        assert_eq!(f.header.seq, 2);
        assert_eq!(&f.packet.data[..], b"kept");
    }
    #[test]
    fn noise_can_be_resynchronized() {
        let mut b = BytesMut::from(&b"junkSBR2SBR"[..]);
        b.extend_from_slice(&frame(HELLO, 0, json!({}), b""));
        assert_eq!(
            unwrap_frame(FrameCodec.decode(&mut b).unwrap()).header.kind,
            HELLO
        );
    }
    #[test]
    fn protocol_one_peer_is_reported() {
        let mut b = BytesMut::from(&b"SBR1\x01\x01\0\0\0\0\0\x02"[..]);
        b.extend_from_slice(&[0; 16]);
        b.extend_from_slice(&2u32.to_be_bytes());
        b.extend_from_slice(b"{}");
        let crc = crc32fast::hash(&b[4..]);
        b.put_u32(crc);
        let err = FrameCodec.decode(&mut b).unwrap_err();
        assert!(err.to_string().contains("protocol 1"));
    }
    #[test]
    fn oversized_header_with_valid_crc_is_rejected_before_allocation() {
        let mut b = BytesMut::from(&b"SBR2\x02\x02\0\0"[..]);
        b.resize(24, 0);
        b.put_u32(u32::MAX);
        b.resize(48, 0);
        let crc = crc32fast::hash(&b[4..48]);
        b.put_u32(crc);
        assert!(FrameCodec.decode(&mut b).is_err());
    }
    #[test]
    fn concatenated_frames_are_decoded_in_order() {
        let mut wire = BytesMut::new();
        for n in 0..3 {
            wire.extend_from_slice(&frame(REQUEST, n, json!({"n":n}), b""));
        }
        for n in 0..3 {
            let f = unwrap_frame(FrameCodec.decode(&mut wire).unwrap());
            assert_eq!(f.packet.meta["n"], n);
        }
        assert!(wire.is_empty());
    }
    #[test]
    fn encoder_accepts_exact_limit_and_rejects_one_more_byte() {
        let mut wire = BytesMut::new();
        let id = Uuid::nil();
        encode(
            &mut wire,
            h(REQUEST, 0),
            &id,
            b"{}",
            &vec![1; MAX_PAYLOAD - 2],
        )
        .unwrap();
        let f = unwrap_frame(FrameCodec.decode(&mut wire).unwrap());
        assert_eq!(f.packet.data.len(), MAX_PAYLOAD - 2);
        assert!(encode(
            &mut wire,
            h(REQUEST, 0),
            &id,
            b"{}",
            &vec![1; MAX_PAYLOAD - 1]
        )
        .is_err());
        assert!(wire.is_empty());
    }
}
