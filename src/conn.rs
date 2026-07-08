//! Per-connection state and non-blocking read/write helpers.

use std::io;
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
    /// Whether the peer has closed their side of the connection
    pub peer_closed: bool,

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
            want_read: true,   // always read first request
            want_write: false, // nothing to write on start
            peer_closed: false,
            incoming: Vec::new(),
            outgoing: Vec::new(),
        }
    }

    /// Reads from the socket until EOF, error, or WouldBlock
    /// Puts all read bytes into `incoming` buffer
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
                    self.incoming.extend_from_slice(&buf[..n]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    return Ok({});
                }
                Err(e) => return Err(e),
            }
        }
    }

    // TODO: add a try_parse_one_request function which attempts to parse a request from the incoming buffer.
    //  - If not a full request, just returns empty
    //  - If there's a full request, return it and also the length of it

    // TODO: add a function `truncate_front` or similar which allows us to essentially remove the front N bytes of the incoming vector
    // This will be used after repeated `try_parse_one_request` calls so that we can drop the bytes that have already been read ONCE at the end

    // TODO: Add a `queue_response` or similar function which appends data into the outgoing buffer

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

    pub fn ready_for_close(&self) -> bool {
        self.peer_closed && self.incoming.is_empty() && self.outgoing.is_empty()
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
    fn wait_for_read(conn: &mut Conn) -> () {
        loop {
            match conn.try_read() {
                Ok(()) => (),
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
        wait_for_read(&mut conn);
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

        // try_read should cleanly return Ok(())
        let res = wait_for_read(&mut conn);
        assert_eq!(res, ());
    }
}
