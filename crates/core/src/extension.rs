use crate::{
    error::{BridgeError, Result},
    node::{BrowserEvent, BrowserLink, Node, PairCode},
};
use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use futures_util::SinkExt;
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
pub fn token_equal(a: &str, b: &str) -> bool {
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}
pub fn valid_origin(s: &str) -> bool {
    let Some(id) = s.strip_prefix("chrome-extension://") else {
        return false;
    };
    id.len() == 32 && id.bytes().all(|b| (b'a'..=b'p').contains(&b))
}
impl Node {
    pub fn new_pair_code(&self) -> Value {
        let code = Uuid::new_v4().simple().to_string()[..12].to_ascii_uppercase();
        *self.pairing.lock().unwrap() = Some(PairCode {
            code: code.clone(),
            expires: Instant::now() + Duration::from_secs(180),
            attempts: 0,
        });
        json!({"code":code,"expires_in_seconds":180})
    }
    pub fn revoke_browser(&self) -> Result<()> {
        let mut cfg = self.config.write().unwrap();
        cfg.extension_origin = None;
        cfg.extension_token = None;
        cfg.save(&self.config_path)?;
        drop(cfg);
        *self.pairing.lock().unwrap() = None;
        if let Some(link) = self.browser.lock().unwrap().take() {
            link.stop.cancel();
            self.browser_pending
                .lock()
                .unwrap()
                .retain(|_, p| p.epoch != link.epoch);
        }
        self.log("warn", "browser", "Browser pairing revoked", "");
        Ok(())
    }
    fn authenticate_browser(&self, origin: &str, first: &Value) -> Result<Option<String>> {
        if !valid_origin(origin) {
            return Err(BridgeError::new("UNAUTHORIZED", "Invalid extension origin"));
        }
        match first["type"].as_str() {
            Some("auth") => {
                let cfg = self.cfg();
                if cfg.extension_origin.as_deref() != Some(origin)
                    || !cfg
                        .extension_token
                        .as_ref()
                        .is_some_and(|t| token_equal(t, first["token"].as_str().unwrap_or("")))
                {
                    return Err(BridgeError::new(
                        "UNAUTHORIZED",
                        "Browser credential rejected",
                    ));
                }
                Ok(None)
            }
            Some("pair") => {
                let mut pairing = self.pairing.lock().unwrap();
                let code = pairing.as_mut().ok_or_else(|| {
                    BridgeError::new(
                        "UNAUTHORIZED",
                        "Generate a pairing code in the desktop console first",
                    )
                })?;
                code.attempts = code.attempts.saturating_add(1);
                if code.expires < Instant::now()
                    || code.attempts > 5
                    || !token_equal(&code.code, first["code"].as_str().unwrap_or(""))
                {
                    return Err(BridgeError::new(
                        "UNAUTHORIZED",
                        "Pairing code invalid, expired or locked",
                    ));
                }
                *pairing = None;
                drop(pairing);
                let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
                let mut cfg = self.config.write().unwrap();
                cfg.extension_origin = Some(origin.to_string());
                cfg.extension_token = Some(token.clone());
                cfg.save(&self.config_path)?;
                Ok(Some(token))
            }
            _ => Err(BridgeError::new(
                "UNAUTHORIZED",
                "Authentication must be the first message",
            )),
        }
    }
}
pub async fn serve_socket(node: Arc<Node>, mut ws: WebSocket, origin: String) {
    let result=async {
        let first=tokio::time::timeout(Duration::from_secs(5),ws.recv()).await.map_err(|_|BridgeError::new("UNAUTHORIZED","Authentication timeout"))?.ok_or_else(||BridgeError::new("UNAUTHORIZED","Socket closed"))?.map_err(BridgeError::io)?;
        let Message::Text(text)=first else{return Err(BridgeError::new("UNAUTHORIZED","Expected authentication JSON"))};
        let value:Value=serde_json::from_str(text.as_str())?;let token=node.authenticate_browser(&origin,&value)?;
        let epoch=Uuid::new_v4();let(tx,mut rx)=mpsc::channel::<Value>(32);let stop=CancellationToken::new();
        let old={node.browser.lock().unwrap().replace(BrowserLink{epoch,tx,stop:stop.clone(),origin:origin.clone()})};
        if let Some(old)=old{old.stop.cancel();node.browser_pending.lock().unwrap().retain(|_,p|p.epoch!=old.epoch);}
        struct Guard{node:Arc<Node>,epoch:Uuid}
        impl Drop for Guard{fn drop(&mut self){let mut b=self.node.browser.lock().unwrap();if b.as_ref().is_some_and(|b|b.epoch==self.epoch){*b=None;}drop(b);self.node.browser_pending.lock().unwrap().retain(|_,p|p.epoch!=self.epoch);self.node.log("warn","browser","Local extension disconnected",self.epoch);}}
        let _guard=Guard{node:node.clone(),epoch};
        ws.send(Message::Text(json!({"type":"ready","token":token,"node":node.cfg().node_name,"session":epoch}).to_string().into())).await.map_err(BridgeError::io)?;
        node.log("info","browser","Local extension authenticated",epoch);
        let mut last=Instant::now();let mut tick=tokio::time::interval(Duration::from_secs(20));
        loop{tokio::select!{
            _=node.shutdown.cancelled()=>return Ok(()),_=stop.cancelled()=>return Ok(()),
            _=tick.tick()=>{if last.elapsed()>Duration::from_secs(60){return Err(BridgeError::new("BROWSER_DISCONNECTED","Extension heartbeat timed out"))}ws.send(Message::Text(json!({"type":"ping"}).to_string().into())).await.map_err(BridgeError::io)?;},
            Some(v)=rx.recv()=>{ws.send(Message::Text(v.to_string().into())).await.map_err(BridgeError::io)?;},
            incoming=ws.recv()=>{
                let msg=incoming.ok_or_else(||BridgeError::new("BROWSER_DISCONNECTED","Extension socket closed"))?.map_err(BridgeError::io)?;last=Instant::now();
                let event=match msg{
                    Message::Text(t)=>{
                        let v:Value=serde_json::from_str(t.as_str())?;
                        if v["type"]=="ping"{
                            ws.send(Message::Text(json!({"type":"pong"}).to_string().into())).await.map_err(BridgeError::io)?;
                            continue;
                        }
                        if v["type"]=="pong"{continue}
                        BrowserEvent::Meta(v)
                    },
                    Message::Binary(b)=>{if b.len()<4{return Err(BridgeError::invalid("Truncated browser frame"))}let n=u32::from_be_bytes(b[..4].try_into().unwrap())as usize;if n>4096||n+4>b.len()||b.len()-4-n>65536{return Err(BridgeError::invalid("Oversized browser frame"))}BrowserEvent::Chunk(serde_json::from_slice(&b[4..4+n])?,Bytes::copy_from_slice(&b[4+n..]))},
                    Message::Close(_)=>return Ok(()),Message::Ping(b)=>{ws.send(Message::Pong(b)).await.map_err(BridgeError::io)?;continue},Message::Pong(_)=>continue,
                };
                let meta=match &event{BrowserEvent::Meta(v)=>v,BrowserEvent::Chunk(v,_)=>v};
                let id=Uuid::parse_str(meta["id"].as_str().unwrap_or("")).map_err(|_|BridgeError::invalid("Missing browser task ID"))?;
                let sender={node.browser_pending.lock().unwrap().get(&id).filter(|p|p.epoch==epoch).map(|p|p.tx.clone())};
                if let Some(sender)=sender{sender.try_send(event).map_err(|_|BridgeError::new("BUSY","Browser exceeded task receive window"))?;}
            }
        }}
    }.await;
    if let Err(e) = result {
        node.log("warn", "browser", e.code.clone(), "");
        let _ = ws
            .send(Message::Text(
                json!({"type":"error","error":e}).to_string().into(),
            ))
            .await;
    }
    let _ = ws.close().await;
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extension_origin_is_strict() {
        assert!(valid_origin(
            "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ));
        for s in [
            "https://evil.test",
            "chrome-extension://abc",
            "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaZ",
            "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/",
        ] {
            assert!(!valid_origin(s))
        }
    }
    #[test]
    fn token_check_rejects_empty_and_different_strings() {
        assert!(token_equal("abcd", "abcd"));
        assert!(!token_equal("abcd", "abce"));
        assert!(!token_equal("abcd", ""));
    }
}
