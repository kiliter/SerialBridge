use crate::{config::Config,error::{BridgeError,Result},node::{Node,Reply}};
use base64::Engine;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json,Value};
use std::{collections::BTreeMap,sync::Arc,time::Duration};
#[derive(Debug,Clone,Deserialize)]#[serde(deny_unknown_fields)]
pub struct FetchSpec {pub url:String,#[serde(default="default_method")]pub method:String,#[serde(default)]pub headers:BTreeMap<String,String>,#[serde(default)]pub body:Option<String>}
fn default_method()->String{"GET".into()}
pub fn validate(config:&Config,params:&Value)->Result<FetchSpec>{
    let mut spec:FetchSpec=serde_json::from_value(params.clone())?;spec.method=spec.method.to_ascii_uppercase();
    let url=reqwest::Url::parse(&spec.url).map_err(|_|BridgeError::invalid("Invalid request URL"))?;
    if !matches!(url.scheme(),"http"|"https")||!url.username().is_empty()||url.password().is_some()||!config.allowed_origins.contains(&url.origin().ascii_serialization()){return Err(BridgeError::new("ORIGIN_NOT_ALLOWED","Target origin is not explicitly allowed on the executing node"))}
    if !config.allowed_methods.contains(&spec.method){return Err(BridgeError::new("METHOD_NOT_ALLOWED","Method is not allowed on the executing node"))}
    if spec.headers.len()>32{return Err(BridgeError::invalid("Too many headers"))}
    for(k,v)in &spec.headers{let key=k.to_ascii_lowercase();if ["cookie","host","origin","referer","content-length","connection","set-cookie","transfer-encoding"].contains(&key.as_str())||key.starts_with("sec-")||key.starts_with("proxy-"){return Err(BridgeError::new("FORBIDDEN","Header is not permitted"))}if k.len()>128||v.len()>4096||(k.contains('\r')||k.contains('\n'))||(v.contains('\r')||v.contains('\n')){return Err(BridgeError::invalid("Invalid header"))}}
    if spec.body.as_ref().is_some_and(|b|b.len()>8192){return Err(BridgeError::invalid("Request body limit is 8 KiB"))}
    if matches!(spec.method.as_str(),"GET"|"HEAD")&&spec.body.as_ref().is_some_and(|b|!b.is_empty()){return Err(BridgeError::invalid("GET/HEAD cannot contain a body"))}
    Ok(spec)
}
pub async fn response(node:&Arc<Node>,params:&Value)->Result<reqwest::Response>{
    let spec=validate(&node.cfg(),params)?;let method=reqwest::Method::from_bytes(spec.method.as_bytes()).map_err(BridgeError::io)?;
    let mut req=node.client.request(method,&spec.url).timeout(Duration::from_secs(300));
    for(k,v)in &spec.headers{req=req.header(k.as_str(),v.as_str());}if let Some(body)=spec.body{req=req.body(body)}
    let res=req.send().await.map_err(|_|BridgeError::new("HTTP_ERROR","HTTP request failed (DNS, TLS, connection or timeout); credentials/URL are not logged"))?;
    if res.status().is_redirection(){return Err(BridgeError::new("REDIRECT_NOT_ALLOWED","Alpha requires the final URL; redirects are not followed"))}Ok(res)
}
pub async fn fetch(node:&Arc<Node>,params:Value)->Result<Reply>{
    let res=response(node,&params).await?;
    if res.content_length().is_some_and(|n|n>16384){return Err(BridgeError::new("RESPONSE_TOO_LARGE","Use the downloads API for responses over 16 KiB"))}
    let status=res.status().as_u16();let headers:BTreeMap<String,String>=res.headers().iter().filter(|(k,_)|k.as_str()!="set-cookie").filter_map(|(k,v)|v.to_str().ok().map(|s|(k.to_string(),s.to_string()))).collect();
    let mut body=Vec::new();let mut stream=res.bytes_stream();while let Some(chunk)=stream.next().await{let chunk=chunk.map_err(|_|BridgeError::new("HTTP_ERROR","Response stream interrupted"))?;if body.len()+chunk.len()>16384{return Err(BridgeError::new("RESPONSE_TOO_LARGE","Use the downloads API"))}body.extend_from_slice(&chunk);}
    let value=match String::from_utf8(body.clone()){Ok(text)=>json!({"status":status,"headers":headers,"body":text,"body_encoding":"utf8"}),Err(_)=>json!({"status":status,"headers":headers,"body_base64":base64::engine::general_purpose::STANDARD.encode(body),"body_encoding":"base64"})};Ok(Reply::json(value))
}
#[cfg(test)]mod tests{use super::*;#[test]fn policy_rejects_suffix_and_credential_headers(){let mut c=Config::default();c.allowed_origins=vec!["https://example.com".into()];assert!(validate(&c,&json!({"url":"https://example.com/api"})).is_ok());assert!(validate(&c,&json!({"url":"https://example.com.evil"})).is_err());assert!(validate(&c,&json!({"url":"https://example.com","headers":{"Cookie":"secret"}})).is_err());}}
