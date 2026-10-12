//! Wire format: magic[4], version:u8, kind:u8, reserved:u16, payload_len:u32BE,
//! request UUID[16], json_len:u32BE, JSON metadata, raw data, CRC32:u32BE.
//! CRC covers bytes from version through data (not the magic). Never replay a corrupt RPC.
use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde_json::Value;
use std::io;
use tokio_util::codec::{Decoder,Encoder};
use uuid::Uuid;
pub const MAX_PAYLOAD:usize=128*1024;
pub const HELLO:u8=1;pub const REQUEST:u8=2;pub const RESPONSE:u8=3;pub const HEARTBEAT:u8=4;pub const CANCEL:u8=5;
#[derive(Debug,Clone)]
pub struct Packet {pub kind:u8,pub id:Uuid,pub meta:Value,pub data:Bytes}
impl Packet {pub fn new(kind:u8,id:Uuid,meta:Value,data:Bytes)->Self {Self{kind,id,meta,data}}}
#[derive(Default)] pub struct FrameCodec;
fn invalid(s:impl Into<String>)->io::Error {io::Error::new(io::ErrorKind::InvalidData,s.into())}
impl Encoder<Packet> for FrameCodec {
    type Error=io::Error;
    fn encode(&mut self,item:Packet,dst:&mut BytesMut)->io::Result<()> {
        let json=serde_json::to_vec(&item.meta).map_err(|e|invalid(e.to_string()))?;let len=json.len()+item.data.len();
        if len>MAX_PAYLOAD{return Err(invalid("Frame too large"))}
        let start=dst.len();dst.reserve(36+len);dst.extend_from_slice(b"SBR1");dst.put_u8(1);dst.put_u8(item.kind);dst.put_u16(0);dst.put_u32(len as u32);dst.extend_from_slice(item.id.as_bytes());dst.put_u32(json.len() as u32);dst.extend_from_slice(&json);dst.extend_from_slice(&item.data);
        let crc=crc32fast::hash(&dst[start+4..]);dst.put_u32(crc);Ok(())
    }
}
impl Decoder for FrameCodec {
    type Item=Packet;type Error=io::Error;
    fn decode(&mut self,src:&mut BytesMut)->io::Result<Option<Packet>> {
        if src.len()<4{return Ok(None)}
        if &src[..4]!=b"SBR1" {
            if let Some(i)=src.windows(4).position(|w|w==b"SBR1"){src.advance(i)}else{let n=src.len().saturating_sub(3);src.advance(n);return Ok(None)}
        }
        if src.len()<32{return Ok(None)}
        if src[4]!=1||src[6]!=0||src[7]!=0||!(1..=5).contains(&src[5]){return Err(invalid("Unsupported frame version/header"))}
        let len=u32::from_be_bytes(src[8..12].try_into().unwrap()) as usize;
        let json_len=u32::from_be_bytes(src[28..32].try_into().unwrap()) as usize;
        if len>MAX_PAYLOAD||json_len>len {return Err(invalid("Invalid frame length"))}
        if src.len()<36+len{return Ok(None)}
        let wire=src.split_to(36+len);
        let expected=u32::from_be_bytes(wire[32+len..36+len].try_into().unwrap());
        if crc32fast::hash(&wire[4..32+len])!=expected{return Err(invalid("CRC mismatch"))}
        Ok(Some(Packet{kind:wire[5],id:Uuid::from_slice(&wire[12..28]).map_err(|_|invalid("Invalid UUID"))?,meta:serde_json::from_slice(&wire[32..32+json_len]).map_err(|e|invalid(e.to_string()))?,data:Bytes::copy_from_slice(&wire[32+json_len..32+len])}))
    }
}
#[cfg(test)]mod tests{
    use super::*;use serde_json::json;
    #[test]fn fragmented_and_binary_roundtrip(){let p=Packet::new(REQUEST,Uuid::new_v4(),json!({"method":"file.write"}),Bytes::from_static(&[0,255,1,2]));let mut codec=FrameCodec;let mut encoded=BytesMut::new();codec.encode(p.clone(),&mut encoded).unwrap();let mut input=BytesMut::new();let mut decoded=None;for b in encoded {input.extend_from_slice(&[b]);if let Some(p)=codec.decode(&mut input).unwrap(){decoded=Some(p)}}let q=decoded.unwrap();assert_eq!(q.id,p.id);assert_eq!(q.data,p.data);assert_eq!(q.meta,p.meta);}
    #[test]fn crc_failure_is_not_silently_accepted(){let mut c=FrameCodec;let mut b=BytesMut::new();c.encode(Packet::new(REQUEST,Uuid::nil(),json!({}),Bytes::from_static(b"test")),&mut b).unwrap();b[34]^=1;assert!(c.decode(&mut b).is_err());}
    #[test]fn oversized_header_is_rejected_before_allocation(){let mut b=BytesMut::from(&b"SBR1\x01\x02\0\0\xff\xff\xff\xff"[..]);b.resize(32,0);assert!(FrameCodec.decode(&mut b).is_err());}
    #[test]fn noise_can_be_resynchronized(){let mut b=BytesMut::from(&b"junk"[..]);FrameCodec.encode(Packet::new(HELLO,Uuid::nil(),json!({}),Bytes::new()),&mut b).unwrap();assert!(FrameCodec.decode(&mut b).unwrap().is_some());}
}
