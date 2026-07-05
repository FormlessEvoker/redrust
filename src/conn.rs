//! Per-connection state and non-blocking read/write helpers.

use std::io::{Read, Write};
use std::net::TcpStream;

/// Tracks one client socket plus the buffers and poll interests associated with it.
pub struct Conn {
    /// The live TCP stream for this client.
    pub stream: TcpStream,

    /// Whether the event loop should ask the OS for read readiness on this socket.
    pub want_read: bool,
    /// Whether the event loop should ask the OS for write readiness on this socket.
    pub want_write: bool,

    /// Bytes received from the client that the application has buffered.
    pub incoming: Vec<u8>,
    /// Bytes queued to be written back to the client.
    pub outgoing: Vec<u8>,
}

impl Conn {
    /// Creates a new connection in "read first request" mode.
    pub fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            want_read: true, // always read first request
            want_write: false,
            incoming: Vec::new(),
            outgoing: Vec::new(),
        }
    }

    /// Attempts a single non-blocking read and updates buffers plus poll intent.
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
            Err(e) => Err(e),
        }
    }

    /// Attempts a single non-blocking write from the outgoing buffer.
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
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

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

    #[test]
    fn test_conn_initial_state() {
        let (conn, _) = setup_test_connection();
        assert!(conn.want_read);
        assert!(!conn.want_write);
        assert!(conn.incoming.is_empty());
        assert!(conn.outgoing.is_empty());
    }

    // Retries until the non-blocking server side has data ready to read.
    fn wait_for_read(conn: &mut Conn) -> usize {
        loop {
            match conn.try_read() {
                Ok(n) => return n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err(e) => panic!("Unexpected read error: {}", e),
            }
        }
    }

    #[test]
    fn test_try_read_would_block() {
        let (mut conn, _) = setup_test_connection();

        // Client hasn't sent anything, try_read should return a WouldBlock error
        let err = conn.try_read().unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
    }

    #[test]
    fn test_try_read_and_echo() {
        let (mut conn, mut client) = setup_test_connection();

        // Client writes data to server
        client.write_all(b"ping").unwrap();

        // Server reads it without blocking
        let n = wait_for_read(&mut conn);
        assert_eq!(n, 4);
        assert_eq!(conn.incoming, b"ping");
        assert_eq!(conn.outgoing, b"ping"); // Echo server logic

        // Intent flags should flip to allow write polling
        assert!(!conn.want_read);
        assert!(conn.want_write);

        // Server flushed write buffer to client
        let n = conn.try_write().unwrap();
        assert_eq!(n, 4);
        assert!(conn.outgoing.is_empty());

        // Intent flags flip back
        assert!(conn.want_read);
        assert!(!conn.want_write);

        // Verify client received the echo
        let mut buf = [0u8; 4];
        client.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ping");
    }

    #[test]
    fn test_try_read_eof() {
        let (mut conn, client) = setup_test_connection();

        // Dropping the client drops the socket (sends EOF/FIN to the server)
        drop(client);

        // try_read should cleanly return Ok(0)
        let n = wait_for_read(&mut conn);
        assert_eq!(n, 0);
    }
}
