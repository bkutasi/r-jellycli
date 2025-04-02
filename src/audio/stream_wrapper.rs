use crate::audio::AudioError;

use bytes::Bytes;
use futures_util::StreamExt;
use tracing::{debug, info, trace, error}; // Added error
use std::io::{self, Read, Seek, SeekFrom}; // Removed Cursor
use symphonia::core::io::MediaSource;

const LOG_TARGET: &str = "r_jellycli::audio::stream_wrapper";

/// Wraps a Reqwest byte stream, pre-downloading all content into memory
/// to satisfy Symphonia's `Read + Seek` requirement for `MediaSource`.
///
/// **Warning:** This defeats true streaming playback and can consume significant
/// memory for large files. A future improvement would be a true streaming adapter.
pub struct ReqwestStreamWrapper {
    buffer: Vec<u8>,      // Store buffer directly
    position: u64,        // Manual position tracking
}

impl ReqwestStreamWrapper {
    /// Creates a new wrapper by asynchronously downloading the entire stream content.
    /// Returns an io::Error if the stream is empty after download.
    pub async fn new_async( // Changed return type
        stream: impl futures_util::Stream<Item = Result<Bytes, reqwest::Error>> + Send + Unpin + 'static,
    ) -> Result<Self, io::Error> { // Changed return type
        let mut buffer = Vec::new();
        let mut stream = stream; // Shadowing is fine here

        debug!(target: LOG_TARGET, "Pre-downloading audio stream into memory...");
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Error reading stream chunk: {}", e)))?;
            buffer.extend_from_slice(&chunk);
            trace!(target: LOG_TARGET, "Downloaded {} bytes (total {})", chunk.len(), buffer.len());
        }
        // Check if any data was actually downloaded
        if buffer.is_empty() {
            error!(target: LOG_TARGET, "Pre-download complete but buffer is empty. Stream likely closed prematurely.");
            // Return a standard io::Error
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Downloaded stream was empty"));
        }
        info!(target: LOG_TARGET, "Pre-download complete ({} bytes). Creating buffered source.", buffer.len());

        Ok(Self {
            buffer,
            position: 0, // Initialize position
        })
    }
}

impl Read for ReqwestStreamWrapper {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let current_pos = self.position as usize;
        if current_pos >= self.buffer.len() {
            return Ok(0); // EOF
        }
        let remaining = self.buffer.len() - current_pos;
        let can_read = std::cmp::min(buf.len(), remaining);
        if can_read > 0 {
            buf[..can_read].copy_from_slice(&self.buffer[current_pos..current_pos + can_read]);
            self.position += can_read as u64;
        }
        trace!(target: LOG_TARGET, "Manual Read: requested={}, read={}, pos_before={}, pos_after={}", buf.len(), can_read, current_pos, self.position);
        Ok(can_read)
    }
}

impl Seek for ReqwestStreamWrapper {
    fn seek(&mut self, style: SeekFrom) -> io::Result<u64> {
        let pos_before = self.position;
        let buffer_len = self.buffer.len() as u64;

        let new_pos = match style {
            SeekFrom::Start(pos) => pos,
            SeekFrom::End(pos) => {
                // Handle potential overflow/underflow with i64 for offset calculation
                let len_i64 = buffer_len as i64;
                let new_pos_signed = len_i64.checked_add(pos).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek offset overflow"))?;
                if new_pos_signed < 0 {
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before start"));
                }
                new_pos_signed as u64
            }
            SeekFrom::Current(pos) => {
                let current_pos_i64 = self.position as i64;
                let new_pos_signed = current_pos_i64.checked_add(pos).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek offset overflow"))?;
                 if new_pos_signed < 0 {
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before start"));
                }
                new_pos_signed as u64
            }
        };

        // Allow seeking exactly to the end, but not beyond for reads.
        // Seeking beyond might be valid depending on interpretation, but let's cap it for simplicity.
        self.position = std::cmp::min(new_pos, buffer_len);

        trace!(target: LOG_TARGET, "Manual Seek: requested={:?}, result={:?}, pos_before={}, pos_after={}", style, Ok::<u64, AudioError>(self.position), pos_before, self.position);
        Ok(self.position)
    }
}

impl MediaSource for ReqwestStreamWrapper {
    fn is_seekable(&self) -> bool {
        true // We now support seeking manually
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.buffer.len() as u64) // Get length directly from Vec
    }
}

// io::Cursor<Vec<u8>> is Send + Sync, so ReqwestStreamWrapper is too.
// No need for unsafe impls.