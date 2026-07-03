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

// Stands for REvent action
// as in an action to take for specific REvent...
// or our "reaction" to it... lol
enum REAction {
    TryRead,
    TryWrite,
    Close,
}

fn react_to_clients(ready_clients: Vec<(RawFd, PollFlags)>, fd2conn: &mut HashMap<RawFd, Conn>) {
    ready_clients
        .into_iter()
        .flat_map(|(fd, revents)| {
            let mut actions = Vec::new();

            if revents.contains(PollFlags::POLLIN) {
                actions.push((fd, REAction::TryRead))
            }

            if revents.contains(PollFlags::POLLOUT) {
                actions.push((fd, REAction::TryWrite))
            }

            if revents.intersects(PollFlags::POLLERR | PollFlags::POLLHUP) {
                actions.push((fd, REAction::Close))
            }

            actions
        })
        .for_each(|(fd, action)| match action {
            REAction::TryRead => try_read(fd, fd2conn),
            REAction::TryWrite => try_write(fd, fd2conn),
            REAction::Close => close(fd, fd2conn),
        })
}

fn try_read(fd: i32, fd2conn: &mut HashMap<RawFd, Conn>) {
    let should_close = match fd2conn.get_mut(&fd) {
        Some(conn) => match conn.try_read() {
            Ok(0) => true,
            Ok(_) => false,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => false,
            Err(e) => {
                eprintln!("Read error on fd {}: {}", fd, e);
                true
            }
        },
        None => false,
    };
    if should_close {
        close(fd, fd2conn);
    }
}

fn try_write(fd: i32, fd2conn: &mut HashMap<RawFd, Conn>) {
    let should_close = match fd2conn.get_mut(&fd) {
        Some(conn) => match conn.try_write() {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => false,
            Err(e) => {
                eprintln!("Write error on fd {}: {}", fd, e);
                true
            }
            _ => false,
        },
        None => false,
    };
    if should_close {
        close(fd, fd2conn);
    }
}

fn close(fd: i32, fd2conn: &mut HashMap<RawFd, Conn>) {
    println!("Client disconnected! (fd: {})", fd);
    fd2conn.remove(&fd);
}
