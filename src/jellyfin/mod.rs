//! Jellyfin API client module for interacting with Jellyfin media server

mod api;
mod auth;
mod models;
mod models_playback;
mod session;
mod websocket;
#[cfg(test)]
mod tests;

pub use api::*;
pub use websocket::WebSocketHandler;
pub use auth::*;
pub use models::*;
pub use models_playback::*;
pub use session::*;
