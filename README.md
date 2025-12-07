[![API Docs](https://img.shields.io/badge/API%20Docs-Rustdoc-blue)](https://markusflad.github.io/so2wrap/so2wrap/)

# BoundSocket: Exclusive & Self-Healing Socket Reservation for Tokio

A robust, asynchronous wrapper around a TCP server socket that enables **exclusive access** for listening and guarantees **continuous port reservation** through automatic re-binding.

## Features

* **Continuous Port Reservation:** Binds the socket immediately upon creation (`BoundSocket::new`), effectively "reserving" the port. If the listening functionality is temporarily dropped, the port is instantly re-bound and reserved for future use.
* **Exclusive Access Control:** Uses `tokio::sync::Mutex` and `tokio::sync::Notify` to ensure only a single task can acquire the `ListeningSocket` at any given time.
* **Self-Healing:** When the exclusive `ListeningSocket` is dropped, the underlying socket is closed and immediately re-created and re-bound to the same address in the background, ensuring the port is never released to other processes.
* **Idiomatic Tokio:** Built using modern asynchronous primitives for safe, concurrent operation.

## The Core Idea: Early Binding and Reservation

In standard applications, a socket is only bound (`bind`) and put into listening mode (`listen`) right before it's ready to accept connections. If the server temporarily stops listening or crashes, the port is freed.

The **`BoundSocket`** approach guarantees that a program **reserves a critical server port early** (e.g., during application startup) and **keeps it reserved, regardless of listening state**.

1.  **Reservation:** `BoundSocket::new(addr)` binds the socket, reserving the port.
2.  **Acquisition:** A task calls `bound_socket.listen(backlog)` to acquire the exclusive `ListeningSocket`.
3.  **Release & Re-bind:** When the `ListeningSocket` is dropped, the original port is instantly rebound by the `BoundSocket` in the background, making it immediately available for the next call to `listen()` without ever letting an external process snatch the port in between.

This ensures **continuous server port availability** for your application.

## Examples

The following examples demonstrate how to use the `BoundSocket` to implement a simple TCP server that processes one connection at a time.

### Server Example (`server.rs`)

This loop illustrates the **core principle** of exclusive access and re-binding:

1.  It calls `bound_socket.listen(128).await?` to **acquire the exclusive `ListeningSocket`**. Other callers to `listen()` would block here.
2.  It accepts exactly **one client** (`listening_socket.accept().await?`).
3.  The main loop scope ends, and the `listening_socket` is **dropped**.
4.  The $\text{Drop}$ implementation immediately **re-binds the socket** in the background (ensuring that the port is reserved) and notifies the next waiting task (or allows the loop to continue its next iteration).

### Client Example (client.rs)

A simple tokio::net::TcpStream client used to test the server. It connects, sends a message, reads the response, and closes. This client is used to trigger the accept call in the server example.

## Documentation

The complete API documentation for `so2wrap` can be found here:

[**Rustdoc API Documentation**](https://markusflad.github.io/so2wrap/so2wrap/)

## Usage

Add the required dependencies to your `Cargo.toml`:

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
socket2 = "0.5"