use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeError {
    pub code: String,
    pub message: String,
}
pub type Result<T> = std::result::Result<T, BridgeError>;
impl BridgeError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
    pub fn io(e: impl std::fmt::Display) -> Self {
        Self::new("IO_ERROR", e.to_string())
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("INVALID_REQUEST", message)
    }
}
impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for BridgeError {}
impl From<std::io::Error> for BridgeError {
    fn from(e: std::io::Error) -> Self {
        Self::io(e)
    }
}
impl From<serde_json::Error> for BridgeError {
    fn from(e: serde_json::Error) -> Self {
        Self::invalid(e.to_string())
    }
}
impl IntoResponse for BridgeError {
    fn into_response(self) -> Response {
        let status = match self.code.as_str() {
            "UNAUTHORIZED" => StatusCode::UNAUTHORIZED,
            "FORBIDDEN" | "ORIGIN_NOT_ALLOWED" | "METHOD_NOT_ALLOWED" => StatusCode::FORBIDDEN,
            "NOT_FOUND" => StatusCode::NOT_FOUND,
            "BUSY" => StatusCode::TOO_MANY_REQUESTS,
            "PEER_DISCONNECTED" | "BROWSER_DISCONNECTED" => StatusCode::SERVICE_UNAVAILABLE,
            "TIMEOUT" => StatusCode::GATEWAY_TIMEOUT,
            "IO_ERROR" | "HTTP_ERROR" => StatusCode::BAD_GATEWAY,
            _ => StatusCode::BAD_REQUEST,
        };
        (status, Json(serde_json::json!({"error":self}))).into_response()
    }
}
