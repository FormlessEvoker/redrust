# Why use `nix` in RedRust (rationale)

This note explains why this project prefers the `nix` crate and lower-level OS primitives over higher-level event frameworks like `mio` or async runtimes like `tokio`.

## Goals and tradeoffs

- Primary goal: learning systems-level networking, sockets, and event-loop mechanics by implementing them with minimal abstraction.
- `nix` exposes Unix-like APIs (posix syscalls, `fcntl`, `getsockopt`, raw socket operations) that let you see and control exactly what the OS is doing. That visibility is valuable for learning.

Tradeoffs:
- More boilerplate and platform-specific code compared to `mio`/`tokio`.
- You must manage non-blocking semantics, partial reads/writes, and re-registration logic yourself.
- Not intended as a production-ready framework — rather, an educational implementation that can be refactored/replaced later.

## What `nix` gives you

- Direct access to POSIX syscalls and socket options.
- Fine-grained control over socket flags (non-blocking, close-on-exec), `fcntl`, `setsockopt` and low-level error codes.
- Easier experimentation with `getsockopt` variants (platform-specific options), raw `recvmsg`/`sendmsg`, and ancillary data.

## Why not `mio` or `tokio` for this project (initially)

- `mio` is a cross-platform readiness API (epoll/kqueue/IOCP) that is excellent for building high-performance servers. However, it abstracts away some syscall-level details and standardizes the event model.
- `tokio` provides an async runtime and many conveniences (tasks, timers, combinators). It hides event-loop mechanics behind futures and a scheduler.
- For learning the foundations (how sockets and polling actually behave), using `nix` and `std::net` with explicit non-blocking I/O keeps those mechanisms visible and explicit.

## When to adopt `mio`/`tokio`

- After implementing and understanding a raw event loop and socket handling, consider porting the event-loop layer to `mio` to get a robust, cross-platform readiness backend while preserving your higher-level logic.
- If you want to experiment with async/await concurrency and the broader Rust ecosystem, porting to `tokio` will provide performance and ergonomics but will also reintroduce some abstraction.

## Practical guidance for mixing approaches

- You can use `nix` for socket setup and fine-grained socket options, while using `mio` for scalable event polling. That gives both visibility and scalability.
- Example pattern: use `nix` to create and configure the socket (set CLOEXEC, reuseaddr, nonblocking), then convert the FD to an object `mio` can register for readiness events.

## Example: set non-blocking and CLOEXEC with `nix`

```rust
use nix::fcntl::{fcntl, FcntlArg, OFlag};
use std::os::unix::io::AsRawFd;

let stream = std::net::TcpStream::connect(addr)?;
let fd = stream.as_raw_fd();
// Set non-blocking
let flags = OFlag::from_bits_truncate(fcntl(fd, FcntlArg::F_GETFL)?);
fcntl(fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
// Set close-on-exec
let fdflags = OFlag::from_bits_truncate(fcntl(fd, FcntlArg::F_GETFD)?);
fcntl(fd, FcntlArg::F_SETFD(fdflags | OFlag::FD_CLOEXEC))?;
```

Note: the exact `fcntl` constants to use may vary; the example shows the general idea.

## Summary

- Using `nix` aligns with this project's educational goals: direct syscall exposure, clearer learning of TCP/IP and OS interactions, and manual event-loop handling.
- When you want to increase portability or reduce boilerplate, you can migrate the polling layer to `mio` and later to `tokio` if you prefer an async-first design.

---

File: `docs/implementation/why_nix.md`
