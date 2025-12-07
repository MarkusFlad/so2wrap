//! Async-Wrapper for a Bound Socket that allows exclusive acquisition of the socket. When the BoundSocketGuard is dropped,
//! the socket is closed and a new one is created and bound to the same address.

use socket2::{Domain, Socket, Type};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};

/// Async-Wrapper for a Bound Socket that allows exclusive acquisition of the socket. Accessing the actual socket allways
/// requires acquiring a Boun via the `acquire` method. Only one BoundSocketGuard can exist at a time.
/// When the BoundSocketGuard is dropped, the socket is closed and a new one is created and bound to the same address.
#[derive(Clone)]
pub struct BoundSocket {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<State>,
    notify: Notify,
    addr: SocketAddr,
}

struct State {
    active: bool,
    socket: Option<Socket>,
}

/// Exclusive Bound Socket Guard there only can be one at a time. When dropped, the socket is closed
/// and a new one is created and bound to the same address. Waiting tasks are notified.
pub struct BoundSocketGuard {
    inner: Arc<Inner>,
    socket: Option<Socket>,
}

impl BoundSocket {
    /// Async-Constructor for BoundSocket. Binds the socket to the given address. Fails if the address is already in use.
    ///
    /// # Arguments
    /// * `addr` - The address to bind the socket to.
    ///
    /// # Returns
    /// * `std::io::Result<BoundSocket>` - The created BoundSocket or an error.
    ///
    /// # Errors
    /// * Returns an error if the socket could not be created or bound.
    /// ```
    pub async fn new(addr: SocketAddr) -> std::io::Result<Self> {
        let socket = Self::create_bound_socket(addr)?;

        Ok(Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    active: false,
                    socket: Some(socket),
                }),
                notify: Notify::new(),
                addr,
            }),
        })
    }

    /// Wartet ASYNC, bis dieser Wrapper exklusiver Primary wird
    pub async fn acquire(&self) -> std::io::Result<BoundSocketGuard> {
        loop {
            let mut guard = self.inner.state.lock().await;

            if !guard.active {
                guard.active = true;
                let socket = guard.socket.take().expect("Socket fehlt");

                return Ok(BoundSocketGuard {
                    inner: self.inner.clone(),
                    socket: Some(socket),
                });
            }

            drop(guard);
            self.inner.notify.notified().await;
        }
    }

    fn create_bound_socket(addr: SocketAddr) -> std::io::Result<Socket> {
        let socket = Socket::new(Domain::for_address(addr), Type::STREAM, None)?;
        socket.set_reuse_address(true)?;
        socket.bind(&addr.into())?;
        Ok(socket)
    }
}

impl BoundSocketGuard {
    /// Darf nur vom Primary aufgerufen werden
    pub fn listen(&self, backlog: i32) -> std::io::Result<()> {
        self.socket.as_ref().unwrap().listen(backlog)
    }

    /// Darf nur vom Primary aufgerufen werden
    pub fn accept(&self) -> std::io::Result<(Socket, SocketAddr)> {
        let (sock, addr) = self.socket.as_ref().unwrap().accept()?;
        let addr = addr.as_socket().unwrap();
        Ok((sock, addr))
    }
}

impl Drop for BoundSocketGuard {
    fn drop(&mut self) {
        let inner = self.inner.clone();

        // Drop darf keinen .await enthalten → wir starten eine Fire-&-Forget-Task
        tokio::spawn(async move {
            let mut guard = inner.state.lock().await;

            // Aktiven Socket explizit schließen
            guard.socket = None;
            guard.active = false;

            // Neuen bereits gebundenen Socket erzeugen
            if let Ok(new_socket) = BoundSocket::create_bound_socket(inner.addr) {
                guard.socket = Some(new_socket);
            }

            // GENAU EINEN Wartenden wecken
            inner.notify.notify_one();
        });
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
                let _guard = bound.acquire().await.unwrap();
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
