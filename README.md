# BoundSocket: Exclusive & Self-Healing Socket Reservation for Tokio

A robust, asynchronous wrapper around a network socket that enables **exclusive access** for listening and guarantees **continuous port reservation** through automatic re-binding.

## ✨ Features

* **Continuous Port Reservation:** Binds the socket immediately upon creation (`BoundSocket::new`), effectively "reserving" the port. If the listening functionality is temporarily dropped, the port is instantly re-bound and reserved for future use.
* **Exclusive Access Control:** Uses `tokio::sync::Mutex` and `tokio::sync::Notify` to ensure only a single task can acquire the `ListeningSocket` at any given time.
* **Self-Healing:** When the exclusive `ListeningSocket` is dropped, the underlying socket is closed and immediately re-created and re-bound to the same address in the background, ensuring the port is never released to other processes.
* **Idiomatic Tokio:** Built using modern asynchronous primitives for safe, concurrent operation.

## 💡 The Core Idea: Early Binding and Reservation

In standard applications, a socket is only bound (`bind`) and put into listening mode (`listen`) right before it's ready to accept connections. If the server temporarily stops listening or crashes, the port is freed.

The **`BoundSocket`** approach guarantees that a program **reserves a critical server port early** (e.g., during application startup) and **keeps it reserved, regardless of listening state**.

1.  **Reservation:** `BoundSocket::new(addr)` binds the socket, reserving the port.
2.  **Acquisition:** A task calls `bound_socket.listen(backlog)` to acquire the exclusive `ListeningSocket`.
3.  **Release & Re-bind:** When the `ListeningSocket` is dropped, the original port is instantly rebound by the `BoundSocket` in the background, making it immediately available for the next call to `listen()` without ever letting an external process snatch the port in between.

This ensures **continuous server port availability** for your application.

## 🚀 Usage

### 1. Setup

Add the required dependencies to your `Cargo.toml`:

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
socket2 = "0.5"