/// WebSocket JSON message types for the companion protocol.
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    #[serde(rename = "hello")]
    Hello {
        version: u32,
        origin: String,
    },
    #[serde(rename = "ctap_relay")]
    CtapRelay {
        id: String,
        command: u32,
        payload: String, // base64-encoded
    },
    #[serde(rename = "ctap_cancel")]
    CtapCancel {
        id: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "hello_ack")]
    HelloAck {
        version: u32,
        fido_available: bool,
        device_name: String,
    },
    #[serde(rename = "ctap_response")]
    CtapResponse {
        id: String,
        command: u32,
        payload: String, // base64-encoded
        error_code: u32,
    },
    #[serde(rename = "status")]
    Status {
        fido_available: bool,
        device_name: String,
    },
}
