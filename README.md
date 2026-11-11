# LPPNet - Networking Package for LPPO

LPPNet is a networking (TCP/UDP) package for the [LPPO](https://github.com/lppo-lang/lppo)
educational programming language. It provides LPPO programs with the ability to
create TCP connections, run TCP servers, and send/receive UDP datagrams over
localhost.

## Installation

Add the local repository to LPPO and install:

```bash
lppo pkg repo add local /home/alex/Documents/LPPNet path
lppo pkg install lppnet
```

Or build directly:

```bash
cargo build --release
```

## Package Manifest

- **Identifier:** `lppnet`
- **Version:** `1.0.0`
- **LPPO version:** `>=5.0.0`
- **Build:** `cdylib` (loaded via FFI by the LPPO interpreter)

## API

All functions communicate over `127.0.0.1` (localhost). Socket file descriptors
are `Int64` values returned by `connect`/`bind`. Data is sent and received one
byte at a time as `Int64` (0-255).

### TCP

| Function | Signature | Description |
|---|---|---|
| `net_tcp_connect` | `(port: Int64) -> Int64` | Connect to `127.0.0.1:port`. Returns socket fd or `-1`. |
| `net_tcp_bind` | `(port: Int64) -> Int64` | Bind and listen on `127.0.0.1:port`. Returns listener fd or `-1`. |
| `net_tcp_accept` | `(fd: Int64) -> Int64` | Accept a connection on a listener. Returns new stream fd or `-1`. |
| `net_tcp_send` | `(fd: Int64, data: Int64) -> Int64` | Send one byte (low 8 bits of `data`). Returns bytes sent or `-1`. |
| `net_tcp_receive` | `(fd: Int64) -> Int64` | Receive one byte. Returns byte (0-255) or `-1` on EOF/error. |
| `net_tcp_close` | `(fd: Int64) -> Bool` | Close a TCP socket. |

### UDP

| Function | Signature | Description |
|---|---|---|
| `net_udp_bind` | `(port: Int64) -> Int64` | Bind a UDP socket to `127.0.0.1:port`. Returns fd or `-1`. |
| `net_udp_send` | `(fd: Int64, port: Int64, data: Int64) -> Int64` | Send one byte to `127.0.0.1:port`. Returns bytes sent or `-1`. |
| `net_udp_receive` | `(fd: Int64) -> Int64` | Receive one byte. Returns byte (0-255) or `-1` on error/timeout. |
| `net_udp_close` | `(fd: Int64) -> Bool` | Close a UDP socket. |

### General

| Function | Signature | Description |
|---|---|---|
| `net_last_error` | `() -> Int64` | Return the last OS error code (or `-1`). |

## Example

```lppo
use lppnet;

pub proc main() -> Void {
    let own server_fd: Int64 = net_tcp_bind(9876);
    if server_fd < 0 {
        out "failed to bind";
        return;
    }
    out "listening on 9876";

    let own client_fd: Int64 = net_tcp_accept(server_fd);
    if client_fd < 0 {
        out "failed to accept";
        return;
    }
    out "client connected";

    let own byte: Int64 = net_tcp_receive(client_fd);
    out "received:", byte;

    let own sent: Int64 = net_tcp_send(client_fd, 65);
    out "sent:", sent;

    let own ok: Bool = net_tcp_close(client_fd);
    out "closed client:", ok;

    let own ok2: Bool = net_tcp_close(server_fd);
    out "closed server:", ok2;
}
```

## Notes

- All connections are limited to `127.0.0.1` (localhost).
- Only one byte can be sent or received per call. This is a limitation of the
  LPPO native FFI, which passes integer values only (strings cannot be passed
  through the FFI boundary).
- Sockets are tracked internally and closed when `net_tcp_close` /
  `net_udp_close` are called, or when the process exits.

## License

MIT
