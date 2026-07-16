//! Single-threaded non-blocking TCP echo server built around `poll(2)`.
//!
//! The event loop owns the connection registry, polls for readiness, and
//! delegates socket state and buffering to [`Conn`]. Each readable event may
//! contain zero, one, or many length-prefixed requests.

use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use std::collections::HashMap;
use std::error::Error;
use std::io;
use std::net::TcpListener;
use std::os::fd::{AsFd, AsRawFd, RawFd};

mod buffer;
mod config;
mod conn;

use crate::conn::Conn;

/// Starts the listener and runs the central poll-driven event loop forever.
fn main() -> Result<(), Box<dyn Error>> {
    let cfg = config::load_env()?;
    let addr = format!("0.0.0.0:{}", cfg.port);

    // Map file descriptors to Conn
    let mut fd2conn: HashMap<RawFd, Conn> = HashMap::new();

    // Start our TCP listener socket
    let listener: TcpListener = TcpListener::bind(&addr).unwrap();
    listener.set_nonblocking(true).unwrap();

    // Central Event Loop
    loop {
        // Build poll_args from our connection map
        let mut poll_args = build_poll_args(&listener, &fd2conn);

        // Wait for notification from OS
        match poll(&mut poll_args, PollTimeout::NONE) {
            Ok(_) => {}
            Err(nix::Error::EINTR) => continue,
            Err(e) => panic!("Poll failed: {}", e),
        }

        // Get any events from the listener (first member of poll_args)
        // and check whether it contains POLLIN, indicating that there's
        // one or more new client connections ready to be accepted
        let listener_ready = poll_args[0]
            .revents()
            .unwrap_or(PollFlags::empty())
            .contains(PollFlags::POLLIN);

        // Build a list of client connections which have some kind of
        // poll flags returned from the OS
        let ready_clients: Vec<(RawFd, PollFlags)> = poll_args
            .iter()
            // First item is the listener, not the clients
            .skip(1)
            // Get file descriptors that actually have poll
            .filter_map(|pfd| {
                pfd.revents()
                    .filter(|r| !r.is_empty())
                    .map(|r| (pfd.as_fd().as_raw_fd(), r))
            })
            .collect();

        // We are now done with poll_args, we can drop the reference
        drop(poll_args);

        // Listener has one or more clients ready to be accepted
        // Register them
        if listener_ready {
            accept_new_clients(&listener, &mut fd2conn);
        }

        // Process the clients which have poll flags
        handle_ready_clients(ready_clients, &mut fd2conn);
    }
}

/// Builds the list of file descriptors that `poll` should watch this iteration.
///
/// The first entry is always the listening socket. Each active connection adds
/// one entry whose flags are derived from that connection's current read/write intent.
fn build_poll_args<'sock>(
    listener: &'sock TcpListener,
    fd2conn: &'sock HashMap<RawFd, Conn>,
) -> Vec<PollFd<'sock>> {
    let mut poll_args: Vec<PollFd<'sock>> = Vec::new();

    // 1. Put Listening Socket in the list (listening for new clients)
    poll_args.push(PollFd::new(listener.as_fd(), PollFlags::POLLIN));

    // Put all active client sockets in the list
    for conn in fd2conn.values() {
        let mut flags = PollFlags::empty();

        if conn.want_read {
            flags.insert(PollFlags::POLLIN);
        }
        if conn.want_write {
            flags.insert(PollFlags::POLLOUT);
        }

        poll_args.push(PollFd::new(conn.stream.as_fd(), flags));
    }

    poll_args
}

/// Accepts every client currently queued on the listening socket.
///
/// Because the listener is non-blocking, `WouldBlock` means we have drained the
/// kernel's pending-accept queue for now.
fn accept_new_clients(listener: &TcpListener, fd2conn: &mut HashMap<RawFd, Conn>) {
    loop {
        match listener.accept() {
            Ok((stream, _addr)) => {
                // Register new client
                stream.set_nonblocking(true).unwrap();

                let fd = stream.as_raw_fd();
                fd2conn.insert(fd, Conn::new(stream));
                println!("New client connected! (fd: {})", fd);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // We accepted all pending clients. Break the loop
                break;
            }
            Err(e) => {
                eprintln!("Error accepting connection: {}", e);
                break;
            }
        }
    }
}

/// Describes whether a connection should stay registered after one I/O attempt.
enum PostIOState {
    /// Keep the connection registered for a future readiness event.
    KeepAlive,
    /// Remove the connection after the current I/O attempt.
    Close,
}

/// Handles the readiness flags returned by `poll` for each active client.
///
/// Structural mutation of the connection map stays here so helpers can borrow
/// one connection, return a semantic outcome, and let this function remove the
/// entry only after that borrow has ended.
fn handle_ready_clients(
    ready_clients: Vec<(RawFd, PollFlags)>,
    fd2conn: &mut HashMap<RawFd, Conn>,
) {
    for (fd, revents) in ready_clients {
        if revents.intersects(PollFlags::POLLIN | PollFlags::POLLHUP) {
            match try_read(fd, fd2conn) {
                PostIOState::Close => {
                    close(fd, fd2conn);
                    continue;
                }
                PostIOState::KeepAlive => {}
            }
        }

        if revents.contains(PollFlags::POLLHUP) {
            set_peer_closed(fd, fd2conn);
        }

        if revents.contains(PollFlags::POLLOUT) {
            match try_write(fd, fd2conn) {
                PostIOState::Close => {
                    close(fd, fd2conn);
                    continue;
                }
                PostIOState::KeepAlive => {}
            }
        }

        if revents.contains(PollFlags::POLLERR) || check_conn_ready_for_close(fd, fd2conn) {
            close(fd, fd2conn);
        }
    }
}

/// Attempts one non-blocking read on the selected connection.
///
/// EOF and hard read errors request closure. `WouldBlock` means the socket is
/// still alive and should remain in the connection map.
fn try_read(fd: i32, fd2conn: &mut HashMap<RawFd, Conn>) -> PostIOState {
    match fd2conn.get_mut(&fd) {
        Some(conn) => match conn.try_read() {
            Ok(()) => handle_requests(conn),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => PostIOState::KeepAlive,
            Err(e) => {
                eprintln!("Read error on fd {}: {}", fd, e);
                PostIOState::Close
            }
        },
        None => PostIOState::KeepAlive,
    }
}

/// Parses complete buffered requests and queues their responses in order.
///
/// Parsing stops at an incomplete request or when outgoing backpressure
/// reaches the high watermark. Buffer compaction is deferred until this pass
/// finishes.
fn handle_requests(conn: &mut Conn) -> PostIOState {
    // Stop parsing when queued output reaches the high watermark. This keeps
    // one large pipeline from growing the outgoing buffer without bound.
    while conn.can_process_requests() {
        let Some(msg) = conn.try_parse_one_request() else {
            break;
        };

        conn.queue_response(msg);
    }

    conn.incoming.compact();
    conn.outgoing.compact();

    if conn.ready_for_close() {
        return PostIOState::Close;
    }

    PostIOState::KeepAlive
}

/// Attempts one non-blocking write on the selected connection.
///
/// Write errors other than `WouldBlock` are treated as fatal for that client.
fn try_write(fd: i32, fd2conn: &mut HashMap<RawFd, Conn>) -> PostIOState {
    match fd2conn.get_mut(&fd) {
        Some(conn) => match conn.try_write() {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => PostIOState::KeepAlive,
            Err(e) => {
                eprintln!("Write error on fd {}: {}", fd, e);
                PostIOState::Close
            }
            _ => PostIOState::KeepAlive,
        },
        None => PostIOState::KeepAlive,
    }
}

/// Removes the connection from the server's registry.
///
/// Dropping the `Conn` closes the underlying socket.
fn close(fd: i32, fd2conn: &mut HashMap<RawFd, Conn>) {
    println!("Connection ended: (fd: {})", fd);
    fd2conn.remove(&fd);
}

/// Records that the peer has sent a hangup without immediately discarding
/// buffered input or output.
fn set_peer_closed(fd: i32, fd2conn: &mut HashMap<RawFd, Conn>) {
    match fd2conn.get_mut(&fd) {
        Some(conn) => {
            println!("Peer disconnected: (fd: {})", fd);
            conn.peer_closed = true
        }
        None => {}
    }
}

/// Checks whether a peer-closed connection has no work left to drain.
fn check_conn_ready_for_close(fd: i32, fd2conn: &mut HashMap<RawFd, Conn>) -> bool {
    match fd2conn.get(&fd) {
        Some(conn) => conn.ready_for_close(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::thread;
    use std::time::{Duration, Instant};

    fn setup_connection() -> (Conn, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();

        server.set_nonblocking(true).unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        (Conn::new(server), client)
    }

    fn insert_connection(fd2conn: &mut HashMap<RawFd, Conn>) -> (RawFd, TcpStream) {
        let (conn, client) = setup_connection();
        let fd = conn.stream.as_raw_fd();
        fd2conn.insert(fd, conn);
        (fd, client)
    }

    fn wait_for_read(fd: RawFd, fd2conn: &mut HashMap<RawFd, Conn>) -> PostIOState {
        let deadline = Instant::now() + Duration::from_millis(100);
        loop {
            let state = try_read(fd, fd2conn);
            if fd2conn
                .get(&fd)
                .is_some_and(|conn| !conn.incoming.is_empty() || !conn.outgoing.is_empty())
                || matches!(state, PostIOState::Close)
                || Instant::now() >= deadline
            {
                return state;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn framed(payload: &[u8]) -> Vec<u8> {
        let mut request = (payload.len() as u32).to_be_bytes().to_vec();
        request.extend_from_slice(payload);
        request
    }

    #[test]
    fn build_poll_args_always_registers_listener_for_reading() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let connections = HashMap::new();

        let poll_args = build_poll_args(&listener, &connections);

        assert_eq!(poll_args.len(), 1);
        assert_eq!(poll_args[0].as_fd().as_raw_fd(), listener.as_raw_fd());
        assert_eq!(poll_args[0].events(), PollFlags::POLLIN);
    }

    #[test]
    fn build_poll_args_uses_connection_interest_flags() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut connections = HashMap::new();
        let (read_fd, _read_client) = insert_connection(&mut connections);
        let (write_fd, _write_client) = insert_connection(&mut connections);
        let (both_fd, _both_client) = insert_connection(&mut connections);

        connections.get_mut(&read_fd).unwrap().want_read = true;
        connections.get_mut(&read_fd).unwrap().want_write = false;
        connections.get_mut(&write_fd).unwrap().want_read = false;
        connections.get_mut(&write_fd).unwrap().want_write = true;
        connections.get_mut(&both_fd).unwrap().want_read = true;
        connections.get_mut(&both_fd).unwrap().want_write = true;

        let poll_args = build_poll_args(&listener, &connections);
        let mut flags_by_fd = HashMap::new();
        for poll_fd in poll_args.iter().skip(1) {
            flags_by_fd.insert(poll_fd.as_fd().as_raw_fd(), poll_fd.events());
        }

        assert_eq!(flags_by_fd[&read_fd], PollFlags::POLLIN);
        assert_eq!(flags_by_fd[&write_fd], PollFlags::POLLOUT);
        assert_eq!(
            flags_by_fd[&both_fd],
            PollFlags::POLLIN | PollFlags::POLLOUT
        );
    }

    #[test]
    fn accept_new_clients_drains_all_pending_connections() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let _client_one = TcpStream::connect(addr).unwrap();
        let _client_two = TcpStream::connect(addr).unwrap();
        let mut connections = HashMap::new();

        let deadline = Instant::now() + Duration::from_millis(100);
        while connections.len() < 2 && Instant::now() < deadline {
            accept_new_clients(&listener, &mut connections);
            if connections.len() < 2 {
                thread::sleep(Duration::from_millis(1));
            }
        }

        assert_eq!(connections.len(), 2);
        assert!(connections.values().all(|conn| conn.want_read));
        assert!(connections.values().all(|conn| !conn.want_write));
    }

    #[test]
    fn accept_new_clients_is_noop_when_queue_is_empty() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut connections = HashMap::new();

        accept_new_clients(&listener, &mut connections);

        assert!(connections.is_empty());
    }

    #[test]
    fn try_read_handles_missing_connection() {
        let mut connections = HashMap::new();

        assert!(matches!(
            try_read(123, &mut connections),
            PostIOState::KeepAlive
        ));
    }

    #[test]
    fn try_read_handles_partial_request_without_queuing_response() {
        let mut connections = HashMap::new();
        let (fd, mut client) = insert_connection(&mut connections);
        client.write_all(&[0, 0, 0]).unwrap();

        let state = wait_for_read(fd, &mut connections);

        assert!(matches!(state, PostIOState::KeepAlive));
        let conn = connections.get(&fd).unwrap();
        assert_eq!(conn.incoming.as_slice(), &[0, 0, 0]);
        assert!(conn.outgoing.is_empty());
    }

    #[test]
    fn try_read_handles_pipelined_requests_and_queues_each_response() {
        let mut connections = HashMap::new();
        let (fd, mut client) = insert_connection(&mut connections);
        let mut requests = framed(b"one");
        requests.extend_from_slice(&framed(b"two"));
        client.write_all(&requests).unwrap();

        let state = wait_for_read(fd, &mut connections);

        assert!(matches!(state, PostIOState::KeepAlive));
        let conn = connections.get(&fd).unwrap();
        assert!(conn.incoming.is_empty());
        assert_eq!(conn.outgoing.as_slice(), b"onetwo");
        assert!(conn.want_write);
    }

    #[test]
    fn try_read_closes_connection_after_peer_eof_when_no_data_remains() {
        let (conn, client) = setup_connection();
        let fd = conn.stream.as_raw_fd();
        let mut connections = HashMap::from([(fd, conn)]);
        drop(client);

        let deadline = Instant::now() + Duration::from_millis(100);
        let state = loop {
            let state = try_read(fd, &mut connections);
            if matches!(state, PostIOState::Close) || Instant::now() >= deadline {
                break state;
            }
            thread::sleep(Duration::from_millis(1));
        };

        assert!(matches!(state, PostIOState::Close));
    }

    #[test]
    fn try_write_handles_missing_connection() {
        let mut connections = HashMap::new();

        assert!(matches!(
            try_write(123, &mut connections),
            PostIOState::KeepAlive
        ));
    }

    #[test]
    fn try_write_flushes_queued_response_and_client_can_read_it() {
        let mut connections = HashMap::new();
        let (fd, mut client) = insert_connection(&mut connections);
        connections
            .get_mut(&fd)
            .unwrap()
            .queue_response(b"pong".to_vec());

        assert!(matches!(
            try_write(fd, &mut connections),
            PostIOState::KeepAlive
        ));

        let mut response = [0; 4];
        client.read_exact(&mut response).unwrap();
        let conn = connections.get(&fd).unwrap();
        assert_eq!(&response, b"pong");
        assert!(conn.outgoing.is_empty());
        assert!(conn.want_read);
        assert!(!conn.want_write);
    }

    #[test]
    fn handle_requests_processes_all_complete_requests_and_compacts_input() {
        let (mut conn, _) = setup_connection();
        let mut requests = framed(b"one");
        requests.extend_from_slice(&framed(b"two"));
        conn.incoming.append(&requests);

        assert!(matches!(handle_requests(&mut conn), PostIOState::KeepAlive));
        assert!(conn.incoming.is_empty());
        assert_eq!(conn.outgoing.as_slice(), b"onetwo");
    }

    #[test]
    fn handle_requests_closes_only_when_peer_closed_and_buffers_are_empty() {
        let (mut conn, _) = setup_connection();
        conn.peer_closed = true;

        assert!(matches!(handle_requests(&mut conn), PostIOState::Close));
    }

    #[test]
    fn close_removes_connection_from_registry() {
        let mut connections = HashMap::new();
        let (fd, _client) = insert_connection(&mut connections);

        close(fd, &mut connections);

        assert!(!connections.contains_key(&fd));
    }

    #[test]
    fn handle_ready_clients_keeps_unknown_descriptors() {
        let mut connections = HashMap::new();

        handle_ready_clients(
            vec![(123, PollFlags::POLLIN | PollFlags::POLLOUT)],
            &mut connections,
        );

        assert!(connections.is_empty());
    }

    #[test]
    fn handle_ready_clients_closes_on_error_or_hangup() {
        let mut connections = HashMap::new();
        let (fd, _client) = insert_connection(&mut connections);

        handle_ready_clients(vec![(fd, PollFlags::POLLERR)], &mut connections);

        assert!(!connections.contains_key(&fd));
    }

    #[test]
    fn handle_ready_clients_processes_read_then_write_when_both_are_ready() {
        let mut connections = HashMap::new();
        let (fd, mut client) = insert_connection(&mut connections);
        client.write_all(&framed(b"ping")).unwrap();

        let deadline = Instant::now() + Duration::from_millis(100);
        let state = PostIOState::KeepAlive;
        let mut response = Vec::new();
        client.set_nonblocking(true).unwrap();

        while Instant::now() < deadline {
            handle_ready_clients(
                vec![(fd, PollFlags::POLLIN | PollFlags::POLLOUT)],
                &mut connections,
            );

            let mut buf = [0; 4];
            match client.read(&mut buf) {
                Ok(n) => response.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("unexpected client read error: {e}"),
            }

            if response.len() == 4 {
                break;
            }

            thread::sleep(Duration::from_millis(1));
        }

        assert!(matches!(state, PostIOState::KeepAlive));
        assert_eq!(response, b"ping");
    }

    #[test]
    fn handle_ready_clients_preserves_response_after_peer_half_close() {
        let mut connections = HashMap::new();
        let (fd, mut client) = insert_connection(&mut connections);
        client.write_all(&framed(b"ping")).unwrap();
        client.shutdown(Shutdown::Write).unwrap();

        let deadline = Instant::now() + Duration::from_millis(100);
        while Instant::now() < deadline {
            handle_ready_clients(vec![(fd, PollFlags::POLLIN)], &mut connections);
            if connections
                .get(&fd)
                .is_some_and(|conn| !conn.outgoing.is_empty())
            {
                break;
            }

            thread::sleep(Duration::from_millis(1));
        }

        let conn = connections
            .get(&fd)
            .expect("connection was closed before its response was flushed");
        assert_eq!(conn.outgoing.as_slice(), b"ping");

        handle_ready_clients(vec![(fd, PollFlags::POLLHUP)], &mut connections);
        assert!(connections.contains_key(&fd));

        handle_ready_clients(vec![(fd, PollFlags::POLLOUT)], &mut connections);

        let mut response = [0; 4];
        client.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"ping");
    }
}
