use std::io;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use tokio::signal;

/// Maximum number of consecutive ports to try before giving up.
const MAX_PORT_ATTEMPTS: u16 = 100;

/// Minimal server configuration (extended by later issues).
pub struct AppConfig;

/// Shared application state passed to all request handlers via `Arc<AppState>`.
pub struct AppState {
    /// Base directory from which markdown files and assets are served.
    pub serve_root: PathBuf,
    /// The primary markdown entry file.
    pub entry_file: PathBuf,
    /// Server configuration.
    pub config: AppConfig,
}

/// Attempt to bind a TCP listener on `bind_addr` starting at `start_port`.
///
/// On `EADDRINUSE` the port is incremented by one and the attempt is retried up
/// to `MAX_PORT_ATTEMPTS` times.  Any other OS error causes an immediate failure
/// without further retries.
///
/// Returns the bound `TcpListener` and the actual port on success, or a
/// descriptive `String` error on failure.
pub fn bind_with_retry(bind_addr: &str, start_port: u16) -> Result<(TcpListener, u16), String> {
    let mut port = start_port;
    eprintln!("[bind] trying port={}", port);
    for _ in 0..MAX_PORT_ATTEMPTS {
        let addr = format!("{}:{}", bind_addr, port);
        match TcpListener::bind(&addr) {
            Ok(listener) => {
                eprintln!("[bind] success port={}", port);
                return Ok((listener, port));
            }
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                let next = port.wrapping_add(1);
                eprintln!("[bind] EADDRINUSE, trying {}", next);
                port = next;
            }
            Err(e) => {
                return Err(format!("bind {}:{} failed: {}", bind_addr, port, e));
            }
        }
    }
    Err(format!(
        "exhausted {} port candidates starting at {}; all ports in use",
        MAX_PORT_ATTEMPTS, start_port,
    ))
}

/// Start the HTTP server for the given markdown `file`.
///
/// Binds to `bind_addr` starting at `start_port`, retrying on `EADDRINUSE` up
/// to 100 times.  The server shuts down cleanly when SIGINT (Ctrl+C) is
/// received.
pub async fn run_serve(file: String, bind_addr: String, start_port: u16) -> io::Result<()> {
    let entry_file = std::fs::canonicalize(&file).unwrap_or_else(|_| PathBuf::from(&file));
    let serve_root = entry_file
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let state = Arc::new(AppState {
        serve_root,
        entry_file,
        config: AppConfig,
    });

    let (std_listener, bound_port) =
        bind_with_retry(&bind_addr, start_port).map_err(|msg| {
            eprintln!("Error: {}", msg);
            io::Error::new(io::ErrorKind::AddrInUse, msg)
        })?;

    std_listener.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(std_listener)?;

    let app = Router::new().with_state(state);

    eprintln!("[serve] listening on {}:{}", bind_addr, bound_port);

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            signal::ctrl_c()
                .await
                .expect("failed to install SIGINT handler");
            eprintln!("[shutdown] complete");
        })
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    Ok(())
}
