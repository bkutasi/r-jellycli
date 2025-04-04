//! Jellyfin API client module for interacting with Jellyfin media server

pub mod api;
mod auth;
pub mod models;
pub mod models_playback;
pub mod session;
pub mod websocket;
mod ws_incoming_handler;
pub mod reporter;
#[cfg(test)]
mod tests;

pub use api::*;
pub use websocket::WebSocketHandler;
pub use auth::*;
pub use models::*;
pub use models_playback::*;
