//! Per-connection TCP state and non-blocking I/O helpers.
//!
//! [`Conn`] owns the socket-facing state for one client. The event loop uses
//! its readiness flags to decide whether to poll for input, output, or both.

use std::io;
use std::io::{Read, Write};
use std::net::TcpStream;

use crate::buffer::Buffer;

const DEFAULT_HIGH_WATERMARK: usize = 64 * 1024;
const DEFAULT_LOW_WATERMARK: usize = 32 * 1024;

#[derive(Debug)]
/// Tracks one client socket plus the buffers and poll interests associated with it.
pub struct Conn {
    /// The live TCP stream for this client.
    pub stream: TcpStream,

    /// Whether the event loop should ask the OS for read readiness on this socket.
    pub want_read: bool,
    /// Whether the event loop should ask the OS for write readiness on this socket.
    pub want_write: bool,
    /// Whether the peer has closed its side of the connection.
    pub peer_closed: bool,

    /// Bytes received from the client that the application has buffered.
    pub incoming: Buffer,
    /// Bytes queued to be written back to the client.
    pub outgoing: Buffer,

    high_watermark: usize,
    low_watermark: usize,
}

impl Conn {
    /// Creates a new connection in read-first mode.
    ///
    /// A new connection has no buffered input or output, so it initially
    /// requests read readiness only.
    pub fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            want_read: true,   // always read first request
            want_write: false, // nothing to write on start
            peer_closed: false,
            incoming: Buffer::new(),
            outgoing: Buffer::new(),
            high_watermark: DEFAULT_HIGH_WATERMARK,
            low_watermark: DEFAULT_LOW_WATERMARK,
        }
    }

    #[cfg(test)]
    fn with_watermarks(stream: TcpStream, high: usize, low: usize) -> Self {
        Self {
            stream,
            want_read: true,   // always read first request
            want_write: false, // nothing to write on start
            peer_closed: false,
            incoming: Buffer::new(),
            outgoing: Buffer::new(),
            high_watermark: high,
            low_watermark: low,
        }
    }

    /// Reads from the socket until EOF, an error, or `WouldBlock`.
    ///
    /// Every successful read is appended to [`Conn::incoming`]. Reaching EOF
    /// sets [`Conn::peer_closed`] but does not discard bytes already buffered.
    pub fn try_read(&mut self) -> io::Result<()> {
        let mut buf = [0u8; 4096];

        loop {
            match self.stream.read(&mut buf) {
                Ok(0) => {
                    // Connection closed
                    self.peer_closed = true;

                    // There could still be data in the buffers
                    // Return because there's nothing to read
                    // let the caller deal with flushing the remaining data
                    return Ok({});
                }
                Ok(n) => {
                    // Append data to the buffer
                    self.incoming.append(&buf[..n]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    return Ok({});
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Reads the four-byte big-endian payload length from the buffer head.
    ///
    /// Returns `None` when fewer than four bytes are available. The buffer is
    /// not modified.
    fn parse_request_length(buf: &Buffer) -> Option<usize> {
        let header: [u8; 4] = buf.as_slice().get(..4)?.try_into().ok()?;
        Some(u32::from_be_bytes(header) as usize)
    }

    /// Returns whether parsing another complete request is allowed.
    ///
    /// Request parsing pauses when queued output reaches the high watermark.
    /// Writing output below the low watermark re-enables reading.
    pub fn can_process_requests(&self) -> bool {
        self.outgoing.len() < self.high_watermark
    }

    /// Attempts to extract one complete length-prefixed request.
    ///
    /// Returns `None` without changing the input buffer when the header or
    /// payload is incomplete. On success, consumes the four-byte header and
    /// payload and returns an owned copy of the payload.
    pub fn try_parse_one_request(&mut self) -> Option<Vec<u8>> {
        if self.incoming.len() < 4 {
            return None;
        }

        // TODO: eventually, we may want to account for potential malformed request length headers
        let len = Self::parse_request_length(&self.incoming)?;
        let len_with_header = 4usize.checked_add(len)?;

        // Check to ensure that full message exists and can be read
        if self.incoming.len() < len_with_header {
            return None;
        }

        self.incoming.consume(4);
        self.incoming.take(len)
    }

    /// Appends a response to the outgoing queue and requests write readiness.
    pub fn queue_response(&mut self, data: Vec<u8>) {
        self.outgoing.append(data.as_slice());

        if self.outgoing.len() >= self.high_watermark {
            self.want_read = false;
        }
        self.want_write = true;
    }

    /// Attempts one non-blocking write from the outgoing buffer.
    ///
    /// A partial write consumes only the bytes accepted by the socket. If
    /// bytes remain queued, the connection continues requesting write
    /// readiness for a later event-loop iteration.
    pub fn try_write(&mut self) -> io::Result<()> {
        if self.outgoing.is_empty() {
            self.want_read = true;
            self.want_write = false;
            return Ok(());
        }

        match self.stream.write(&self.outgoing.as_slice()) {
            Ok(n) => {
                self.outgoing.consume(n);

                if self.outgoing.is_empty() {
                    self.want_write = false;
                }

                if self.outgoing.len() <= self.low_watermark {
                    self.want_read = true;
                }

                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Returns whether the connection can be removed without losing data.
    ///
    /// The peer must have closed and both input and output buffers must be
    /// empty.
    pub fn ready_for_close(&self) -> bool {
        self.peer_closed && self.incoming.is_empty() && self.outgoing.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::{Duration, Instant};

    // Builds a connected client/server socket pair for focused `Conn` tests.
    fn setup_test_connection() -> (Conn, TcpStream) {
        // Bind to a random port on localhost
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        // Connect a client socket
        let client_stream = TcpStream::connect(addr).unwrap();
        // Accept the connection on the server side
        let (server_stream, _) = listener.accept().unwrap();

        // Make the server stream non-blocking just like the real app
        server_stream.set_nonblocking(true).unwrap();
        client_stream.set_nonblocking(false).unwrap(); // Client can block to make tests easier

        (Conn::new(server_stream), client_stream)
    }

    fn setup_test_connection_with_watermarks(high: usize, low: usize) -> (Conn, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client_stream = TcpStream::connect(addr).unwrap();
        let (server_stream, _) = listener.accept().unwrap();

        server_stream.set_nonblocking(true).unwrap();
        client_stream.set_nonblocking(false).unwrap();

        (
            Conn::with_watermarks(server_stream, high, low),
            client_stream,
        )
    }

    fn wait_until_incoming_len(conn: &mut Conn, expected_len: usize) {
        let deadline = Instant::now() + Duration::from_millis(100);

        while Instant::now() < deadline {
            conn.try_read().unwrap();
            if conn.incoming.len() == expected_len {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }

        panic!(
            "timed out waiting for incoming buffer to reach {expected_len} bytes; got {}",
            conn.incoming.len()
        );
    }

    #[test]
    fn test_conn_initial_state() {
        let (conn, _) = setup_test_connection();
        assert!(conn.want_read);
        assert!(!conn.want_write);
        assert!(!conn.peer_closed);
        assert!(conn.incoming.is_empty());
        assert!(conn.outgoing.is_empty());
    }

    #[test]
    fn test_try_read_would_block() {
        let (mut conn, _) = setup_test_connection();

        // Client hasn't sent anything
        // try_read returns without putting anything
        // into incoming because it hit WouldBlock
        assert_eq!(conn.try_read().unwrap(), ());
        assert!(conn.incoming.is_empty())
    }

    #[test]
    fn test_try_read_buffers_received_bytes() {
        let (mut conn, mut client) = setup_test_connection();

        client.write_all(b"ping").unwrap();
        wait_until_incoming_len(&mut conn, 4);
        assert_eq!(conn.incoming.take(4), Some(b"ping".to_vec()));
        assert!(!conn.peer_closed);
    }

    #[test]
    fn test_try_read_eof_marks_peer_closed() {
        let (mut conn, client) = setup_test_connection();
        drop(client);

        let deadline = Instant::now() + Duration::from_millis(100);
        while Instant::now() < deadline && !conn.peer_closed {
            conn.try_read().unwrap();
            if !conn.peer_closed {
                thread::sleep(Duration::from_millis(1));
            }
        }

        assert!(conn.peer_closed);
    }

    #[test]
    fn test_parse_request_waits_for_complete_header() {
        let (mut conn, _) = setup_test_connection();
        conn.incoming.append(&[0, 0]);

        assert_eq!(conn.try_parse_one_request(), None);
        assert_eq!(conn.incoming.as_slice(), &[0, 0]);
    }

    #[test]
    fn test_parse_request_waits_for_complete_payload() {
        let (mut conn, _) = setup_test_connection();
        conn.incoming.append(&[0, 0, 0, 5, b'h', b'e']);

        assert_eq!(conn.try_parse_one_request(), None);
        assert_eq!(conn.incoming.as_slice(), &[0, 0, 0, 5, b'h', b'e']);

        conn.incoming.append(b"llo");

        assert_eq!(conn.try_parse_one_request(), Some(b"hello".to_vec()));
        assert!(conn.incoming.is_empty());
    }

    #[test]
    fn test_parse_request_consumes_one_pipelined_request_at_a_time() {
        let (mut conn, _) = setup_test_connection();
        conn.incoming
            .append(&[0, 0, 0, 3, b'o', b'n', b'e', 0, 0, 0, 3, b't', b'w', b'o']);

        assert_eq!(conn.try_parse_one_request(), Some(b"one".to_vec()));
        assert_eq!(conn.try_parse_one_request(), Some(b"two".to_vec()));
        assert_eq!(conn.try_parse_one_request(), None);
        assert!(conn.incoming.is_empty());
    }

    #[test]
    fn test_parse_request_supports_empty_payload() {
        let (mut conn, _) = setup_test_connection();
        conn.incoming.append(&[0, 0, 0, 0]);

        assert_eq!(conn.try_parse_one_request(), Some(Vec::new()));
        assert!(conn.incoming.is_empty());
    }

    #[test]
    fn test_queue_response_appends_data_and_requests_writes() {
        let (mut conn, _) = setup_test_connection();

        conn.queue_response(b"one".to_vec());
        conn.queue_response(b"two".to_vec());

        assert_eq!(conn.outgoing.as_slice(), b"onetwo");
        assert!(conn.want_write);
    }

    #[test]
    fn test_queue_response_keeps_reading_below_high_watermark() {
        let (mut conn, _) = setup_test_connection_with_watermarks(8, 4);

        conn.queue_response(b"1234".to_vec());

        assert!(conn.want_read);
        assert!(conn.want_write);
        assert_eq!(conn.outgoing.len(), 4);
    }

    #[test]
    fn test_queue_response_disables_reading_at_high_watermark() {
        let (mut conn, _) = setup_test_connection_with_watermarks(8, 4);

        conn.queue_response(b"12345678".to_vec());

        assert!(!conn.want_read);
        assert!(conn.want_write);
        assert_eq!(conn.outgoing.len(), 8);
    }

    #[test]
    fn test_try_write_resumes_reading_at_or_below_low_watermark() {
        let (mut conn, mut client) = setup_test_connection_with_watermarks(8, 4);
        conn.queue_response(b"12345678".to_vec());
        assert!(!conn.want_read);

        conn.try_write().unwrap();

        let mut response = [0; 8];
        client.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"12345678");
        assert!(conn.want_read);
        assert!(!conn.want_write);
        assert!(conn.outgoing.is_empty());
    }

    #[test]
    fn test_default_watermarks_disable_reading_for_large_queued_response() {
        let (mut conn, _) = setup_test_connection();

        conn.queue_response(vec![0; DEFAULT_HIGH_WATERMARK]);

        assert!(!conn.want_read);
        assert!(conn.want_write);
    }

    #[test]
    fn test_try_write_flushes_response_and_restores_read_mode() {
        let (mut conn, mut client) = setup_test_connection();
        conn.queue_response(b"pong".to_vec());

        conn.try_write().unwrap();

        let mut response = [0u8; 4];
        client.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"pong");
        assert!(conn.outgoing.is_empty());
        assert!(conn.want_read);
        assert!(!conn.want_write);
    }

    #[test]
    fn test_try_write_with_empty_queue_restores_read_mode() {
        let (mut conn, _) = setup_test_connection();
        conn.want_read = false;
        conn.want_write = true;

        conn.try_write().unwrap();

        assert!(conn.want_read);
        assert!(!conn.want_write);
    }

    #[test]
    fn test_repeated_responses_do_not_grow_outgoing_storage_without_bound() {
        let (mut conn, mut client) = setup_test_connection();
        let response = b"pong".to_vec();

        for _ in 0..1024 {
            conn.queue_response(response.clone());
            conn.try_write().unwrap();

            let mut received = [0; 4];
            client.read_exact(&mut received).unwrap();
            assert_eq!(&received, b"pong");
        }

        assert!(conn.outgoing.is_empty());
        assert!(
            conn.outgoing.allocated_capacity() <= 64,
            "outgoing capacity grew to {} bytes",
            conn.outgoing.allocated_capacity()
        );
    }

    #[test]
    fn test_ready_for_close_requires_peer_closed_and_empty_buffers() {
        let (mut conn, _) = setup_test_connection();

        assert!(!conn.ready_for_close());

        conn.peer_closed = true;
        assert!(conn.ready_for_close());

        conn.incoming.append(b"ping");
        assert!(!conn.ready_for_close());

        assert_eq!(conn.incoming.take(4), Some(b"ping".to_vec()));
        conn.outgoing.append(b"pong");
        assert!(!conn.ready_for_close());
    }

    #[test]
    fn test_ready_for_close_preserves_buffered_data_after_peer_close() {
        let (mut conn, _) = setup_test_connection();

        conn.incoming.append(b"ping");
        conn.peer_closed = true;

        assert_eq!(conn.incoming.len(), 4);
        assert!(!conn.ready_for_close());
    }
}
