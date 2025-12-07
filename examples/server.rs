use clap::Parser;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Simple TCP server using BoundSocket
#[derive(Parser)]
struct Args {
    /// IP address to bind to
    #[clap(short, long, default_value = "127.0.0.1")]
    ip: String,

    /// Port to bind to (0 = OS chooses)
    #[clap(short, long, default_value = "0")]
    port: u16,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();
    let addr: SocketAddr = format!("{}:{}", args.ip, args.port).parse().unwrap();

    println!("Starting BoundSocket server on {}", addr);

    let bound = bound_socket::BoundSocket::new(addr).await?;
    
    loop {
        // Acquire exclusive listener
        let listener = bound.listen(128).await?;
        let local_addr = bound.local_addr().await;
        println!("Listening on {}", local_addr);

        // Accept a client
        let (mut socket, peer_addr) = listener.accept().await?;
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

        // Drop listener to trigger hot-swap for next client
        drop(listener);
    }
}
