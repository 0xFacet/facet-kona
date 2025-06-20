//! Native implementation of the [Channel] trait, backed by [async_channel]'s unbounded
//! channel primitives.

use crate::{
    Channel,
    errors::{ChannelError, ChannelResult},
};
use async_channel::{Receiver, Sender, unbounded};
use async_trait::async_trait;
use std::{
    collections::VecDeque,
    io::Result,
    sync::{Arc, Mutex},
};

/// A bidirectional channel, allowing for synchronized communication between two parties.
#[derive(Debug, Clone)]
pub struct BidirectionalChannel {
    /// The client handle of the channel.
    pub client: NativeChannel,
    /// The host handle of the channel.
    pub host: NativeChannel,
}

impl BidirectionalChannel {
    /// Creates a [BidirectionalChannel] instance.
    pub fn new() -> Result<Self> {
        let (bw, ar) = unbounded();
        let (aw, br) = unbounded();

        Ok(Self {
            client: NativeChannel { read: ar, write: aw, read_buffer: Arc::new(Mutex::new(VecDeque::new())) },
            host: NativeChannel { read: br, write: bw, read_buffer: Arc::new(Mutex::new(VecDeque::new())) },
        })
    }
}

/// A channel with a receiver and sender.
#[derive(Debug, Clone)]
pub struct NativeChannel {
    /// The receiver of the channel.
    pub(crate) read: Receiver<Vec<u8>>,
    /// The sender of the channel.
    pub(crate) write: Sender<Vec<u8>>,
    /// Buffer for accumulating partial reads
    read_buffer: Arc<Mutex<VecDeque<u8>>>,
}

#[async_trait]
impl Channel for NativeChannel {
    async fn read(&self, buf: &mut [u8]) -> ChannelResult<usize> {
        // Check if we have buffered data first
        {
            let mut read_buffer = self.read_buffer.lock().unwrap();
            if !read_buffer.is_empty() {
                let len = buf.len().min(read_buffer.len());
                for i in 0..len {
                    buf[i] = read_buffer.pop_front().unwrap();
                }
                return Ok(len);
            }
        } // Drop lock before await
        
        // Otherwise, receive new data
        let data = self.read.recv().await.map_err(|_| ChannelError::Closed)?;
        let len = data.len().min(buf.len());
        buf[..len].copy_from_slice(&data[..len]);
        
        // If we received more than requested, buffer the rest
        if data.len() > buf.len() {
            let mut read_buffer = self.read_buffer.lock().unwrap();
            read_buffer.extend(&data[buf.len()..]);
        }
        
        Ok(len)
    }

    async fn read_exact(&self, buf: &mut [u8]) -> ChannelResult<usize> {
        let mut total_read = 0;
        
        while total_read < buf.len() {
            // First, drain any buffered data
            {
                let mut read_buffer = self.read_buffer.lock().unwrap();
                while !read_buffer.is_empty() && total_read < buf.len() {
                    buf[total_read] = read_buffer.pop_front().unwrap();
                    total_read += 1;
                }
            } // Drop lock before await
            
            // If we still need more data, receive from channel
            if total_read < buf.len() {
                let data = self.read.recv().await.map_err(|_| ChannelError::Closed)?;
                let remaining = buf.len() - total_read;
                let to_copy = data.len().min(remaining);
                
                buf[total_read..total_read + to_copy].copy_from_slice(&data[..to_copy]);
                total_read += to_copy;
                
                // Buffer any excess data
                if data.len() > to_copy {
                    let mut read_buffer = self.read_buffer.lock().unwrap();
                    read_buffer.extend(&data[to_copy..]);
                }
            }
        }
        
        Ok(total_read)
    }

    async fn write(&self, buf: &[u8]) -> ChannelResult<usize> {
        self.write.send(buf.to_vec()).await.map_err(|_| ChannelError::Closed)?;
        Ok(buf.len())
    }
}
