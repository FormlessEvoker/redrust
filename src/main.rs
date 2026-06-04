use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use std::collections::HashMap;
use std::error::Error;
use std::io;
use std::net::TcpListener;
use std::os::fd::{AsFd, AsRawFd, RawFd};

mod config;
mod conn;

use crate::conn::Conn;

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
            .skip(1)
            .filter_map(|pfd| {
                pfd.revents()
                    .filter(|r| !r.is_empty())
                    .map(|r| (pfd.as_fd().as_raw_fd(), r))
            })
            .collect();

        drop(poll_args);

        if listener_ready {
            accept_new_clients(&listener, &mut fd2conn);
        }

        process_ready_clients(ready_clients, &mut fd2conn);
    }
}

// Given the listener and the connection map,
// build a list of poll file descriptors, which are the fds which we are listening
// to OS events for sockets that
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

fn process_ready_clients(
    ready_clients: Vec<(RawFd, PollFlags)>,
    fd2conn: &mut HashMap<RawFd, Conn>,
) {
    let mut disconnected_clients = Vec::new();

    for (fd, revents) in ready_clients {
        if let Some(conn) = fd2conn.get_mut(&fd) {
            let mut close_connection = false;

            // For each revent:
            //  - if POLLIN, try to read
            //    - if EOF, close
            //    - if WouldBlock, ignore
            //    - if Error, report and close
            //    - otherwise, continue
            //  - if POLLOUT, and not need to close, try to write
            //    - if WouldBlock, ignore
            //    - if Error, report and close
            //    - otherwise, continue
            //  - if POLLERR or POLLHUP, close

            if revents.contains(PollFlags::POLLIN) {
                match conn.try_read() {
                    Ok(0) => {
                        // Peer closed the connection cleanly (EOF)
                        close_connection = true;
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        // False alarm from OS, ignore
                    }
                    Err(e) => {
                        eprintln!("Read error on fd {}: {}", fd, e);
                        close_connection = true;
                    }
                }
            }

            if !close_connection && revents.contains(PollFlags::POLLOUT) {
                match conn.try_write() {
                    Ok(_) => {}
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        // Kernel buffer full, ignore
                    }
                    Err(e) => {
                        eprintln!("Write error on fd {}: {}", fd, e);
                        close_connection = true;
                    }
                }
            }

            // If POLLERR or POLLHUP occur, close
            if revents.intersects(PollFlags::POLLERR | PollFlags::POLLHUP) {
                close_connection = true;
            }

            if close_connection {
                disconnected_clients.push(fd);
            }
        }
    }

    // Clean up closed connections
    for fd in disconnected_clients {
        println!("Client disconnected! (fd: {})", fd);
        fd2conn.remove(&fd);
    }
}
