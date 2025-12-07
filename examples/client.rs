use clap::Parser;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Simple TCP client
#[derive(Parser)]
struct Args {
    /// Server IP address
    #[clap(short, long, default_value = "127.0.0.1")]
    ip: String,

    /// Server port
    #[clap(short, long)]
    port: u16,

    /// Message to send
    #[clap(short, long, default_value = "Hello Server!")]
    message: String,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();
    let addr: SocketAddr = format!("{}:{}", args.ip, args.port).parse().unwrap();

    println!("Connecting to server at {}", addr);

    let mut stream = tokio::net::TcpStream::connect(addr).await?;
    stream.write_all(args.message.as_bytes()).await?;
    println!("Sent: {}", args.message);

    let mut buf = [0u8; 1024];
    let n = stream.read(&mut buf).await?;
    println!("Received: {}", String::from_utf8_lossy(&buf[..n]));

    Ok(())
}
