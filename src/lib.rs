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

/// Cached handles to lppo's exported buffer accessor functions (resolved from
/// the main executable at first use).
struct BufferAccessors {
    buf_get: unsafe extern "C" fn(i64, *mut u8, i64) -> i64,
    buf_set: unsafe extern "C" fn(i64, *const u8, i64) -> i64,
    buf_len: unsafe extern "C" fn(i64) -> i64,
    buf_new: unsafe extern "C" fn() -> i64,
    buf_clear: unsafe extern "C" fn(i64) -> i64,
    buf_append: unsafe extern "C" fn(i64, *const u8, i64) -> i64,
}

unsafe impl Send for BufferAccessors {}
unsafe impl Sync for BufferAccessors {}

static BUFFER_ACCESSORS: Lazy<Mutex<Option<&'static BufferAccessors>>> =
    Lazy::new(|| Mutex::new(None));

fn buffer_accessors() -> LppResult<&'static BufferAccessors> {
    let mut guard = BUFFER_ACCESSORS.lock().unwrap();
    if let Some(acc) = guard.as_ref() {
        return Ok(*acc);
    }
    // The main executable exports lppo_buf_* symbols. Open /proc/self/exe to
    // resolve them (dlopen(NULL) equivalent on Linux).
    let exe = std::ffi::OsStr::new("");
    unsafe {
        let lib = libloading::Library::new(exe).map_err(|e| {
            set_last_error(1);
            LppError::ffi(format!("failed to open main executable: {e}"))
        })?;
        let buf_get: libloading::Symbol<'_, unsafe extern "C" fn(i64, *mut u8, i64) -> i64> =
            lib.get(b"lppo_buf_get\0").map_err(|e| {
                set_last_error(1);
                LppError::ffi(format!("failed to resolve lppo_buf_get: {e}"))
            })?;
        let buf_set: libloading::Symbol<'_, unsafe extern "C" fn(i64, *const u8, i64) -> i64> =
            lib.get(b"lppo_buf_set\0").map_err(|e| {
                set_last_error(1);
                LppError::ffi(format!("failed to resolve lppo_buf_set: {e}"))
            })?;
        let buf_len: libloading::Symbol<'_, unsafe extern "C" fn(i64) -> i64> =
            lib.get(b"lppo_buf_len\0").map_err(|e| {
                set_last_error(1);
                LppError::ffi(format!("failed to resolve lppo_buf_len: {e}"))
            })?;
        let buf_new: libloading::Symbol<'_, unsafe extern "C" fn() -> i64> =
            lib.get(b"lppo_buf_new\0").map_err(|e| {
                set_last_error(1);
                LppError::ffi(format!("failed to resolve lppo_buf_new: {e}"))
            })?;
        let buf_clear: libloading::Symbol<'_, unsafe extern "C" fn(i64) -> i64> =
            lib.get(b"lppo_buf_clear\0").map_err(|e| {
                set_last_error(1);
                LppError::ffi(format!("failed to resolve lppo_buf_clear: {e}"))
            })?;
        let buf_append: libloading::Symbol<'_, unsafe extern "C" fn(i64, *const u8, i64) -> i64> =
            lib.get(b"lppo_buf_append\0").map_err(|e| {
                set_last_error(1);
                LppError::ffi(format!("failed to resolve lppo_buf_append: {e}"))
            })?;
        let accessors = Box::leak(Box::new(BufferAccessors {
            buf_get: *buf_get,
            buf_set: *buf_set,
            buf_len: *buf_len,
            buf_new: *buf_new,
            buf_clear: *buf_clear,
            buf_append: *buf_append,
        }));
        // `lib` must outlive the symbols; leak it too.
        Box::leak(Box::new(lib));
        *guard = Some(accessors);
        Ok(accessors)
    }
}

#[derive(Debug)]
enum LppError {
    ffi(String),
}

type LppResult<T> = Result<T, LppError>;

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
// Buffer helpers (read/write lppo string buffers)
////////////////////////////////////////////////////////////////////////////////

/// Read an lppo buffer into a Vec<u8>. Returns None if the buffer id is unknown.
fn buffer_get(id: i64, max_len: usize) -> Option<Vec<u8>> {
    let acc = buffer_accessors().ok()?;
    let len = unsafe { (acc.buf_len)(id) };
    if len < 0 {
        return None;
    }
    let len = len as usize;
    let cap = len.min(max_len);
    let mut vec = Vec::with_capacity(cap);
    let written = unsafe { (acc.buf_get)(id, vec.as_mut_ptr(), cap as i64) };
    if written < 0 {
        return None;
    }
    unsafe { vec.set_len(written as usize) };
    Some(vec)
}

/// Write a byte slice into an lppo buffer (overwriting). Returns true on success.
fn buffer_set(id: i64, data: &[u8]) -> bool {
    let Ok(acc) = buffer_accessors() else {
        return false;
    };
    let s = std::str::from_utf8(data).unwrap_or("");
    let result = unsafe { (acc.buf_set)(id, s.as_ptr(), s.len() as i64) };
    result == 1
}

/// Append a byte slice to an lppo buffer. Returns true on success.
fn buffer_append(id: i64, data: &[u8]) -> bool {
    let Ok(acc) = buffer_accessors() else {
        return false;
    };
    let s = std::str::from_utf8(data).unwrap_or("");
    let result = unsafe { (acc.buf_append)(id, s.as_ptr(), s.len() as i64) };
    result == 1
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

/// Send an lppo buffer's contents over a TCP stream fd. Returns bytes sent or -1.
#[no_mangle]
pub extern "C" fn net_tcp_send_buf(
    a0: i64,
    a1: i64,
    _a2: i64,
    _a3: i64,
    _a4: i64,
    _a5: i64,
    _a6: i64,
    _a7: i64,
) -> i64 {
    let data = match buffer_get(a1, 65536) {
        Some(d) => d,
        None => {
            set_last_error(22);
            return -1;
        }
    };
    if data.is_empty() {
        set_last_error(0);
        return 0;
    }
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
    match stream.write_all(&data) {
        Ok(_) => {
            set_last_error(0);
            data.len() as i64
        }
        Err(e) => {
            set_last_error(e.raw_os_error().unwrap_or(-1));
            -1
        }
    }
}

/// Receive from a TCP stream fd into an lppo buffer (overwriting). Returns bytes received or -1.
#[no_mangle]
pub extern "C" fn net_tcp_receive_buf(
    a0: i64,
    a1: i64,
    a2: i64,
    _a3: i64,
    _a4: i64,
    _a5: i64,
    _a6: i64,
    _a7: i64,
) -> i64 {
    let max_len = a2 as usize;
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
    let mut buf = vec![0u8; max_len.max(1)];
    match stream.read(&mut buf) {
        Ok(0) => {
            set_last_error(0);
            -1
        }
        Ok(n) => {
            buf.truncate(n);
            let _ = buffer_set(a1, &buf);
            set_last_error(0);
            n as i64
        }
        Err(e) => {
            set_last_error(e.raw_os_error().unwrap_or(-1));
            -1
        }
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

/// Send an lppo buffer's contents via UDP to 127.0.0.1:<port>. Returns bytes sent or -1.
#[no_mangle]
pub extern "C" fn net_udp_send_buf(
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
    let data = match buffer_get(a1, 65536) {
        Some(d) => d,
        None => {
            set_last_error(22);
            return -1;
        }
    };
    if data.is_empty() {
        set_last_error(0);
        return 0;
    }
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
    match socket.send_to(&data, &addr) {
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

/// Receive via UDP into an lppo buffer (overwriting). Returns bytes received or -1.
#[no_mangle]
pub extern "C" fn net_udp_receive_buf(
    a0: i64,
    a1: i64,
    a2: i64,
    _a3: i64,
    _a4: i64,
    _a5: i64,
    _a6: i64,
    _a7: i64,
) -> i64 {
    let max_len = a2 as usize;
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
    let mut buf = vec![0u8; max_len.max(1)];
    match socket.recv_from(&mut buf) {
        Ok((n, _)) => {
            if n == 0 {
                set_last_error(0);
                return -1;
            }
            buf.truncate(n);
            let _ = buffer_set(a1, &buf);
            set_last_error(0);
            n as i64
        }
        Err(e) => {
            set_last_error(e.raw_os_error().unwrap_or(-1));
            -1
        }
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
