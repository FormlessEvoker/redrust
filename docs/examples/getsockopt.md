# Exploring Socket Options: getsockopt

**Status:** Active  
**Audience:** Contributors working on low-level socket tuning or debugging buffer sizes.  
**Overview:** A runnable example demonstrating how to inspect the kernel's underlying socket configuration using `getsockopt`.

---

## What is `getsockopt`?

When you create a `TcpStream`, the kernel manages a lot of hidden state for that connection, such as:
- How big the receive queue is (`SO_RCVBUF`).
- How big the send queue is (`SO_SNDBUF`).
- Whether keep-alive probes are enabled (`SO_KEEPALIVE`).

The `getsockopt` (Get Socket Option) system call allows us to query the kernel for this data. (There is also a reciprocal `setsockopt` for modifying these values).

> **A Note on `TCP_INFO` vs Cross-Platform Constraints:** 
> If you are reading Linux-specific tutorials, you might see references to `TCP_INFO`. This is a massive struct that exposes internal metrics like retransmission timeouts and congestion window sizes. However, `TCP_INFO` is entirely Linux-specific. Because macOS (Darwin) doesn't support it standard, we rely on POSIX-compliant flags like `SO_RCVBUF` and `SO_SNDBUF` for broader compatibility in our examples.

## The Example Code

We use the `nix::sys::socket::getsockopt` wrapper, which safely converts our Rust `TcpStream` into a file descriptor under the hood and executes the system call.

You can find the runnable example in `examples/getsockopt_demo.rs`. It connects to a remote server, rips open the hood of the socket, and prints out the kernel's default assignments.

```rust
use std::net::TcpStream;
use nix::sys::socket;

fn main() {
    // 1. Establish a standard blocking TCP connection
    let stream = TcpStream::connect("google.com:80").expect("Failed to connect");

    // 2. Query the kernel for socket parameters
    // nix safely extracts the underlying file descriptor via the AsFd trait
    let rcvbuf = socket::getsockopt(&stream, socket::sockopt::RcvBuf).unwrap();
    let sndbuf = socket::getsockopt(&stream, socket::sockopt::SndBuf).unwrap();
    let keepalive = socket::getsockopt(&stream, socket::sockopt::KeepAlive).unwrap();
    
    // 3. Print the results
    println!("=== Kernel Socket Buffers ===");
    println!("Receive Buffer (SO_RCVBUF): {} bytes", rcvbuf);
    println!("Send Buffer (SO_SNDBUF): {} bytes", sndbuf);
    println!("Keep-Alive (SO_KEEPALIVE): {}", keepalive);
}
```

## Running It

Execute the following command in your terminal from the root of the `redrust` repository:

```bash
cargo run --example getsockopt_demo
```

### Typical Output

On most systems, you'll see buffer sizes hovering around 128KB (131,376 bytes), which is dynamically allocated by the kernel for this particular socket. Keep-alive is typically disabled (`false`) by default unless explicitly turned on.

```text
=== Kernel Socket Buffers ===
Receive Buffer (SO_RCVBUF): 131376 bytes
Send Buffer (SO_SNDBUF): 131376 bytes
Keep-Alive (SO_KEEPALIVE): false
```