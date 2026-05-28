use std::io::{Read, Write};
use std::net::TcpStream;

pub struct Conn {
    pub stream: TcpStream,

    // Intention for OS poll
    pub want_read: bool,
    pub want_write: bool,

    // Application buffers
    pub incoming: Vec<u8>,
    pub outgoing: Vec<u8>,
}

impl Conn {
    pub fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            want_read: true, // always read first request
            want_write: false,
            incoming: Vec::new(),
            outgoing: Vec::new(),
        }
    }

    pub fn try_read(&mut self) -> std::io::Result<usize> {
        let mut buf = [0u8; 4096];
        match self.stream.read(&mut buf) {
            Ok(0) => {
                // Connection closed
                Ok(0)
            }
            Ok(n) => {
                self.incoming.extend_from_slice(&buf[..n]);
                // For now, let's just echo it back directly for testing
                self.outgoing.extend_from_slice(&buf[..n]);
                self.want_read = false;
                self.want_write = true;
                Ok(n)
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(0),
            Err(e) => Err(e),
        }
    }

    pub fn try_write(&mut self) -> std::io::Result<usize> {
        if self.outgoing.is_empty() {
            self.want_read = true;
            self.want_write = false;
            return Ok(0);
        }

        match self.stream.write(&self.outgoing) {
            Ok(n) => {
                self.outgoing.drain(..n);
                if self.outgoing.is_empty() {
                    self.want_write = false;
                    self.want_read = true;
                }
                Ok(n)
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(0),
            Err(e) => Err(e),
        }
    }
}
