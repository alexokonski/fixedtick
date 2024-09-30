use std::net::SocketAddr;
use std::time;
use bytes::Bytes;
use crate::networking::transport::SimLatencySettings;

pub struct Message {
    /// The destination to send the message.
    pub destination: SocketAddr,
    /// The serialized payload itself.
    pub payload: Bytes,
    // Optional send time
    //pub send_time: Option<time::Instant>,
}

impl Message {
    /// Creates and returns a new Message.
    pub(crate) fn new(destination: SocketAddr, payload: &[u8]/*, send_time: Option<time::Instant>*/) -> Self {
        Self {
            destination,
            payload: Bytes::copy_from_slice(payload),
            //send_time
        }
    }
}
