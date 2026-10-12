pub mod api;pub mod config;pub mod error;pub mod extension;pub mod http_executor;pub mod node;pub mod protocol;pub mod transfer;pub mod transport;
use std::{path::PathBuf,sync::Arc,time::Duration};
use error::Result;
use node::Node;
/// Starts one loopback HTTP listener and one optional serial peer link.
/// The caller retains the node and cancels node.shutdown before exiting.
pub async fn start(path:PathBuf,transport:transport::TransportMode)->Result<Arc<Node>>{
    let cfg=config::Config::load(&path)?;
    let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,cfg.http_port)).await?;
    let node=Node::new(cfg,path)?;
    let server_node=node.clone();let stop=node.shutdown.clone();
    tokio::spawn(async move{if let Err(e)=axum::serve(listener,api::router(server_node.clone())).with_graceful_shutdown(stop.cancelled_owned()).await{server_node.log("error","http",e.to_string(),"");server_node.shutdown.cancel();}});
    tokio::spawn(transport::run(node.clone(),transport));
    let clean=node.clone();tokio::spawn(async move{loop{tokio::select!{_=clean.shutdown.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(60))=>clean.reap_uploads().await}}});
    node.log("info","http",format!("HTTP listening on 127.0.0.1:{}",node.cfg().http_port),"");Ok(node)
}
