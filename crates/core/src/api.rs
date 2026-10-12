use crate::{error::{BridgeError,Result},extension::{serve_socket,token_equal,valid_origin},node::{Node,RpcRequest}};
use axum::{body::Body,extract::{DefaultBodyLimit,Path,Request,State,WebSocketUpgrade},http::{HeaderMap,HeaderValue,Method,StatusCode},middleware::{self,Next},response::{IntoResponse,Response},routing::{get,post,put},Json,Router};
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{json,Value};
use std::{sync::Arc,time::Duration};
use tokio_util::io::ReaderStream;
use uuid::Uuid;
pub fn router(node:Arc<Node>)->Router{
    let protected=Router::new()
        .route("/v1/health",get(health)).route("/v1/ports",get(ports))
        .route("/v1/logs",get(logs)).route("/v1/settings",get(settings).put(save_settings))
        .route("/v1/rpc",post(rpc)).route("/v1/serial/reconnect",post(reconnect))
        .route("/v1/extension/pairing",post(pair).delete(revoke))
        .route("/v1/downloads",get(transfers).post(download))
        .route("/v1/downloads/{id}",get(transfer).delete(cancel))
        .route("/v1/downloads/{id}/content",get(content))
        .route("/v1/files/pull",post(pull))
        .layer(DefaultBodyLimit::max(128*1024))
        .route("/v1/files/push/{filename}",put(push).layer(DefaultBodyLimit::disable()))
        .layer(middleware::from_fn_with_state(node.clone(),auth));
    protected.route("/v1/extension/ws",get(extension_ws)).route("/healthz",get(||async{Json(json!({"service":"SerialBridge"}))})).with_state(node)
}
async fn auth(State(node):State<Arc<Node>>,request:Request,next:Next)->Response{
    let origin=request.headers().get("origin").and_then(|v|v.to_str().ok()).map(str::to_owned);
    let allowed_origin=origin.as_deref().is_some_and(|s|["http://127.0.0.1:4173","http://localhost:4173"].contains(&s));
    if origin.is_some()&&!allowed_origin{return BridgeError::new("FORBIDDEN","Untrusted web origin").into_response()}
    let mut response=if request.method()==Method::OPTIONS {
        if !allowed_origin{return BridgeError::new("FORBIDDEN","No allowed preflight origin").into_response()}
        StatusCode::NO_CONTENT.into_response()
    } else {
        let token=request.headers().get("authorization").and_then(|v|v.to_str().ok()).and_then(|v|v.strip_prefix("Bearer "));
        if !token.is_some_and(|t|token_equal(t,&node.cfg().api_token)){BridgeError::new("UNAUTHORIZED","A valid local API token is required").into_response()}else{next.run(request).await}
    };
    if allowed_origin {
        let h=response.headers_mut();h.insert("access-control-allow-origin",HeaderValue::from_str(origin.as_ref().unwrap()).unwrap());h.insert("vary",HeaderValue::from_static("Origin"));h.insert("access-control-allow-methods",HeaderValue::from_static("GET, POST, PUT, DELETE, OPTIONS"));h.insert("access-control-allow-headers",HeaderValue::from_static("Content-Type, Authorization"));
    }
    response
}
async fn health(State(n):State<Arc<Node>>)->Json<Value>{Json(n.snapshot())}
async fn settings(State(n):State<Arc<Node>>)->Json<Value>{Json(n.cfg().public())}
async fn save_settings(State(n):State<Arc<Node>>,Json(p):Json<Value>)->Result<Json<Value>>{Ok(Json(n.update_settings(p)?))}
async fn logs(State(n):State<Arc<Node>>)->Json<Value>{Json(json!({"logs":n.logs.lock().unwrap().iter().cloned().collect::<Vec<_>>()}))}
async fn ports()->Result<Json<Value>>{
    let ports=tokio_serial::available_ports().map_err(BridgeError::io)?.into_iter().map(|p|{
        match p.port_type{tokio_serial::SerialPortType::UsbPort(u)=>json!({"name":p.port_name,"kind":"usb","vid":u.vid,"pid":u.pid,"serial_number":u.serial_number,"product":u.product}),other=>json!({"name":p.port_name,"kind":format!("{other:?}")})}
    }).collect::<Vec<_>>();Ok(Json(json!({"ports":ports})))
}
async fn rpc(State(n):State<Arc<Node>>,Json(r):Json<RpcRequest>)->Result<Json<Value>>{
    // Internal file blocks use dedicated endpoints / peer protocol, not JSON byte arrays.
    if !["system.ping","http.fetch","browser.fetch"].contains(&r.method.as_str()){return Err(BridgeError::new("METHOD_NOT_FOUND","Public RPC methods: system.ping, http.fetch, browser.fetch; use file/download endpoints for transfers"))}
    let id=Uuid::new_v4();let reply=n.call(&r.target,&r.method,r.params,Bytes::new(),r.timeout_ms).await;
    match reply{Ok(r)=>{n.log("info","rpc","Request complete",id);Ok(Json(json!({"id":id,"result":r.value})))},Err(e)=>{n.log("error","rpc",e.code.clone(),id);Err(e)}}
}
async fn reconnect(State(n):State<Arc<Node>>)->Json<Value>{n.reconnect.notify_one();Json(json!({"reconnecting":true}))}
async fn pair(State(n):State<Arc<Node>>)->Json<Value>{Json(n.new_pair_code())}
async fn revoke(State(n):State<Arc<Node>>)->Result<Json<Value>>{n.revoke_browser()?;Ok(Json(json!({"revoked":true})))}
async fn extension_ws(State(n):State<Arc<Node>>,headers:HeaderMap,ws:WebSocketUpgrade)->Result<Response>{
    let origin=headers.get("origin").and_then(|v|v.to_str().ok()).unwrap_or("").to_string();if !valid_origin(&origin){return Err(BridgeError::new("UNAUTHORIZED","Only a Chromium extension Origin can open this socket"))}
    Ok(ws.max_message_size(128*1024).max_frame_size(128*1024).on_upgrade(move|socket|serve_socket(n,socket,origin)))
}
async fn transfers(State(n):State<Arc<Node>>)->Json<Value>{Json(json!({"transfers":n.transfers.list()}))}
async fn download(State(n):State<Arc<Node>>,Json(p):Json<Value>)->Result<(StatusCode,Json<Value>)>{Ok((StatusCode::ACCEPTED,Json(n.start_download(p)?)))}
async fn pull(State(n):State<Arc<Node>>,Json(p):Json<Value>)->Result<(StatusCode,Json<Value>)>{Ok((StatusCode::ACCEPTED,Json(n.start_pull(p)?)))}
async fn transfer(State(n):State<Arc<Node>>,Path(id):Path<Uuid>)->Result<Json<Value>>{Ok(Json(serde_json::to_value(n.transfers.get(id)?)?))}
async fn cancel(State(n):State<Arc<Node>>,Path(id):Path<Uuid>)->Result<Json<Value>>{
    let view=n.transfers.get(id)?;
    if ["completed","failed","cancelled"].contains(&view.status.as_str()){n.remove_artifact(json!({"id":id})).await?;Ok(Json(json!({"removed":true})))}else{n.transfers.cancel(id)?;Ok(Json(json!({"cancelling":true})))}
}
async fn content(State(n):State<Arc<Node>>,Path(id):Path<Uuid>)->Result<Response>{
    let a=n.transfers.artifact(id)?;let file=tokio::fs::File::open(a.path).await?;
    let escaped=a.filename.bytes().map(|b|if b.is_ascii_alphanumeric()||b"._-".contains(&b){(b as char).to_string()}else{format!("%{b:02X}")}).collect::<String>();
    let mut response=Body::from_stream(ReaderStream::new(file)).into_response();
    response.headers_mut().insert("content-type",HeaderValue::from_static("application/octet-stream"));response.headers_mut().insert("content-length",HeaderValue::from_str(&a.size.to_string()).map_err(BridgeError::io)?);
    response.headers_mut().insert("content-disposition",HeaderValue::from_str(&format!("attachment; filename=\"download.bin\"; filename*=UTF-8''{escaped}")).map_err(BridgeError::io)?);
    Ok(response)
}
async fn push(State(n):State<Arc<Node>>,Path(filename):Path<String>,headers:HeaderMap,body:Body)->Result<(StatusCode,Json<Value>)>{
    let total=headers.get("content-length").and_then(|s|s.to_str().ok()).and_then(|s|s.parse::<u64>().ok()).ok_or_else(||BridgeError::invalid("File push requires Content-Length"))?;
    let upload=n.create_upload(json!({"filename":filename,"total_bytes":total,"local_staging":true})).await?;
    let id=Uuid::parse_str(upload.value["id"].as_str().unwrap_or("")).map_err(BridgeError::io)?;
    let result=async{
        let mut stream=body.into_data_stream();let mut offset=0u64;
        loop{let next=tokio::time::timeout(Duration::from_secs(60),stream.next()).await.map_err(|_|BridgeError::new("TIMEOUT","HTTP upload stalled"))?;let Some(chunk)=next else{break};let chunk=chunk.map_err(BridgeError::io)?;
            if offset+chunk.len()as u64>total{return Err(BridgeError::invalid("HTTP upload exceeds declared length"))}
            for data in chunk.chunks(n.cfg().chunk_bytes){n.write_block(json!({"id":id,"offset":offset}),Bytes::copy_from_slice(data)).await?;offset+=data.len()as u64;}
        }
        n.finish_upload(json!({"id":id})).await?;n.start_push(id)
    }.await;
    match result{Ok(v)=>Ok((StatusCode::ACCEPTED,Json(v))),Err(e)=>{let _=n.remove_artifact(json!({"id":id})).await;Err(e)}}
}
