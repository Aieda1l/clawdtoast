//! Daemon <-> shim transport. Unix: a socket file in the support dir. Windows: a loopback TCP
//! listener on an ephemeral port, whose number is written to the same path (std has no Unix
//! sockets on Windows).

#[cfg(unix)]
mod imp {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    pub use std::os::unix::net::{UnixListener as Listener, UnixStream as Stream};

    pub fn bind(path: &Path) -> std::io::Result<Listener> {
        let l = Listener::bind(path)?;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        Ok(l)
    }

    pub fn connect(path: &Path) -> std::io::Result<Stream> {
        Stream::connect(path)
    }

    pub fn secure_dir(dir: &Path) {
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
}

#[cfg(windows)]
mod imp {
    use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
    use std::path::Path;
    pub use std::net::{TcpListener as Listener, TcpStream as Stream};

    pub fn bind(path: &Path) -> std::io::Result<Listener> {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        std::fs::write(path, l.local_addr()?.port().to_string())?;
        Ok(l)
    }

    pub fn connect(path: &Path) -> std::io::Result<Stream> {
        let port: u16 = std::fs::read_to_string(path)?
            .trim()
            .parse()
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad port file"))?;
        TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), std::time::Duration::from_millis(200))
    }

    /// The support dir lives under the user's profile, which is already private to them.
    pub fn secure_dir(_dir: &Path) {}
}

pub use imp::{bind, connect, secure_dir, Listener};
