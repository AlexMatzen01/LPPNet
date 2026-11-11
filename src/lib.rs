use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::Mutex;
use std::time::Duration;

const LOCALHOST: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);
const SEND_RECV_TIMEOUT_MS: u64 = 5000;

enum SocketEntry {
    TcpStream(TcpStream),
    TcpListener(TcpListener),
    UdpSocket(UdpSocket),
}

static SOCKETS: Lazy<Mutex<HashMap<i64, SocketEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static NEXT_FD: Lazy<Mutex<i64>> = Lazy::new(|| Mutex::new(1));
static LAST_ERROR: Lazy<Mutex<i64>> = Lazy::new(|| Mutex::new(0));

fn set_last_error(code: i32) {
    *LAST_ERROR.lock().unwrap() = code as i64;
}

fn next_fd() -> i64 {
    let mut guard = NEXT_FD.lock().unwrap();
    let fd = *guard;
    *guard = fd.wrapping_add(1);
    if fd == 0 {
        *guard = 1;
    }
    fd
}

fn addr_for_port(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(LOCALHOST), port)
}

fn entry_as_tcp_stream_mut(entry: &mut SocketEntry) -> Option<&mut TcpStream> {
    match entry {
        SocketEntry::TcpStream(s) => Some(s),
        _ => None,
    }
}

fn entry_as_tcp_listener(entry: &SocketEntry) -> Option<&TcpListener> {
    match entry {
        SocketEntry::TcpListener(l) => Some(l),
        _ => None,
    }
}

fn entry_as_udp_socket_mut(entry: &mut SocketEntry) -> Option<&mut UdpSocket> {
    match entry {
        SocketEntry::UdpSocket(s) => Some(s),
        _ => None,
    }
}

////////////////////////////////////////////////////////////////////////////////
// TCP
////////////////////////////////////////////////////////////////////////////////

/// Create a TCP connection to 127.0.0.1:<port>. Returns a socket fd or -1.
#[no_mangle]
pub extern "C" fn net_tcp_connect(
    a0: i64,
    _a1: i64,
    _a2: i64,
    _a3: i64,
    _a4: i64,
    _a5: i64,
    _a6: i64,
    _a7: i64,
) -> i64 {
    let port = match u16::try_from(a0) {
        Ok(p) => p,
        Err(_) => {
            set_last_error(22);
            return -1;
        }
    };
    let addr = addr_for_port(port);
    match TcpStream::connect(&addr) {
        Ok(stream) => {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(SEND_RECV_TIMEOUT_MS)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(SEND_RECV_TIMEOUT_MS)));
            let fd = next_fd();
            SOCKETS
                .lock()
                .unwrap()
                .insert(fd, SocketEntry::TcpStream(stream));
            set_last_error(0);
            fd
        }
        Err(e) => {
            set_last_error(e.raw_os_error().unwrap_or(-1));
            -1
        }
    }
}

/// Bind and listen on 127.0.0.1:<port>. Returns a listener fd or -1.
#[no_mangle]
pub extern "C" fn net_tcp_bind(
    a0: i64,
    _a1: i64,
    _a2: i64,
    _a3: i64,
    _a4: i64,
    _a5: i64,
    _a6: i64,
    _a7: i64,
) -> i64 {
    let port = match u16::try_from(a0) {
        Ok(p) => p,
        Err(_) => {
            set_last_error(22);
            return -1;
        }
    };
    let addr = addr_for_port(port);
    match TcpListener::bind(&addr) {
        Ok(listener) => {
            let fd = next_fd();
            SOCKETS
                .lock()
                .unwrap()
                .insert(fd, SocketEntry::TcpListener(listener));
            set_last_error(0);
            fd
        }
        Err(e) => {
            set_last_error(e.raw_os_error().unwrap_or(-1));
            -1
        }
    }
}

/// Accept an incoming connection on a listener fd. Returns a new stream fd or -1.
#[no_mangle]
pub extern "C" fn net_tcp_accept(
    a0: i64,
    _a1: i64,
    _a2: i64,
    _a3: i64,
    _a4: i64,
    _a5: i64,
    _a6: i64,
    _a7: i64,
) -> i64 {
    let mut sockets = SOCKETS.lock().unwrap();
    let entry = match sockets.get_mut(&a0) {
        Some(e) => e,
        None => {
            set_last_error(9);
            return -1;
        }
    };
    let listener = match entry_as_tcp_listener(entry) {
        Some(l) => l,
        None => {
            set_last_error(22);
            return -1;
        }
    };
    match listener.accept() {
        Ok((stream, _)) => {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(SEND_RECV_TIMEOUT_MS)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(SEND_RECV_TIMEOUT_MS)));
            let fd = next_fd();
            sockets.insert(fd, SocketEntry::TcpStream(stream));
            set_last_error(0);
            fd
        }
        Err(e) => {
            set_last_error(e.raw_os_error().unwrap_or(-1));
            -1
        }
    }
}

/// Send one byte (low 8 bits of `data`) on a TCP stream fd. Returns bytes sent or -1.
#[no_mangle]
pub extern "C" fn net_tcp_send(
    a0: i64,
    a1: i64,
    _a2: i64,
    _a3: i64,
    _a4: i64,
    _a5: i64,
    _a6: i64,
    _a7: i64,
) -> i64 {
    let mut sockets = SOCKETS.lock().unwrap();
    let entry = match sockets.get_mut(&a0) {
        Some(e) => e,
        None => {
            set_last_error(9);
            return -1;
        }
    };
    let stream = match entry_as_tcp_stream_mut(entry) {
        Some(s) => s,
        None => {
            set_last_error(22);
            return -1;
        }
    };
    let byte = (a1 as u8) as u8;
    match stream.write(&[byte]) {
        Ok(n) => {
            set_last_error(0);
            n as i64
        }
        Err(e) => {
            set_last_error(e.raw_os_error().unwrap_or(-1));
            -1
        }
    }
}

/// Receive one byte from a TCP stream fd. Returns the byte (0-255) or -1 on error/EOF.
#[no_mangle]
pub extern "C" fn net_tcp_receive(
    a0: i64,
    _a1: i64,
    _a2: i64,
    _a3: i64,
    _a4: i64,
    _a5: i64,
    _a6: i64,
    _a7: i64,
) -> i64 {
    let mut sockets = SOCKETS.lock().unwrap();
    let entry = match sockets.get_mut(&a0) {
        Some(e) => e,
        None => {
            set_last_error(9);
            return -1;
        }
    };
    let stream = match entry_as_tcp_stream_mut(entry) {
        Some(s) => s,
        None => {
            set_last_error(22);
            return -1;
        }
    };
    let mut buf = [0u8; 1];
    match stream.read(&mut buf) {
        Ok(0) => {
            // EOF / connection closed
            set_last_error(0);
            -1
        }
        Ok(_) => {
            set_last_error(0);
            buf[0] as i64
        }
        Err(e) => {
            set_last_error(e.raw_os_error().unwrap_or(-1));
            -1
        }
    }
}

/// Close a TCP socket fd. Returns true on success.
#[no_mangle]
pub extern "C" fn net_tcp_close(
    a0: i64,
    _a1: i64,
    _a2: i64,
    _a3: i64,
    _a4: i64,
    _a5: i64,
    _a6: i64,
    _a7: i64,
) -> i64 {
    let removed = SOCKETS.lock().unwrap().remove(&a0).is_some();
    set_last_error(0);
    if removed {
        1
    } else {
        0
    }
}

////////////////////////////////////////////////////////////////////////////////
// UDP
////////////////////////////////////////////////////////////////////////////////

/// Bind a UDP socket to 127.0.0.1:<port>. Returns a socket fd or -1.
#[no_mangle]
pub extern "C" fn net_udp_bind(
    a0: i64,
    _a1: i64,
    _a2: i64,
    _a3: i64,
    _a4: i64,
    _a5: i64,
    _a6: i64,
    _a7: i64,
) -> i64 {
    let port = match u16::try_from(a0) {
        Ok(p) => p,
        Err(_) => {
            set_last_error(22);
            return -1;
        }
    };
    let addr = addr_for_port(port);
    match UdpSocket::bind(&addr) {
        Ok(socket) => {
            let _ = socket.set_read_timeout(Some(Duration::from_millis(SEND_RECV_TIMEOUT_MS)));
            let fd = next_fd();
            SOCKETS
                .lock()
                .unwrap()
                .insert(fd, SocketEntry::UdpSocket(socket));
            set_last_error(0);
            fd
        }
        Err(e) => {
            set_last_error(e.raw_os_error().unwrap_or(-1));
            -1
        }
    }
}

/// Send one byte (low 8 bits of `data`) via UDP to 127.0.0.1:<port>. Returns bytes sent or -1.
#[no_mangle]
pub extern "C" fn net_udp_send(
    a0: i64,
    a1: i64,
    a2: i64,
    _a3: i64,
    _a4: i64,
    _a5: i64,
    _a6: i64,
    _a7: i64,
) -> i64 {
    let port = match u16::try_from(a2) {
        Ok(p) => p,
        Err(_) => {
            set_last_error(22);
            return -1;
        }
    };
    let addr = addr_for_port(port);
    let mut sockets = SOCKETS.lock().unwrap();
    let entry = match sockets.get_mut(&a0) {
        Some(e) => e,
        None => {
            set_last_error(9);
            return -1;
        }
    };
    let socket = match entry_as_udp_socket_mut(entry) {
        Some(s) => s,
        None => {
            set_last_error(22);
            return -1;
        }
    };
    let byte = (a1 as u8) as u8;
    match socket.send_to(&[byte], &addr) {
        Ok(n) => {
            set_last_error(0);
            n as i64
        }
        Err(e) => {
            set_last_error(e.raw_os_error().unwrap_or(-1));
            -1
        }
    }
}

/// Receive one byte via UDP. Returns the byte (0-255) or -1 on error/timeout.
#[no_mangle]
pub extern "C" fn net_udp_receive(
    a0: i64,
    _a1: i64,
    _a2: i64,
    _a3: i64,
    _a4: i64,
    _a5: i64,
    _a6: i64,
    _a7: i64,
) -> i64 {
    let mut sockets = SOCKETS.lock().unwrap();
    let entry = match sockets.get_mut(&a0) {
        Some(e) => e,
        None => {
            set_last_error(9);
            return -1;
        }
    };
    let socket = match entry_as_udp_socket_mut(entry) {
        Some(s) => s,
        None => {
            set_last_error(22);
            return -1;
        }
    };
    let mut buf = [0u8; 1];
    match socket.recv_from(&mut buf) {
        Ok((n, _)) => {
            if n == 0 {
                set_last_error(0);
                return -1;
            }
            set_last_error(0);
            buf[0] as i64
        }
        Err(e) => {
            set_last_error(e.raw_os_error().unwrap_or(-1));
            -1
        }
    }
}

/// Close a UDP socket fd. Returns true on success.
#[no_mangle]
pub extern "C" fn net_udp_close(
    a0: i64,
    _a1: i64,
    _a2: i64,
    _a3: i64,
    _a4: i64,
    _a5: i64,
    _a6: i64,
    _a7: i64,
) -> i64 {
    let removed = SOCKETS.lock().unwrap().remove(&a0).is_some();
    set_last_error(0);
    if removed {
        1
    } else {
        0
    }
}

////////////////////////////////////////////////////////////////////////////////
// General
////////////////////////////////////////////////////////////////////////////////

/// Return the last error code (raw OS error or -1).
#[no_mangle]
pub extern "C" fn net_last_error(
    _a0: i64,
    _a1: i64,
    _a2: i64,
    _a3: i64,
    _a4: i64,
    _a5: i64,
    _a6: i64,
    _a7: i64,
) -> i64 {
    *LAST_ERROR.lock().unwrap()
}
