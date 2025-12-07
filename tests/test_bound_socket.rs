use so2wrap::bound_tcp_socket::BoundSocket;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test]
async fn bound_socket_real_tcp_handover_between_two_primaries() {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    let bound = BoundSocket::new(addr).await.unwrap();

    // ─────────────────────────────────────────────
    // PRIMARY 1
    // ─────────────────────────────────────────────
    let bound_clone = bound.clone();
    let tcp_server1 = tokio::spawn(async move {
        let listening_socket = bound_clone.listen(128).await.unwrap();
        // First primary is now active
        listening_socket
    });
    let listening_socket_server1 = tcp_server1.await.unwrap();
    // Get the actual bound address
    let actual_addr = bound.local_addr().await;
    // Client 1 connects
    let mut client1 = TcpStream::connect(actual_addr).await.unwrap();
    client1.write_all(b"hello-1").await.unwrap();
    // Server 1 accepts
    let (mut server1_tcp_stream, _) = listening_socket_server1.accept().await.unwrap();
    let mut buf = [0u8; 32];
    let n = server1_tcp_stream.read(&mut buf).await.unwrap();
    assert_eq!(&buf[..n], b"hello-1");
    // Drop of primary 1 -> should trigger cleanup and allow primary 2 to accept
    drop(listening_socket_server1);

    // ─────────────────────────────────────────────
    // PRIMARY 2
    // ─────────────────────────────────────────────
    let bound_clone2 = bound.clone();
    let tcp_server2 = tokio::spawn(async move { bound_clone2.listen(128).await.unwrap() });
    let listening_socket_server2 = tcp_server2.await.unwrap();
    // Actual IP address should be the same (port may differ because of re-binding and OS behavior with 0 port)
    let actual_addr_2 = bound.local_addr().await;
    assert_eq!(actual_addr.ip(), actual_addr_2.ip());
    // Client 2 connects
    let mut client2 = TcpStream::connect(actual_addr_2).await.unwrap();
    client2.write_all(b"hello-2").await.unwrap();
    // Server 2 accepts
    let (mut server2_tcp_stream, _) = listening_socket_server2.accept().await.unwrap();
    let n2 = server2_tcp_stream.read(&mut buf).await.unwrap();
    assert_eq!(&buf[..n2], b"hello-2");
}
