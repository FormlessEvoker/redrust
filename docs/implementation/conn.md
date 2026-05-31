# `Conn` — connection implementation and event-loop integration

This document explains the `Conn` struct in this repository, what each field means, how `try_read()` / `try_write()` behave, and how `Conn` is intended to be used from an event loop.

See the implementation in the code: [src/conn.rs](src/conn.rs)

## Overview

- `Conn` represents a single connected TCP endpoint. It owns a `TcpStream` and maintains application-level buffers and intent flags used by a poll-driven event loop.

## Fields explained

- `stream: TcpStream` — the Rust handle that wraps the OS socket. Use `stream.set_nonblocking(true)` in the server to avoid blocking reads/writes.
- `want_read: bool` — whether the application wants to receive readable notifications for the socket.
- `want_write: bool` — whether the application wants writable notifications for the socket.
- `incoming: Vec<u8>` — bytes read from the socket and not yet processed by the application.
- `outgoing: Vec<u8>` — bytes the application wants to send to the peer; `try_write()` flushes these.

## Methods and semantics

### `new(stream: TcpStream)`
- Constructs a new `Conn`. By default `want_read = true` so a newly-accepted connection begins by reading the client's request.

### `try_read(&mut self) -> std::io::Result<usize>`
- Attempts to read up to a fixed-size buffer from the socket and appends bytes to `incoming`.
- `Ok(n)` with `n > 0`: `n` bytes were read. The example `Conn` echoes those bytes to `outgoing` for testing.
- `Ok(0)`: peer performed an orderly shutdown (EOF / FIN).
- `Err(e)`:
  - If `e.kind() == std::io::ErrorKind::WouldBlock`, no data is available on a non-blocking socket — wait for the next readable event.
  - Other `Err` values represent real I/O errors and typically require closing the connection or logging/handling the error.
- Side effects: sets `want_read`/`want_write` to guide the poller (the example flips to write when it has data to send).

### `try_write(&mut self) -> std::io::Result<usize>`
- Attempts to write as many bytes as the OS will accept from `outgoing`.
- `Ok(n)`: `n` bytes written; `outgoing.drain(..n)` removes those bytes from the buffer.
- If `outgoing` becomes empty, `want_write` is cleared and `want_read` is typically re-enabled.
- `Err(e)`:
  - `WouldBlock` indicates the socket's send buffer is full — wait for a writable event.
  - Other errors are fatal for the connection.

## Event loop integration (pseudo-code)

The `want_read` / `want_write` flags express interest to the OS poller (e.g., `mio`, `poll`, `epoll`). The poller should register or reregister the socket with the appropriate interest mask based on those flags.

Example loop pseudocode:

```rust
loop {
    // Build interest set from Conn.want_read/want_write
    poll.poll(&mut events, timeout)?;
    for event in events.iter() {
        let conn = conn_for_event(event);
        if event.is_readable() && conn.want_read {
            match conn.try_read() {
                Ok(0) => close(conn),
                Ok(_) => reregister(conn),
                Err(e) if e.kind() == WouldBlock => (),
                Err(_) => close(conn),
            }
        }
        if event.is_writable() && conn.want_write {
            match conn.try_write() {
                Ok(_) => reregister(conn),
                Err(e) if e.kind() == WouldBlock => (),
                Err(_) => close(conn),
            }
        }
    }
}
```

Notes:
- Always reregister interest with the poller after changing `want_read` / `want_write`.
- Don't busy-loop on `WouldBlock`: rely on the OS to notify when the socket becomes readable/writable.

## Partial writes and buffering

TCP `write()` calls are allowed to write fewer bytes than requested. `Conn::try_write()` drains the prefix of `outgoing` that was written — leaving the remainder for the next writable event. This is the correct behavior for non-blocking I/O.

## Tests and behavior in this repo

See the unit tests in [`src/conn.rs`](src/conn.rs#L1) which create a loopback connection and exercise the read/echo/write behavior. The tests illustrate non-blocking server socket behavior and the `Ok(0)` EOF case.

## Observing OS-level socket state (`getsockopt`, `TCP_INFO`)

If you want to inspect kernel TCP state (retransmits, RTT, state) programmatically, Linux exposes `TCP_INFO` via `getsockopt`. Example using the `socket2` crate and `libc` on Linux:

```rust
// Cargo.toml: socket2 = "0.4"
use socket2::{Socket, TcpKeepalive};
use std::os::unix::io::AsRawFd;

let sock = Socket::from(stream.try_clone().unwrap());
let fd = sock.as_raw_fd();

// On Linux you can call getsockopt with TCP_INFO (requires libc bindings):
// unsafe { libc::getsockopt(fd, libc::IPPROTO_TCP, libc::TCP_INFO, &mut info as *mut _ as *mut _, &mut len) };
```

Notes:
- `TCP_INFO` is Linux-specific and returns a `tcp_info` struct with internal TCP metrics.
- On macOS/BSD, there is no direct `TCP_INFO` equivalent; use `netstat`, `ss`, or system tracing (DTrace) to observe details.
- Using raw `getsockopt` requires unsafe code and platform-specific `libc` structs; prefer a crate that wraps these if available.

## Further reading and next steps

- `src/conn.rs` is intentionally small and focused; consider expanding docs to show full lifecycle with parser integration (RESP parsing) and backpressure handling.
- Add a short example that demonstrates toggling `want_read`/`want_write` with `mio` or `poll` (I can add an example file if you'd like).

---

File: `docs/implementation/conn.md`
