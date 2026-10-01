//! Transports: move JSON values between a client and a [`MessageHandler`]:
//! LF-delimited JSON ([`serve_lines`], [`serve_stdio`]) and WebSocket frames
//! over a Unix socket ([`serve_unix`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

#[cfg(unix)]
use std::future::Future;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
use futures_util::{SinkExt, StreamExt};
#[cfg(unix)]
use tokio_tungstenite::tungstenite::{Error as WsError, Message as WsMessage};

/// Connection id, unique per server.
pub type ConnectionId = u64;

/// What a transport talks to. The server implements this.
pub trait MessageHandler: Send + Sync + 'static {
    /// A client connected. Messages for it arrive on the returned receiver,
    /// until the server drops the sender or [`MessageHandler::disconnect`].
    fn connect(&self) -> (ConnectionId, mpsc::UnboundedReceiver<Value>);
    /// One message from the client; `Err` = the frame was not valid JSON
    /// (the text of the parse error). Must not block.
    fn receive(&self, connection: ConnectionId, message: Result<Value, String>);
    /// The client went away.
    fn disconnect(&self, connection: ConnectionId);
}

/// Which transport the CLI was asked to serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listen {
    /// LF-delimited JSON on stdin/stdout.
    Stdio,
    /// A WebSocket served over a Unix domain socket at the given path.
    Unix(PathBuf),
}

/// Parse a listen URL (`stdio://`, `unix://`, `unix://PATH`) into a [`Listen`].
///
/// `unix://` uses `default_socket`; `unix://PATH` requires `PATH` to be
/// absolute.
pub fn parse_listen(url: &str, default_socket: &Path) -> Result<Listen, String> {
    const ACCEPTED: &str = "accepted values are `stdio://`, `unix://`, and `unix://PATH`";
    if url == "stdio://" {
        return Ok(Listen::Stdio);
    }
    if let Some(rest) = url.strip_prefix("unix://") {
        if rest.is_empty() {
            return Ok(Listen::Unix(default_socket.to_path_buf()));
        }
        let path = PathBuf::from(rest);
        if !path.is_absolute() {
            return Err(format!(
                "unix socket path must be absolute: `{rest}`; {ACCEPTED}"
            ));
        }
        return Ok(Listen::Unix(path));
    }
    Err(format!("unrecognized listen url `{url}`; {ACCEPTED}"))
}

/// Serve one line-delimited connection: read JSON lines from `reader`, feed
/// them to `handler`, and write the values it produces to `writer`.
///
/// Returns when `reader` hits EOF (the connection is then disconnected) or the
/// server drops the outgoing sender.
pub async fn serve_lines<R, W>(handler: Arc<dyn MessageHandler>, reader: R, writer: W)
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let (connection, mut outgoing) = handler.connect();
    let mut reader = BufReader::new(reader);
    let mut writer = writer;
    let mut line = String::new();

    loop {
        // `read_line` isn't cancel-safe: when the other branch wins, the bytes
        // read so far stay in `line`, so clear it only after a whole line.
        tokio::select! {
            read = reader.read_line(&mut line) => match read {
                Ok(0) => break,
                Ok(_) => {
                    let text = line.trim_end_matches(['\n', '\r']);
                    if !text.is_empty() {
                        dispatch_text(handler.as_ref(), connection, text);
                    }
                    line.clear();
                }
                Err(error) => {
                    handler.receive(connection, Err(error.to_string()));
                    break;
                }
            },
            outgoing_message = outgoing.recv() => match outgoing_message {
                Some(value) => {
                    if write_line(&mut writer, &value).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
        }
    }

    handler.disconnect(connection);
    // Short bounded drain: only whatever the server already queued (it may
    // still be holding the sender), never block waiting for more.
    while let Ok(value) = outgoing.try_recv() {
        if write_line(&mut writer, &value).await.is_err() {
            break;
        }
    }
    let _ = writer.flush().await;
}

/// Serve LF-delimited JSON on stdin/stdout.
pub async fn serve_stdio(handler: Arc<dyn MessageHandler>) {
    serve_lines(handler, tokio::io::stdin(), tokio::io::stdout()).await;
}

/// Serve a WebSocket over a Unix domain socket until `shutdown` resolves.
#[cfg(unix)]
pub async fn serve_unix(
    handler: Arc<dyn MessageHandler>,
    path: &Path,
    shutdown: impl Future<Output = ()> + Send,
) -> std::io::Result<()> {
    create_private_parent(path)?;

    if let Ok(meta) = std::fs::symlink_metadata(path) {
        use std::os::unix::fs::FileTypeExt;
        if !meta.file_type().is_socket() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{} exists and is not a socket", path.display()),
            ));
        }
        match tokio::net::UnixStream::connect(path).await {
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    format!("app-server already running at {}", path.display()),
                ));
            }
            Err(_) => {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    let listener = tokio::net::UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;

    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let handler = Arc::clone(&handler);
                    tokio::spawn(async move {
                        serve_websocket(handler, stream).await;
                    });
                }
                Err(error) => eprintln!("app-server: accept failed: {error}"),
            },
            () = &mut shutdown => break,
        }
    }

    let _ = std::fs::remove_file(path);
    Ok(())
}

/// [`serve_unix`], shut down by Ctrl-C.
#[cfg(unix)]
pub async fn serve_unix_until_ctrl_c(
    handler: Arc<dyn MessageHandler>,
    path: &Path,
) -> std::io::Result<()> {
    serve_unix(handler, path, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
}

/// Create `path`'s parent directory (mode 0700) if it does not yet exist.
#[cfg(unix)]
fn create_private_parent(path: &Path) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() || parent.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(parent)?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// Serve one WebSocket connection: each text frame is one JSON message;
/// outgoing values are sent as text frames.
#[cfg(unix)]
async fn serve_websocket(handler: Arc<dyn MessageHandler>, stream: tokio::net::UnixStream) {
    let socket = match tokio_tungstenite::accept_async(stream).await {
        Ok(socket) => socket,
        Err(_) => return,
    };
    let (mut sink, mut source) = socket.split();
    let (connection, mut outgoing) = handler.connect();

    loop {
        tokio::select! {
            incoming = source.next() => match incoming {
                Some(Ok(WsMessage::Text(text))) => {
                    dispatch_text(handler.as_ref(), connection, text.as_str());
                }
                Some(Ok(WsMessage::Binary(data))) => match std::str::from_utf8(&data) {
                    Ok(text) => dispatch_text(handler.as_ref(), connection, text),
                    Err(error) => handler.receive(connection, Err(error.to_string())),
                },
                Some(Ok(WsMessage::Close(_))) | None => break,
                Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_))) => {}
                Some(Err(_)) => break,
            },
            outgoing_message = outgoing.recv() => match outgoing_message {
                Some(value) => {
                    if send_json(&mut sink, &value).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
        }
    }

    handler.disconnect(connection);
    while let Ok(value) = outgoing.try_recv() {
        if send_json(&mut sink, &value).await.is_err() {
            break;
        }
    }
    let _ = sink.close().await;
}

/// Parse `text` as JSON and hand it to `handler`, or report the parse error.
fn dispatch_text(handler: &dyn MessageHandler, connection: ConnectionId, text: &str) {
    match serde_json::from_str::<Value>(text) {
        Ok(value) => handler.receive(connection, Ok(value)),
        Err(error) => handler.receive(connection, Err(error.to_string())),
    }
}

/// Write one JSON value as a single line and flush.
async fn write_line<W>(writer: &mut W, value: &Value) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut line = serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned());
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await
}

/// Send one JSON value as a WebSocket text frame.
#[cfg(unix)]
async fn send_json<S>(sink: &mut S, value: &Value) -> Result<(), WsError>
where
    S: futures_util::sink::Sink<WsMessage, Error = WsError> + Unpin,
{
    let text = serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned());
    sink.send(WsMessage::text(text)).await
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::mpsc;
    use tokio::sync::oneshot;

    use super::*;

    #[derive(Default)]
    struct Echo {
        connections: Mutex<HashMap<ConnectionId, mpsc::UnboundedSender<Value>>>,
        next_id: AtomicU64,
        disconnects: AtomicUsize,
    }

    impl MessageHandler for Echo {
        fn connect(&self) -> (ConnectionId, mpsc::UnboundedReceiver<Value>) {
            let (sender, receiver) = mpsc::unbounded_channel();
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            self.connections.lock().unwrap().insert(id, sender);
            (id, receiver)
        }

        fn receive(&self, connection: ConnectionId, message: Result<Value, String>) {
            let connections = self.connections.lock().unwrap();
            if let Some(sender) = connections.get(&connection) {
                let reply = match message {
                    Ok(value) => json!({ "echo": value }),
                    Err(error) => json!({ "parseError": error }),
                };
                let _ = sender.send(reply);
            }
        }

        fn disconnect(&self, connection: ConnectionId) {
            self.connections.lock().unwrap().remove(&connection);
            self.disconnects.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn parse_listen_accepts_documented_urls() {
        let default = Path::new("/tmp/default.sock");
        assert_eq!(parse_listen("stdio://", default), Ok(Listen::Stdio));
        assert_eq!(
            parse_listen("unix://", default),
            Ok(Listen::Unix(default.to_path_buf()))
        );
        assert_eq!(
            parse_listen("unix:///tmp/named.sock", default),
            Ok(Listen::Unix(PathBuf::from("/tmp/named.sock")))
        );
    }

    #[test]
    fn parse_listen_rejects_bad_urls() {
        let default = Path::new("/tmp/default.sock");
        let relative = parse_listen("unix://relative.sock", default).unwrap_err();
        assert!(relative.contains("absolute"), "{relative}");
        let unknown = parse_listen("tcp://127.0.0.1:1", default).unwrap_err();
        assert!(unknown.contains("stdio://"), "{unknown}");
        assert!(unknown.contains("unix://PATH"), "{unknown}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn serve_lines_echoes_bad_json_and_disconnect() {
        let handler = Arc::new(Echo::default());
        let (client_read, server_write) = tokio::io::duplex(4096);
        let (server_read, mut client_write) = tokio::io::duplex(4096);

        let serving = {
            let handler = Arc::clone(&handler);
            tokio::spawn(async move { serve_lines(handler, server_read, server_write).await })
        };

        client_write
            .write_all(b"{\"a\":1}\n{\"b\":2}\nnot json\n")
            .await
            .unwrap();

        let mut reader = BufReader::new(client_read);
        let mut line = String::new();

        reader.read_line(&mut line).await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(line.trim()).unwrap(),
            json!({ "echo": { "a": 1 } })
        );

        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(line.trim()).unwrap(),
            json!({ "echo": { "b": 2 } })
        );

        line.clear();
        reader.read_line(&mut line).await.unwrap();
        let value: Value = serde_json::from_str(line.trim()).unwrap();
        assert!(value.get("parseError").is_some(), "{value}");

        drop(client_write);
        tokio::time::timeout(Duration::from_secs(5), serving)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(handler.disconnects.load(Ordering::SeqCst), 1);
    }

    /// macOS has no `SOCK_CLOEXEC`: a socket is made close-on-exec just after
    /// it is created, so a child spawned in between inherits it. A test that
    /// relies on a socket being closed holds this while creating it; tests
    /// that spawn processes hold it while spawning.
    #[cfg(unix)]
    static FD_LOCK: Mutex<()> = Mutex::new(());

    #[cfg(unix)]
    fn spawn_locked(
        command: &mut tokio::process::Command,
    ) -> std::io::Result<tokio::process::Child> {
        let _guard = FD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        command.spawn()
    }

    #[cfg(unix)]
    fn short_tempdir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("as")
            .tempdir_in("/tmp")
            .unwrap()
    }

    #[cfg(unix)]
    async fn wait_for_socket(path: &Path) {
        for _ in 0..200 {
            if tokio::net::UnixStream::connect(path).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("socket {} never became available", path.display());
    }

    #[cfg(unix)]
    async fn connect_client(
        path: &Path,
    ) -> tokio_tungstenite::WebSocketStream<tokio::net::UnixStream> {
        let stream = tokio::net::UnixStream::connect(path).await.unwrap();
        let (socket, _) = tokio_tungstenite::client_async("ws://localhost/", stream)
            .await
            .unwrap();
        socket
    }

    #[cfg(unix)]
    async fn read_json(
        socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
    ) -> Value {
        use futures_util::StreamExt;
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        serde_json::from_str(message.to_text().unwrap()).unwrap()
    }

    #[cfg(unix)]
    async fn spawn_server(
        handler: Arc<dyn MessageHandler>,
        path: PathBuf,
    ) -> (
        oneshot::Sender<()>,
        tokio::task::JoinHandle<std::io::Result<()>>,
    ) {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let handle = tokio::spawn(async move {
            serve_unix(handler, &path, async move {
                let _ = shutdown_rx.await;
            })
            .await
        });
        (shutdown_tx, handle)
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn serve_unix_round_trips_two_clients_and_removes_socket() {
        use futures_util::SinkExt;
        let dir = short_tempdir();
        let path = dir.path().join("s.sock");
        let handler: Arc<dyn MessageHandler> = Arc::new(Echo::default());
        let (shutdown, server) = spawn_server(Arc::clone(&handler), path.clone()).await;
        wait_for_socket(&path).await;

        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let mut first = connect_client(&path).await;
        first.send(WsMessage::text(r#"{"hi":1}"#)).await.unwrap();
        assert_eq!(read_json(&mut first).await, json!({ "echo": { "hi": 1 } }));

        let mut second = connect_client(&path).await;
        second.send(WsMessage::text(r#"{"yo":2}"#)).await.unwrap();
        assert_eq!(read_json(&mut second).await, json!({ "echo": { "yo": 2 } }));

        let _ = shutdown.send(());
        server.await.unwrap().unwrap();
        assert!(!path.exists(), "socket should be removed on shutdown");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn serve_unix_reports_already_running() {
        let dir = short_tempdir();
        let path = dir.path().join("s.sock");
        let handler: Arc<dyn MessageHandler> = Arc::new(Echo::default());
        let (shutdown, server) = spawn_server(Arc::clone(&handler), path.clone()).await;
        wait_for_socket(&path).await;

        let error = serve_unix(Arc::clone(&handler), &path, std::future::pending::<()>())
            .await
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
        assert!(error.to_string().contains("already running"), "{error}");

        let _ = shutdown.send(());
        server.await.unwrap().unwrap();
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn serve_unix_replaces_stale_socket() {
        use futures_util::SinkExt;
        let dir = short_tempdir();
        let path = dir.path().join("s.sock");
        {
            let _guard = FD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let stale = std::os::unix::net::UnixListener::bind(&path).unwrap();
            drop(stale);
        }
        assert!(path.exists(), "stale socket file should remain");

        let handler: Arc<dyn MessageHandler> = Arc::new(Echo::default());
        let (shutdown, server) = spawn_server(Arc::clone(&handler), path.clone()).await;
        wait_for_socket(&path).await;

        let mut socket = connect_client(&path).await;
        socket
            .send(WsMessage::text(r#"{"stale":false}"#))
            .await
            .unwrap();
        assert_eq!(
            read_json(&mut socket).await,
            json!({ "echo": { "stale": false } })
        );

        let _ = shutdown.send(());
        server.await.unwrap().unwrap();
    }

    #[cfg(unix)]
    async fn bun_available() -> bool {
        let child = spawn_locked(
            tokio::process::Command::new("bun")
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null()),
        );
        match child {
            Ok(mut child) => child.wait().await.is_ok_and(|s| s.success()),
            Err(_) => false,
        }
    }

    #[cfg(unix)]
    const BUN_SCRIPT: &str = r#"const ws = new WebSocket("ws+unix://" + process.argv[2]);
ws.onopen=()=>ws.send(JSON.stringify({hi:1}));
ws.onmessage=(e)=>{console.log(e.data); process.exit(0)};
setTimeout(()=>process.exit(3),5000);
"#;

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn serve_unix_interoperates_with_bun() {
        if !bun_available().await {
            return;
        }
        let dir = short_tempdir();
        let path = dir.path().join("s.sock");
        let handler: Arc<dyn MessageHandler> = Arc::new(Echo::default());
        let (shutdown, server) = spawn_server(Arc::clone(&handler), path.clone()).await;
        wait_for_socket(&path).await;

        let script = dir.path().join("client.ts");
        std::fs::write(&script, BUN_SCRIPT).unwrap();

        let child = spawn_locked(
            tokio::process::Command::new("bun")
                .arg(&script)
                .arg(&path)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped()),
        )
        .unwrap();
        let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stdout.contains("echo"),
            "bun stdout: {stdout:?} stderr: {stderr:?}"
        );

        let _ = shutdown.send(());
        server.await.unwrap().unwrap();
    }
}
