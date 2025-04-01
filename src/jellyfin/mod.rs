//! Jellyfin API client module for interacting with Jellyfin media server

pub mod api;
mod auth;
pub mod models;
mod models_playback;
mod session;
pub mod websocket;
#[cfg(test)]
mod tests;

pub use api::*;
pub use websocket::WebSocketHandler;
pub use auth::*;
pub use models::*;
pub use models_playback::*;
pub use session::*;
