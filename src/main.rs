//! Single-threaded non-blocking TCP echo server built around `poll(2)`.

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
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

        // listener has one or more clients ready to be accepted
        // register them
        if listener_ready {
            accept_new_clients(&listener, &mut fd2conn);
        }

        // process the clients which are already
        react_to_clients(ready_clients, &mut fd2conn);
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
    KeepAlive,
    Close,
}

/// Handles the readiness flags returned by `poll` for each active client.
///
/// Structural mutation of the connection map stays here so helpers can borrow
/// one connection, return a semantic outcome, and let this function remove the
/// entry only after that borrow has ended.
fn react_to_clients(ready_clients: Vec<(RawFd, PollFlags)>, fd2conn: &mut HashMap<RawFd, Conn>) {
    for (fd, revents) in ready_clients {
        if revents.contains(PollFlags::POLLIN) {
            match try_read(fd, fd2conn) {
                PostIOState::Close => {
                    close(fd, fd2conn);
                    continue;
                }
                PostIOState::KeepAlive => {}
            }
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

        if revents.intersects(PollFlags::POLLERR | PollFlags::POLLHUP) {
            close(fd, fd2conn);
        }
    }
}

/// Attempts one non-blocking read on the selected connection.
///
/// EOF and hard read errors request closure. `WouldBlock` means the socket is
/// still alive and should remain in the connection map.
// TODO: this will change and it need to read as much as possible, then iterate through each request, handle it, etc
// The loop will be something like this:
//  - use conn.try_read to read from socket until there's nothing left to read
//  - loop try_parse_one_request to get a list of requests
//  - using the sum of the request lengths, truncate the incoming vector on conn
//  - call a `handle_request` function which just echoes the request into the outgoing vector on conn for now
fn try_read(fd: i32, fd2conn: &mut HashMap<RawFd, Conn>) -> PostIOState {
    match fd2conn.get_mut(&fd) {
        Some(conn) => match conn.try_read() {
            Ok(()) => PostIOState::KeepAlive,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => PostIOState::KeepAlive,
            Err(e) => {
                eprintln!("Read error on fd {}: {}", fd, e);
                PostIOState::Close
            }
        },
        None => PostIOState::KeepAlive,
    }
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
    println!("Client disconnected! (fd: {})", fd);
    fd2conn.remove(&fd);
}

// #[test]
// fn test_try_read_and_echo() {
//     let (mut conn, mut client) = setup_test_connection();

//     // Client writes data to server
//     client.write_all(b"ping").unwrap();

//     // Server reads it without blocking
//     wait_for_read(&mut conn);
//     assert_eq!(conn.incoming, b"ping");
//     assert_eq!(conn.outgoing, b"ping"); // Echo server logic

//     // Intent flags should flip to allow write polling
//     assert!(!conn.want_read);
//     assert!(conn.want_write);

//     // Server flushed write buffer to client
//     let n = conn.try_write().unwrap();
//     assert_eq!(n, 4);
//     assert!(conn.outgoing.is_empty());

//     // Intent flags flip back
//     assert!(conn.want_read);
//     assert!(!conn.want_write);

//     // Verify client received the echo
//     let mut buf = [0u8; 4];
//     client.read_exact(&mut buf).unwrap();
//     assert_eq!(&buf, b"ping");
// }
