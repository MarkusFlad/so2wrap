//! # BoundSocket: Thread-Safe, Rebindable TCP Listener Wrapper
//!
//! **Async-Wrapper** for a **[[BoundSocket]]** that allows **exclusive, sequential acquisition** of the socket.
//! Accessing the actual socket always requires acquiring a **[[ListeningSocket]]**.
//!
//! **The key invariants of this design are:**
//! 1. **Exclusivity:** Only one task can hold a **[[ListeningSocket]]** at any given time.
//! 2. **Auto-Rebind:** When the **[[ListeningSocket]]** is dropped, the underlying socket is **atomically** closed and
//!    a **new one** is immediately created and bound to the same address.
//! 3. **Notification:** Waiting tasks are notified and can immediately retry acquiring the socket via **[BoundSocket::listen()]**.
//!
//! This pattern is ideal for services that need to temporarily release the listening state but must
//! **guarantee continuous reservation of the bound TCP port** while ensuring only one component can listen on it
//! at a time.

use socket2::{Domain, Socket, Type};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Mutex, Notify},
};

/// **BoundSocket: Thread-safe, Rebindable, Exclusive Socket Container**
///
/// Async-Wrapper for a **[[BoundSocket]]** that allows **exclusive acquisition**. Accessing the actual socket always
/// requires acquiring a **[[ListeningSocket]]** (via method **[BoundSocket::listen()]**).
///
/// **Only one** **[[ListeningSocket]]** can exist for a given **[[BoundSocket]]** at any time.
/// When the **[[ListeningSocket]]** is dropped, the underlying operating system socket is closed and a new one is
/// created and bound to the same address. Waiting tasks are notified and can immediately compete to acquire the
/// **[[ListeningSocket]]** via the **[BoundSocket::listen()]** method.
#[derive(Clone)]
pub struct BoundSocket {
    inner: Arc<Inner>,
}

/// **Internal State Management Structure**
struct Inner {
    /// Protects the **`State`** and ensures atomic transitions between listening and unbound states.
    state: Mutex<State>,
    /// Used to notify waiting tasks when the socket is released (i.e., when **[[ListeningSocket]]** is dropped).
    notify: Notify,
    /// The original address the socket should always be bound to.
    addr: SocketAddr,
    /// Stores the last known actual local address, useful when binding to port 0 (dynamic port allocation).
    last_local_addr: Mutex<SocketAddr>,
    /// Handle to the Tokio runtime used to spawn the async cleanup task when **[ListeningSocket::drop()]** is called.
    tokio_runtime_handle: tokio::runtime::Handle,
}

/// **Current Status and Socket Handle**
struct State {
    /// **Invariant Flag**: **`true`** if a **[[ListeningSocket]]** is currently active (acquired).
    active: bool,
    /// The underlying **`socket2::Socket`**. It is **`Some`** when available for acquisition, and **`None`** when active
    /// or being recreated.
    socket: Option<Socket>,
}

/// **Exclusive Listening Socket: The Access Guard**
///
/// This struct represents the **exclusive lease** to listen on the underlying socket.
///
/// **Invariant**: Only one **[[ListeningSocket]]** instance can exist for a **[[BoundSocket]]** at any time.
///
/// When this struct is dropped, the underlying OS socket is closed and a **new one is immediately created and bound**
/// to the original address (**`addr`** in **`Inner`**). Waiting tasks are notified.
///
/// This struct wraps a Tokio **`TcpListener`** for standard async **`accept`** operations. Use the **[BoundSocket::listen()]** method of
/// the **[[BoundSocket]]** to create an instance.
pub struct ListeningSocket {
    inner: Arc<Inner>,
    tcp_listener: TcpListener,
}

impl BoundSocket {
    /// **Asynchronous Constructor: Creates and Binds the Initial Socket**
    ///
    /// Creates a new **`socket2::Socket`**, sets standard options (**`SO_REUSEADDR`**, **`non-blocking`**), and binds it
    /// to the specified address. It also captures the current Tokio runtime handle for the **`Drop`** implementation
    /// of **[[ListeningSocket]]**.
    ///
    /// # Arguments
    /// * **`addr`** - The address (**`IP:Port`**) to bind the socket to. If the port is **`0`**, a random available port is used.
    ///
    /// # Returns
    /// * **`std::io::Result<BoundSocket>`** - The created **[[BoundSocket]]** or an error if binding fails (e.g., address already in use).
    ///
    pub async fn new(addr: SocketAddr) -> std::io::Result<Self> {
        let socket = Self::create_bound_socket(addr)?;
        // Retrieve the actual local address (important if port 0 was used)
        let local_addr = match socket.local_addr()?.as_socket() {
            Some(local_addr) => local_addr,
            None => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Failed to retrieve a valid SocketAddr after binding (non-socket address returned).",
                ));
            }
        };
        // Store the current tokio runtime handle for use in Drop impls
        let tokio_runtime_handle = tokio::runtime::Handle::current();
        Ok(Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    active: false,
                    socket: Some(socket),
                }),
                notify: Notify::new(),
                addr,
                last_local_addr: Mutex::new(local_addr),
                tokio_runtime_handle,
            }),
        })
    }

    /// **Acquire Exclusive Listening Lease**
    ///
    /// **Primary public method** to acquire the socket for listening. If another task currently holds the
    /// **[[ListeningSocket]]** lease, this method will **asynchronously wait** until the previous lease is dropped
    /// and the socket is rebound.
    ///
    /// **Atomic state transition:** When a socket is available, it is **consumed** from the internal state
    /// (**`guard.socket.take()`**) and the **`active`** flag is set to **`true`** before the guard is released.
    ///
    /// # Arguments
    /// * **`backlog`** - The maximum length to which the queue of pending connections may grow (passed to **`socket2::listen`**).
    ///
    /// # Returns
    /// * **`std::io::Result<ListeningSocket>`** - The acquired **[[ListeningSocket]]** or an error (e.g., if listening fails).
    ///
    /// # Errors
    /// * Returns an error if the underlying **`socket2::listen`** call fails.
    ///
    pub async fn listen(&self, backlog: i32) -> std::io::Result<ListeningSocket> {
        loop {
            let mut guard = self.inner.state.lock().await;

            if !guard.active {
                // Found an available, non-active socket. Acquire it.
                guard.active = true;
                match guard.socket.take() {
                    Some(socket) => {
                        // 1. Transition to listening state
                        socket.listen(backlog)?;
                        // 2. Wrap into tokio's TcpListener
                        let tcp_listener = tokio::net::TcpListener::from_std(socket.into())?;
                        // Guard is dropped automatically here, releasing the Mutex
                        return Ok(ListeningSocket {
                            inner: self.inner.clone(),
                            tcp_listener,
                        });
                    }
                    None => unreachable!(
                        "BoundSocket invariant violated: socket was not active but no socket is present. \
                         This state should only occur briefly during cleanup."
                    ),
                }
            }
            // Socket is currently active (held by another task). Release the lock and wait for notification.
            drop(guard);
            self.inner.notify.notified().await;
        }
    }

    /// **Returns the Last Known Local Address**
    ///
    /// Retrieves the last recorded actual local address of the bound socket. This is especially useful when
    /// the socket was initially bound to port **`0`**.
    ///
    /// # Returns
    /// * **`SocketAddr`** - The last known local address.
    pub async fn local_addr(&self) -> SocketAddr {
        *self.inner.last_local_addr.lock().await
    }

    /// **Internal Helper: Create and Bind a New Socket**
    ///
    /// Utility function to create, configure, and bind a new socket. It sets necessary options for Tokios's
    /// **`from_std`** conversion and reusability.
    ///
    /// **Invariant Configuration:** Sets **`set_nonblocking(true)`** which is required by **`TcpListener::from_std`**.
    ///
    /// # Arguments
    /// * **`addr`** - The address to bind the socket to.
    ///
    /// # Returns
    /// * **`std::io::Result<Socket>`** - The created and bound **`socket2::Socket`** or an error.
    ///
    /// # Errors
    /// * Returns an error if the socket could not be created, configured, or bound.
    ///
    fn create_bound_socket(addr: SocketAddr) -> std::io::Result<Socket> {
        let socket = Socket::new(Domain::for_address(addr), Type::STREAM, None)?;
        // Required for use with tokio::net::TcpListener::from_std
        socket.set_nonblocking(true)?;
        // Allows re-binding immediately after closing, crucial for the drop/rebind logic.
        socket.set_reuse_address(true)?;
        socket.bind(&addr.into())?;
        Ok(socket)
    }
}

// ---------------------------------------------------------------------------------------------------------------------

impl ListeningSocket {
    /// **Accepts an Incoming Connection**
    ///
    /// Delegates to the wrapped **`tokio::net::TcpListener::accept()`**.
    ///
    /// # Returns
    /// * **`std::io::Result<(TcpStream, SocketAddr)>`** - The accepted **`TcpStream`** and the remote address or an error.
    ///
    /// # Errors
    /// * Returns an error if the **`accept`** operation fails (e.g., due to temporary OS issues).
    ///
    pub async fn accept(&self) -> std::io::Result<(TcpStream, SocketAddr)> {
        self.tcp_listener.accept().await
    }

    /// **Asynchronous Cleanup and Socket Re-creation**
    ///
    /// This static async method performs the core logic when a **[[ListeningSocket]]** is dropped:
    /// 1. Sets **`active`** to **`false`** and clears the old socket reference.
    /// 2. Attempts to create and bind a **new** socket to the original address.
    /// 3. Updates **`last_local_addr`** if the rebind was successful (relevant for port **`0`**).
    /// 4. Notifies one waiting task via **`notify_one()`**.
    async fn cleanup(inner: Arc<Inner>) {
        let mut guard = inner.state.lock().await;
        // 1. Release the lease and clear the old socket reference.
        guard.socket = None;
        guard.active = false;

        // 2. Attempt to create and bind a new socket immediately.
        if let Ok(new_bound_socket) = BoundSocket::create_bound_socket(inner.addr) {
            // 3. Update local address before storing the new socket.
            if let Ok(local_sock_addr) = new_bound_socket.local_addr() {
                if let Some(local_socket_addr) = local_sock_addr.as_socket() {
                    let mut addr_guard = inner.last_local_addr.lock().await;
                    *addr_guard = local_socket_addr;
                }
            }
            guard.socket = Some(new_bound_socket);
        }
        // Lock is released here.

        // 4. Notify one waiting task to retry the listen() loop.
        inner.notify.notify_one();
    }
}

// ---------------------------------------------------------------------------------------------------------------------

impl Drop for ListeningSocket {
    /// **Drop Handler: Triggers Asynchronous Re-Binding**
    ///
    /// When the **[[ListeningSocket]]** is dropped, the underlying lock must be released, the old socket destroyed,
    /// and a new one created. Since these operations are fallible and require **`await`**, they must be performed
    /// on a Tokio runtime thread.
    ///
    /// This method uses the stored **`tokio_runtime_handle`** to spawn the asynchronous **`cleanup`** operation,
    /// ensuring the socket is re-created without blocking the synchronous **`drop`** call.
    fn drop(&mut self) {
        let inner = self.inner.clone();

        self.inner.tokio_runtime_handle.spawn(async move {
            Self::cleanup(inner).await;
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