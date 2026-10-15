//! Reliable link layer over a noisy byte pipe (selective repeat ARQ).
//!
//! Sequenced packets (RPC and TCP stream frames) are numbered per direction,
//! cumulatively acknowledged in every frame, and retransmitted after a NAK or a
//! timeout. Damaged frames are dropped by the codec instead of ending the
//! session, so bit errors on the cable cost one retransmit, not a reconnect.
//! Each session draws a random link id; a peer restart shows up as a HELLO from
//! a new id and still ends the session (pending RPCs are never replayed).
//!
//! The state machine is pure: time is passed in, I/O is done by the transport.
use crate::{
    error::{BridgeError, Result},
    protocol::{self, Header, Packet, Wire},
};
use bytes::{Bytes, BytesMut};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    time::Duration,
};
use tokio::time::Instant;
use uuid::Uuid;

/// Max unacknowledged frames / bytes for bulk traffic.
pub const WINDOW_FRAMES: usize = 256;
pub const WINDOW_BYTES: usize = 256 * 1024;
/// Control frames (STREAM_ACK, RESPONSE, ...) may use extra room so flow control
/// never waits behind a window filled with bulk data.
const CTL_EXTRA_FRAMES: usize = 256;
const CTL_EXTRA_BYTES: usize = 64 * 1024;
/// Out-of-order frames accepted ahead of the next expected sequence.
const RX_AHEAD: u32 = (WINDOW_FRAMES + CTL_EXTRA_FRAMES) as u32;
const HELLO_IDLE: Duration = Duration::from_secs(5);
const HELLO_HANDSHAKE: Duration = Duration::from_secs(1);
pub const LINK_TIMEOUT: Duration = Duration::from_secs(25);
const RTO_BASE: Duration = Duration::from_millis(300);
const RTO_MAX: Duration = Duration::from_secs(30);
const MAX_BACKOFF: u32 = 8;
const PROBE_DELAY: Duration = Duration::from_millis(50);
/// Minimum spacing of repeated gap NAKs for the same sequence.
const REPEAT_GAP: Duration = Duration::from_millis(300);

struct Pending {
    seq: u32,
    kind: u8,
    id: Uuid,
    json: Bytes,
    data: Bytes,
    sent_at: Option<Instant>,
}
impl Pending {
    fn size(&self) -> usize {
        protocol::OVERHEAD + self.json.len() + self.data.len()
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub retransmits: u64,
    pub damaged: u64,
    pub naks_sent: u64,
}

/// Result of feeding one received frame.
#[derive(Debug, Default)]
pub struct Incoming {
    /// The handshake completed with this frame.
    pub adopted: bool,
    /// HELLO metadata from the adopted (or candidate) peer.
    pub hello: Option<Value>,
    /// Sequenced packets now deliverable in order.
    pub deliver: Vec<Packet>,
}

pub struct Link {
    local: u32,
    remote: Option<u32>,
    candidate: Option<u32>,
    /// Bytes per second of the physical line; `None` when unknown/fast.
    rate: Option<u64>,
    // Sender.
    tx_next: u32,
    base: u32,
    unacked: VecDeque<Pending>,
    unacked_bytes: usize,
    sent: usize,
    retransmit: VecDeque<u32>,
    backoff: u32,
    /// After the sender goes idle with unacked frames, one bare ACK carrying
    /// our next sequence lets the receiver NAK a lost tail before the RTO.
    probe_at: Option<Instant>,
    progress_at: Instant,
    // Receiver.
    rx_next: u32,
    ooo: BTreeMap<u32, Packet>,
    naks: VecDeque<u32>,
    nak_at: HashMap<u32, Instant>,
    ack_due: bool,
    // Timers.
    hello_due: Instant,
    last_rx: Instant,
    pub stats: Stats,
}

fn link_id() -> u32 {
    loop {
        let b = Uuid::new_v4();
        let id = u32::from_be_bytes(b.as_bytes()[..4].try_into().unwrap());
        if id != 0 {
            return id;
        }
    }
}
/// `a - b` interpreted in wrapping sequence space.
fn dist(a: u32, b: u32) -> u32 {
    a.wrapping_sub(b)
}

impl Link {
    pub fn new(rate: Option<u64>, now: Instant) -> Self {
        Self::with_id(link_id(), rate, now)
    }
    pub fn with_id(local: u32, rate: Option<u64>, now: Instant) -> Self {
        Self {
            local,
            remote: None,
            candidate: None,
            rate: rate.filter(|r| *r > 0),
            tx_next: 0,
            base: 0,
            unacked: VecDeque::new(),
            unacked_bytes: 0,
            sent: 0,
            retransmit: VecDeque::new(),
            backoff: 1,
            probe_at: None,
            progress_at: now,
            rx_next: 0,
            ooo: BTreeMap::new(),
            naks: VecDeque::new(),
            nak_at: HashMap::new(),
            ack_due: false,
            hello_due: now,
            last_rx: now,
            stats: Stats::default(),
        }
    }
    pub fn adopted(&self) -> bool {
        self.remote.is_some()
    }
    /// Whether a new packet of this class fits the send window.
    pub fn can_send(&self, ctl: bool) -> bool {
        if !self.adopted() {
            return false;
        }
        if self.unacked.is_empty() {
            return true;
        }
        let (frames, bytes) = if ctl {
            (
                WINDOW_FRAMES + CTL_EXTRA_FRAMES,
                WINDOW_BYTES + CTL_EXTRA_BYTES,
            )
        } else {
            (WINDOW_FRAMES, WINDOW_BYTES)
        };
        self.unacked.len() < frames && self.unacked_bytes < bytes
    }
    /// Queues a sequenced packet. Fails only if it can never be encoded.
    pub fn push(&mut self, p: Packet) -> Result<()> {
        if !protocol::sequenced(p.kind) {
            return Err(BridgeError::invalid("Packet kind is not sequenced"));
        }
        let json = Bytes::from(serde_json::to_vec(&p.meta)?);
        if json.len() + p.data.len() > protocol::MAX_PAYLOAD {
            return Err(BridgeError::invalid("Frame too large"));
        }
        let pending = Pending {
            seq: self.tx_next,
            kind: p.kind,
            id: p.id,
            json,
            data: p.data,
            sent_at: None,
        };
        self.tx_next = self.tx_next.wrapping_add(1);
        self.unacked_bytes += pending.size();
        self.unacked.push_back(pending);
        Ok(())
    }
    fn rto(&self) -> Duration {
        let mut rto = RTO_BASE;
        if let Some(rate) = self.rate {
            // Our queued bytes plus whatever the peer may send before its ACK.
            let bytes = (self.unacked_bytes + WINDOW_BYTES + CTL_EXTRA_BYTES) as u64;
            rto += Duration::from_millis(bytes * 1000 / rate);
        }
        (rto * self.backoff).min(RTO_MAX)
    }
    fn on_ack(&mut self, ack: u32, now: Instant) {
        let n = dist(ack, self.base) as usize;
        if n == 0 || n > self.sent {
            return;
        }
        for _ in 0..n {
            let p = self.unacked.pop_front().unwrap();
            self.unacked_bytes -= p.size();
        }
        self.sent -= n;
        self.base = ack;
        if self.sent == 0 {
            self.probe_at = None;
        }
        self.backoff = 1;
        self.progress_at = now;
        let (base, sent) = (self.base, self.sent as u32);
        self.retransmit.retain(|s| dist(*s, base) < sent);
    }
    fn on_nak(&mut self, seq: u32) {
        let i = dist(seq, self.base) as usize;
        if i < self.sent && !self.retransmit.contains(&seq) {
            self.retransmit.push_back(seq);
        }
    }
    fn want_nak(&mut self, seq: u32, now: Instant) {
        if self
            .nak_at
            .get(&seq)
            .is_some_and(|t| now.duration_since(*t) < REPEAT_GAP)
            || self.naks.contains(&seq)
        {
            return;
        }
        self.nak_at.insert(seq, now);
        self.naks.push_back(seq);
    }
    /// NAKs every missing sequence in `rx_next..upto`.
    fn nak_gap(&mut self, upto: u32, now: Instant) {
        let n = dist(upto, self.rx_next).min(RX_AHEAD);
        for k in 0..n {
            let seq = self.rx_next.wrapping_add(k);
            if !self.ooo.contains_key(&seq) {
                self.want_nak(seq, now);
            }
        }
    }
    /// Feeds one decoded wire item.
    pub fn on_wire(&mut self, wire: Wire, now: Instant) -> Result<Incoming> {
        let mut out = Incoming::default();
        let (h, packet) = match wire {
            Wire::Frame(f) => (f.header, Some(f.packet)),
            Wire::Damaged(h) => (h, None),
        };
        if h.src == self.local {
            // Our own frames echoed back (TX wired to RX).
            return Err(BridgeError::invalid(
                "Loopback peer: received our own link frames",
            ));
        }
        match self.remote {
            Some(r) if h.src == r => {}
            Some(_) => {
                if packet.is_some() && h.kind == protocol::HELLO && h.dst == 0 {
                    return Err(BridgeError::new(
                        "PEER_DISCONNECTED",
                        "Remote session changed; pending calls are not replayed",
                    ));
                }
                return Ok(out);
            }
            None => {
                let Some(p) = packet.as_ref() else {
                    return Ok(out);
                };
                if h.kind == protocol::HELLO && (h.dst == 0 || h.dst == self.local) {
                    if self.candidate != Some(h.src) {
                        self.candidate = Some(h.src);
                        // Answer at once so the peer can adopt us.
                        self.hello_due = now;
                    }
                    out.hello = Some(p.meta.clone());
                }
                if self.candidate == Some(h.src) && h.dst == self.local {
                    self.remote = Some(h.src);
                    self.progress_at = now;
                    self.hello_due = now;
                    out.adopted = true;
                } else {
                    return Ok(out);
                }
            }
        }
        // From here on the frame comes from the adopted peer.
        if h.dst != self.local {
            return Ok(out);
        }
        self.last_rx = now;
        let Some(packet) = packet else {
            self.stats.damaged += 1;
            if protocol::sequenced(h.kind) {
                let ahead = dist(h.seq, self.rx_next);
                if ahead < RX_AHEAD && !self.ooo.contains_key(&h.seq) {
                    // Each damaged arrival is one failed transmission: NAK it
                    // even if an earlier NAK for the same sequence is recent.
                    self.nak_gap(h.seq, now);
                    if !self.naks.contains(&h.seq) {
                        self.nak_at.insert(h.seq, now);
                        self.naks.push_back(h.seq);
                    }
                }
            }
            return Ok(out);
        };
        self.on_ack(h.ack, now);
        match h.kind {
            protocol::HELLO => {
                out.hello = Some(packet.meta);
                // `seq` is the peer's next sequence: anything below it that we
                // lack was lost (tail loss with no later frame to reveal it).
                if dist(h.seq, self.rx_next) <= RX_AHEAD {
                    self.nak_gap(h.seq, now);
                }
            }
            protocol::LINK_ACK => {
                if dist(h.seq, self.rx_next) <= RX_AHEAD {
                    self.nak_gap(h.seq, now);
                }
            }
            protocol::LINK_NAK => self.on_nak(h.seq),
            kind if protocol::sequenced(kind) => {
                let ahead = dist(h.seq, self.rx_next);
                if ahead >= RX_AHEAD {
                    // Duplicate of something already delivered: re-ACK it.
                    self.ack_due = true;
                    return Ok(out);
                }
                self.ack_due = true;
                if ahead > 0 {
                    self.nak_gap(h.seq, now);
                    self.ooo.insert(h.seq, packet);
                    return Ok(out);
                }
                out.deliver.push(packet);
                self.rx_next = self.rx_next.wrapping_add(1);
                while let Some(p) = self.ooo.remove(&self.rx_next) {
                    out.deliver.push(p);
                    self.rx_next = self.rx_next.wrapping_add(1);
                }
                let rx = self.rx_next;
                self.nak_at.retain(|s, _| dist(*s, rx) < RX_AHEAD);
                self.naks.retain(|s| dist(*s, rx) < RX_AHEAD);
            }
            _ => return Err(BridgeError::invalid("Invalid packet kind")),
        }
        Ok(out)
    }
    fn header(&self, kind: u8, seq: u32) -> Header {
        Header {
            kind,
            src: self.local,
            dst: self.remote.or(self.candidate).unwrap_or(0),
            seq,
            ack: self.rx_next,
        }
    }
    /// Next sent sequence (hint for tail-loss detection).
    fn sent_next(&self) -> u32 {
        self.base.wrapping_add(self.sent as u32)
    }
    /// Encodes the next frame to write, if any. `hello` supplies HELLO metadata.
    pub fn next_out(
        &mut self,
        now: Instant,
        hello: impl FnOnce() -> Value,
        out: &mut BytesMut,
    ) -> Result<bool> {
        if now >= self.hello_due {
            self.hello_due = now
                + if self.adopted() {
                    HELLO_IDLE
                } else {
                    HELLO_HANDSHAKE
                };
            let json = serde_json::to_vec(&hello())?;
            let h = self.header(protocol::HELLO, self.sent_next());
            protocol::encode(out, h, &Uuid::nil(), &json, &[])?;
            self.ack_due = false;
            return Ok(true);
        }
        if !self.adopted() {
            return Ok(false);
        }
        if let Some(seq) = self.naks.pop_front() {
            self.stats.naks_sent += 1;
            let h = self.header(protocol::LINK_NAK, seq);
            protocol::encode(out, h, &Uuid::nil(), b"{}", &[])?;
            self.ack_due = false;
            return Ok(true);
        }
        let head_expired = self.sent > 0
            && self.unacked[0]
                .sent_at
                .is_some_and(|t| now.duration_since(t.max(self.progress_at)) >= self.rto());
        let resend = match self.retransmit.pop_front() {
            Some(seq) => Some(dist(seq, self.base) as usize),
            None if head_expired => {
                self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
                Some(0)
            }
            None => None,
        };
        if let Some(i) = resend.filter(|i| *i < self.sent) {
            self.stats.retransmits += 1;
            self.write_pending(i, now, out)?;
            return Ok(true);
        }
        if self.sent < self.unacked.len() {
            let i = self.sent;
            self.sent += 1;
            self.write_pending(i, now, out)?;
            return Ok(true);
        }
        let probe = self.probe_at.is_some_and(|t| now >= t);
        if self.ack_due || probe {
            if probe {
                self.probe_at = None;
            }
            let h = self.header(protocol::LINK_ACK, self.sent_next());
            protocol::encode(out, h, &Uuid::nil(), b"{}", &[])?;
            self.ack_due = false;
            return Ok(true);
        }
        Ok(false)
    }
    fn write_pending(&mut self, i: usize, now: Instant, out: &mut BytesMut) -> Result<()> {
        let drain = self
            .rate
            .map(|r| Duration::from_millis(self.unacked_bytes as u64 * 1000 / r))
            .unwrap_or_default();
        self.probe_at = Some(now + PROBE_DELAY + drain);
        let (kind, seq) = (self.unacked[i].kind, self.unacked[i].seq);
        let h = self.header(kind, seq);
        let p = &mut self.unacked[i];
        p.sent_at = Some(now);
        protocol::encode(out, h, &p.id, &p.json, &p.data)?;
        self.ack_due = false;
        Ok(())
    }
    /// Earliest instant at which [`Link::next_out`] or [`Link::check`] has work.
    pub fn deadline(&self, now: Instant) -> Instant {
        if self.adopted()
            && (!self.naks.is_empty()
                || !self.retransmit.is_empty()
                || self.sent < self.unacked.len()
                || self.ack_due)
        {
            return now;
        }
        let mut at = self.hello_due.min(self.last_rx + LINK_TIMEOUT);
        if let Some(t) = self.probe_at.filter(|_| self.adopted()) {
            at = at.min(t);
        }
        if self.sent > 0 {
            if let Some(t) = self.unacked[0].sent_at {
                at = at.min(t.max(self.progress_at) + self.rto());
            }
        }
        at
    }
    /// Fails when the peer has been silent too long.
    pub fn check(&self, now: Instant) -> Result<()> {
        if now.duration_since(self.last_rx) > LINK_TIMEOUT {
            return Err(BridgeError::new("TIMEOUT", "Peer heartbeat timed out"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::FrameCodec;
    use serde_json::json;
    use tokio_util::codec::Decoder;

    fn drain(link: &mut Link, now: Instant) -> BytesMut {
        let mut out = BytesMut::new();
        while link
            .next_out(now, || json!({"hello":true}), &mut out)
            .unwrap()
        {}
        out
    }
    fn feed(link: &mut Link, bytes: &mut BytesMut, now: Instant) -> Vec<Incoming> {
        let mut v = Vec::new();
        while let Some(w) = FrameCodec.decode(bytes).unwrap() {
            v.push(link.on_wire(w, now).unwrap());
        }
        v
    }
    fn pair(now: Instant) -> (Link, Link) {
        let mut a = Link::with_id(1, None, now);
        let mut b = Link::with_id(2, None, now);
        for _ in 0..3 {
            let mut x = drain(&mut a, now);
            feed(&mut b, &mut x, now);
            let mut y = drain(&mut b, now);
            feed(&mut a, &mut y, now);
        }
        assert!(a.adopted() && b.adopted());
        (a, b)
    }
    fn data(n: u32) -> Packet {
        Packet::new(
            protocol::STREAM_DATA,
            Uuid::nil(),
            json!({"n":n}),
            Bytes::from(vec![n as u8; 100]),
        )
    }

    #[test]
    fn handshake_adopts_both_sides() {
        let now = Instant::now();
        let mut a = Link::with_id(1, None, now);
        let mut b = Link::with_id(2, None, now);
        let mut x = drain(&mut a, now);
        let r = feed(&mut b, &mut x, now);
        assert!(r[0].hello.is_some() && !b.adopted());
        let mut y = drain(&mut b, now);
        let r = feed(&mut a, &mut y, now);
        assert!(r[0].adopted && a.adopted());
        let mut z = drain(&mut a, now);
        let r = feed(&mut b, &mut z, now);
        assert!(r[0].adopted && b.adopted());
    }

    #[test]
    fn restarted_peer_hello_ends_the_session() {
        let now = Instant::now();
        let (mut a, _b) = pair(now);
        let mut c = Link::with_id(3, None, now);
        let mut x = drain(&mut c, now);
        let w = FrameCodec.decode(&mut x).unwrap().unwrap();
        assert!(a.on_wire(w, now).is_err());
    }

    #[test]
    fn own_frames_are_reported_as_loopback() {
        let now = Instant::now();
        let mut a = Link::with_id(1, None, now);
        let mut x = drain(&mut a, now);
        let w = FrameCodec.decode(&mut x).unwrap().unwrap();
        assert!(a.on_wire(w, now).is_err());
    }

    /// Delivers every packet exactly once and in order despite random loss and
    /// corruption in both directions.
    #[test]
    fn lossy_link_delivers_in_order() {
        let mut now = Instant::now();
        let (mut a, mut b) = pair(now);
        let total = 2000u32;
        let mut pushed = 0;
        let mut got = Vec::new();
        let mut rng = 0x1234_5678u64;
        let mut chaos = |buf: &mut BytesMut| {
            for byte in buf.iter_mut() {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                if rng.is_multiple_of(1500) {
                    *byte ^= 0x10;
                }
            }
        };
        for _ in 0..20_000 {
            while pushed < total && a.can_send(false) {
                a.push(data(pushed)).unwrap();
                pushed += 1;
            }
            let mut x = drain(&mut a, now);
            chaos(&mut x);
            for r in feed(&mut b, &mut x, now) {
                got.extend(r.deliver.into_iter().map(|p| p.meta["n"].as_u64().unwrap()));
            }
            let mut y = drain(&mut b, now);
            chaos(&mut y);
            feed(&mut a, &mut y, now);
            if got.len() == total as usize && a.unacked.is_empty() {
                break;
            }
            now += Duration::from_millis(50);
        }
        assert_eq!(got, (0..total as u64).collect::<Vec<_>>());
        assert!(a.unacked.is_empty());
        assert!(a.stats.retransmits > 0);
    }

    #[test]
    fn tail_loss_is_recovered_by_timeout() {
        let mut now = Instant::now();
        let (mut a, mut b) = pair(now);
        a.push(data(0)).unwrap();
        drain(&mut a, now); // lost on the wire
        assert!(a.deadline(now) > now);
        now += Duration::from_secs(1);
        let mut x = drain(&mut a, now);
        let r = feed(&mut b, &mut x, now);
        assert_eq!(r.iter().map(|r| r.deliver.len()).sum::<usize>(), 1);
        let mut y = drain(&mut b, now);
        feed(&mut a, &mut y, now);
        assert!(a.unacked.is_empty());
    }

    #[test]
    fn duplicate_frames_are_not_delivered_twice() {
        let now = Instant::now();
        let (mut a, mut b) = pair(now);
        a.push(data(0)).unwrap();
        let x = drain(&mut a, now);
        let mut first = x.clone();
        let mut second = x;
        assert_eq!(feed(&mut b, &mut first, now)[0].deliver.len(), 1);
        assert!(feed(&mut b, &mut second, now)[0].deliver.is_empty());
    }

    #[test]
    fn window_limits_bulk_but_leaves_room_for_control() {
        let now = Instant::now();
        let (mut a, _b) = pair(now);
        while a.can_send(false) {
            a.push(data(0)).unwrap();
        }
        assert_eq!(a.unacked.len(), WINDOW_FRAMES);
        assert!(a.can_send(true));
    }

    #[test]
    fn silence_times_out() {
        let now = Instant::now();
        let a = Link::with_id(1, None, now);
        assert!(a.check(now + LINK_TIMEOUT / 2).is_ok());
        assert!(a
            .check(now + LINK_TIMEOUT + Duration::from_secs(1))
            .is_err());
    }
}
