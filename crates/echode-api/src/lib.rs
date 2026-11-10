//! JSON-RPC 2.0 for one local project session.
//!
//! Each request is one JSON object on its own line. The socket is readable
//! and writable only by the current user.

mod error;
mod mcp;
mod rpc;
mod serve;

pub use error::{ApiError, ApiIoOperation};
pub use mcp::serve_mcp;
pub use rpc::ApiSession;
pub use serve::{bind_socket, serve, serve_connection};
