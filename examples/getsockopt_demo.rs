use std::net::TcpStream;
use nix::sys::socket;

fn main() {
    let stream = TcpStream::connect("google.com:80").expect("Failed to connect");

    let rcvbuf = socket::getsockopt(&stream, socket::sockopt::RcvBuf).unwrap();
    let sndbuf = socket::getsockopt(&stream, socket::sockopt::SndBuf).unwrap();
    let keepalive = socket::getsockopt(&stream, socket::sockopt::KeepAlive).unwrap();
    
    println!("=== Kernel Socket Buffers ===");
    println!("Receive Buffer (SO_RCVBUF): {} bytes", rcvbuf);
    println!("Send Buffer (SO_SNDBUF): {} bytes", sndbuf);
    println!("Keep-Alive (SO_KEEPALIVE): {}", keepalive);
}
