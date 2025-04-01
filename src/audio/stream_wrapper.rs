use bytes::Bytes;
use futures_util::StreamExt;
use log::{debug, info, trace};
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use symphonia::core::io::MediaSource;

const LOG_TARGET: &str = "r_jellycli::audio::stream_wrapper";

/// Wraps a Reqwest byte stream, pre-downloading all content into memory
/// to satisfy Symphonia's `Read + Seek` requirement for `MediaSource`.
///
/// **Warning:** This defeats true streaming playback and can consume significant
/// memory for large files. A future improvement would be a true streaming adapter.
pub struct ReqwestStreamWrapper {
    buffer: Cursor<Vec<u8>>, // Use Cursor for Read + Seek on Vec<u8>
}

impl ReqwestStreamWrapper {
    /// Creates a new wrapper by asynchronously downloading the entire stream content.
    pub async fn new_async(
        stream: impl futures_util::Stream<Item = Result<Bytes, reqwest::Error>> + Send + Unpin + 'static,
    ) -> Result<Self, reqwest::Error> {
        let mut buffer = Vec::new();
        let mut stream = stream; // Shadowing is fine here

        debug!(target: LOG_TARGET, "Pre-downloading audio stream into memory...");
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result?;
            buffer.extend_from_slice(&chunk);
            trace!(target: LOG_TARGET, "Downloaded {} bytes (total {})", chunk.len(), buffer.len());
        }
        info!(target: LOG_TARGET, "Pre-download complete ({} bytes). Creating buffered source.", buffer.len());

        Ok(Self {
            buffer: Cursor::new(buffer),
        })
    }
}

impl Read for ReqwestStreamWrapper {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.buffer.read(buf)
    }
}

impl Seek for ReqwestStreamWrapper {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.buffer.seek(pos)
    }
}

impl MediaSource for ReqwestStreamWrapper {
    fn is_seekable(&self) -> bool {
        true // The buffer itself is seekable
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.buffer.get_ref().len() as u64)
    }
}

// io::Cursor<Vec<u8>> is Send + Sync, so ReqwestStreamWrapper is too.
// No need for unsafe impls.