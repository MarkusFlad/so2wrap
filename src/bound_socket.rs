//! Async-Wrapper for a Bound Socket that allows exclusive acquisition of the socket. When the BoundSocketGuard is dropped,
//! the socket is closed and a new one is created and bound to the same address.

use socket2::{Domain, Socket, Type};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Mutex, Notify},
};

/// Async-Wrapper for a Bound Socket that allows exclusive acquisition of the socket. Accessing the actual socket allways
/// requires acquiring a Listening Socket. When the Listening Socket is dropped, the socket is closed and a new one is created
/// and bound to the same address. Waiting tasks are notified.
#[derive(Clone)]
pub struct BoundSocket {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<State>,
    notify: Notify,
    addr: SocketAddr,
    last_local_addr: Mutex<SocketAddr>,
}

struct State {
    active: bool,
    socket: Option<Socket>,
}

/// Exclusive Listening Socket. There only can be one at a time for one Bound Socket. When dropped, the socket is closed
/// and a new one is created and bound to the same address. Waiting tasks are notified.
pub struct ListeningSocket {
    inner: Arc<Inner>,
    tcp_listener: TcpListener,
}

impl BoundSocket {
    /// Async-Constructor for BoundSocket. Binds the socket to the given address. Fails if the address is already in use.
    /// Blocks until the socket is successfully created and bound.
    ///
    /// # Arguments
    /// * `addr` - The address to bind the socket to.
    ///
    /// # Returns
    /// * `std::io::Result<BoundSocket>` - The created BoundSocket or an error.
    /// * Returns an error if the socket could not be created or bound.
    ///
    pub async fn new(addr: SocketAddr) -> std::io::Result<Self> {
        let socket = Self::create_bound_socket(addr)?;
        let local_addr = match socket.local_addr()?.as_socket() {
            Some(local_addr) => local_addr,
            None => unreachable!("Socket2 socket has no SocketAddr after binding."),
        };
        Ok(Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    active: false,
                    socket: Some(socket),
                }),
                notify: Notify::new(),
                addr,
                last_local_addr: Mutex::new(local_addr),
            }),
        })
    }
    /// Acquires the BoundSocket for listening. If another task has already acquired the socket, this method
    /// will wait until the socket is released. When the returned ListeningSocket is dropped, the socket is closed
    /// and a new one is created and bound to the same address. Waiting tasks are notified.
    ///
    /// # Arguments
    /// * `backlog` - The backlog for the listening socket.
    ///
    /// # Returns
    /// * `std::io::Result<ListeningSocket>` - The acquired ListeningSocket or an error.
    ///
    /// # Errors
    /// * Returns an error if the socket could not be listened on.
    ///
    pub async fn listen(&self, backlog: i32) -> std::io::Result<ListeningSocket> {
        loop {
            let mut guard = self.inner.state.lock().await;

            if !guard.active {
                guard.active = true;
                match guard.socket.take() {
                    Some(socket) => {
                        socket.listen(backlog)?;
                        let tcp_listener = tokio::net::TcpListener::from_std(socket.into())?;
                        return Ok(ListeningSocket {
                            inner: self.inner.clone(),
                            tcp_listener,
                        });
                    }
                    None => unreachable!(
                        "BoundSocket invariant violated: socket was not active but no socket is present."
                    ),
                }
            }
            drop(guard);
            self.inner.notify.notified().await;
        }
    }
    /// Returns the last known local address of the BoundSocket.
    ///
    /// # Returns
    /// * `SocketAddr` - The last known local address.
    pub async fn local_addr(&self) -> SocketAddr {
        *self.inner.last_local_addr.lock().await
    }
    /// Creates and binds a new socket to the given address. Used internally when creating a new BoundSocket or recreating
    /// the socket after dropping a ListeningSocket.
    ///
    /// # Arguments
    /// * `addr` - The address to bind the socket to.
    ///
    /// # Returns
    /// * `std::io::Result<Socket>` - The created and bound socket or an error.
    ///
    /// # Errors
    /// * Returns an error if the socket could not be created or bound.
    ///
    fn create_bound_socket(addr: SocketAddr) -> std::io::Result<Socket> {
        let socket = Socket::new(Domain::for_address(addr), Type::STREAM, None)?;
        // Note: set_nonblocking must be set before using in tokio (since tokio 1.37)
        socket.set_nonblocking(true)?;
        socket.set_reuse_address(true)?;
        socket.bind(&addr.into())?;
        Ok(socket)
    }
}

impl ListeningSocket {
    /// Accepts a new incoming connection on the ListeningSocket.
    ///
    /// # Returns
    /// * `std::io::Result<(TcpStream, SocketAddr)>` - The accepted TcpStream and the remote address or an error.
    ///
    /// # Errors
    /// * Returns an error if the accept operation fails.
    ///
    pub async fn accept(&self) -> std::io::Result<(TcpStream, SocketAddr)> {
        self.tcp_listener.accept().await
    }
    /// Cleans up the ListeningSocket by closing the active socket and creating a new one bound to the same address.
    /// Notifies one waiting task.
    async fn cleanup(inner: Arc<Inner>) {
        let mut guard = inner.state.lock().await;
        guard.socket = None;
        guard.active = false;

        if let Ok(new_bound_socket) = BoundSocket::create_bound_socket(inner.addr) {
            if let Ok(local_sock_addr) = new_bound_socket.local_addr() {
                if let Some(local_socket_addr) = local_sock_addr.as_socket() {
                    let mut addr_guard = inner.last_local_addr.lock().await;
                    *addr_guard = local_socket_addr;
                }
            }
            guard.socket = Some(new_bound_socket);
        }

        inner.notify.notify_one();
    }
}

impl Drop for ListeningSocket {
    /// Drops the ListeningSocket. Closes the active socket and creates a new one bound to the same address.
    /// Notifies one waiting task.
    fn drop(&mut self) {
        let inner = self.inner.clone();

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                Self::cleanup(inner).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        net::{IpAddr, Ipv4Addr},
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use tokio::time::{Duration, sleep};

    #[tokio::test]
    async fn bound_socket_allows_only_one_primary_and_reacquires_after_drop() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);
        let bound = BoundSocket::new(addr).await.unwrap();
        let active_counter = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));
        let mut handles = vec![];

        for _ in 0..5 {
            let bound = bound.clone();
            let active = active_counter.clone();
            let max_seen = max_seen.clone();

            handles.push(tokio::spawn(async move {
                // All five spawned tokio tasks want to be primary
                let _guard = bound.listen(128).await.unwrap();
                // Another spawend tokio task is now primary (before adding it should be 0)
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                // Store the max simultaneous primaries seen so far (should never exceed 1)
                max_seen.fetch_max(now, Ordering::SeqCst);
                // Simulate some work
                sleep(Duration::from_millis(100)).await;
                // The spawned tokio task is done being primary
                active.fetch_sub(1, Ordering::SeqCst);
                // Drop of the BoundSocketGuard happens here
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        let max = max_seen.load(Ordering::SeqCst);
        // Check the core invariant: max simultaneous primaries is 1
        assert_eq!(max, 1, "Max simultaneous primaries was {}, expected 1", max);
    }
}
