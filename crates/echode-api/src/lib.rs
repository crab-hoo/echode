//! JSON-RPC 2.0 for one local project session.
//!
//! Each request is one JSON object on its own line. The socket is readable
//! and writable only by the current user.

mod rpc;
mod serve;

pub use rpc::ApiSession;
pub use serve::{ApiError, bind_socket, serve, serve_connection};
