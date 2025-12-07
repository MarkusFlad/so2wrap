use clap::Parser;
use so2wrap::bound_socket::BoundSocket;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Simple TCP server using BoundSocket
#[derive(Parser)]
struct Args {
    /// IP address to bind to
    #[clap(short, long, default_value = "127.0.0.1")]
    ip: String,

    /// Port to bind to (0 = OS chooses)
    #[clap(short, long, default_value = "14722")]
    port: u16,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let server_addr: SocketAddr = format!("{}:{}", args.ip, args.port).parse().unwrap();
    if let Err(e) = main_loop(server_addr).await {
        eprintln!("Server error: {}", e);
    }
}

async fn main_loop(server_addr: SocketAddr) -> std::io::Result<()> {
    println!("Starting BoundSocket server on {}", server_addr);
    let bound_socket = BoundSocket::new(server_addr).await?;

    loop {
        // Acquire exclusive ListeningSocket
        let listening_socket = bound_socket.listen(128).await?;
        let local_addr = bound_socket.local_addr().await;
        println!("Listening on {}", local_addr);
        // Accept a client
        let (mut socket, peer_addr) = listening_socket.accept().await?;
        println!("Accepted connection from {}", peer_addr);
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            match socket.read(&mut buf).await {
                Ok(n) if n > 0 => {
                    println!("Received: {}", String::from_utf8_lossy(&buf[..n]));
                    let _ = socket.write_all(b"Hello from server!\n").await;
                }
                _ => println!("Client disconnected or error"),
            }
        });
        // ListeningSocket is dropped here, allowing new connections via cloned BoundSockets
    }
}
