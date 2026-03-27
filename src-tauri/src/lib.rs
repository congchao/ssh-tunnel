use rusqlite::{params, Connection, OptionalExtension};
use russh::client::{AuthResult, Handler};
use russh::Disconnect;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    path::BaseDirectory,
    tray::{TrayIcon, TrayIconBuilder},
    AppHandle, Emitter, Manager, State, WindowEvent,
};
use image::load_from_memory;
use tokio::io::{self, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, RwLock};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SshConfig {
    host: String,
    port: u16,
    username: String,
    password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PortMapping {
    remote_host: String,
    remote_port: u16,
    local_host: Option<String>,
    local_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TunnelConfig {
    ssh: SshConfig,
    mappings: Vec<PortMapping>,
}

#[derive(Debug, Serialize, Clone)]
struct LogEvent {
    level: String,
    message: String,
}

#[derive(Debug, Serialize, Clone)]
struct StatusEvent {
    running: bool,
    connected: bool,
}

#[derive(Clone, Default)]
struct ClientHandler;

impl Handler for ClientHandler {
    type Error = russh::Error;

    fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::PublicKey,
    ) -> impl std::future::Future<Output = Result<bool, Self::Error>> + Send {
        async { Ok(true) }
    }
}

struct TunnelManager {
    stop_tx: watch::Sender<bool>,
    tasks: Vec<tauri::async_runtime::JoinHandle<()>>,
    session: Arc<SessionManager>,
    heartbeat: tauri::async_runtime::JoinHandle<()>,
}

struct TrayState {
    tray: Mutex<Option<TrayIcon>>,
}

impl TrayState {
    fn new() -> Self {
        Self {
            tray: Mutex::new(None),
        }
    }
}

impl TunnelManager {
    async fn start(app: AppHandle, config: TunnelConfig) -> Result<Self, String> {
        let (stop_tx, stop_rx) = watch::channel(false);
        let mut tasks = Vec::new();

        let session = SessionManager::connect(config.ssh.clone()).await?;
        let session = Arc::new(session);

        for mapping in config.mappings.into_iter() {
            let app_handle = app.clone();
            let session = Arc::clone(&session);
            let mut stop_rx = stop_rx.clone();
            let task = tauri::async_runtime::spawn(async move {
                run_listener(app_handle, session, mapping, &mut stop_rx).await;
            });
            tasks.push(task);
        }

        let app_handle = app.clone();
        let session_clone = Arc::clone(&session);
        let mut hb_stop = stop_rx.clone();
        let heartbeat = tauri::async_runtime::spawn(async move {
            heartbeat_loop(app_handle, session_clone, &mut hb_stop).await;
        });

        Ok(Self {
            stop_tx,
            tasks,
            session,
            heartbeat,
        })
    }

    fn stop(self) {
        let _ = self.stop_tx.send(true);
        let session = Arc::clone(&self.session);
        tauri::async_runtime::spawn(async move {
            let _ = session.disconnect().await;
        });
        self.heartbeat.abort();
        for task in self.tasks {
            task.abort();
        }
    }
}

struct TunnelState {
    inner: Mutex<Option<TunnelManager>>,
}

impl TunnelState {
    fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }
}

fn emit_log(app: &AppHandle, level: &str, message: impl Into<String>) {
    let _ = app.emit(
        "tunnel-log",
        LogEvent {
            level: level.to_string(),
            message: message.into(),
        },
    );
}

fn emit_status(app: &AppHandle, running: bool, connected: bool) {
    let _ = app.emit(
        "tunnel-status",
        StatusEvent {
            running,
            connected,
        },
    );
}

fn tray_icon_running() -> Image<'static> {
    load_tray_icon(include_bytes!("../icons/tray_green.png"))
}

fn tray_icon_stopped() -> Image<'static> {
    load_tray_icon(include_bytes!("../icons/tray_red.png"))
}

fn load_tray_icon(bytes: &'static [u8]) -> Image<'static> {
    let image = load_from_memory(bytes).expect("decode tray icon");
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    Image::new_owned(rgba.into_raw(), width, height)
}

fn set_tray_icon(app: &AppHandle, running: bool) {
    let state = app.state::<TrayState>();
    let guard = match state.tray.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    if let Some(tray) = guard.as_ref() {
        let icon = if running {
            tray_icon_running()
        } else {
            tray_icon_stopped()
        };
        let _ = tray.set_icon(Some(icon));
    }
}

const CONFIG_KEY: &str = "tunnel_config";
const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(12);
const RECONNECT_BASE_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
const RECONNECT_MAX_DELAY: std::time::Duration = std::time::Duration::from_secs(30);

struct SessionManager {
    ssh: SshConfig,
    handle: RwLock<Option<russh::client::Handle<ClientHandler>>>,
    reconnecting: AtomicBool,
    connected: AtomicBool,
}

impl SessionManager {
    async fn connect(ssh: SshConfig) -> Result<Self, String> {
        let handle = connect_ssh(&ssh).await?;
        Ok(Self {
            ssh,
            handle: RwLock::new(Some(handle)),
            reconnecting: AtomicBool::new(false),
            connected: AtomicBool::new(true),
        })
    }

    async fn disconnect(&self) -> Result<(), russh::Error> {
        let mut guard = self.handle.write().await;
        if let Some(handle) = guard.as_ref() {
            let _ = handle
                .disconnect(Disconnect::ByApplication, "", "English")
                .await;
        }
        *guard = None;
        Ok(())
    }

    async fn send_ping(&self) -> Result<(), String> {
        let guard = self.handle.read().await;
        let handle = guard.as_ref().ok_or_else(|| "SSH 未连接".to_string())?;
        handle
            .send_ping()
            .await
            .map_err(|err| format!("SSH 心跳失败: {}", DisplayRuSshError(&err)))
    }

    async fn open_direct_channel(
        &self,
        host: String,
        port: u32,
        origin_host: String,
        origin_port: u32,
    ) -> Result<russh::Channel<russh::client::Msg>, String> {
        let guard = self.handle.read().await;
        let handle = guard.as_ref().ok_or_else(|| "SSH 未连接".to_string())?;
        handle
            .channel_open_direct_tcpip(host, port, origin_host, origin_port)
            .await
            .map_err(|err| format!("无法建立 SSH 通道: {}", DisplayRuSshError(&err)))
    }

    fn set_connected(&self, value: bool) {
        self.connected.store(value, Ordering::SeqCst);
    }

    fn begin_reconnect(&self) -> bool {
        self.reconnecting
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    fn end_reconnect(&self) {
        self.reconnecting.store(false, Ordering::SeqCst);
    }

    async fn replace_handle(&self, handle: russh::client::Handle<ClientHandler>) {
        let mut guard = self.handle.write().await;
        *guard = Some(handle);
    }
}

fn open_db(app: &AppHandle) -> Result<Connection, String> {
    let db_path = app
        .path()
        .resolve("config.db", BaseDirectory::AppData)
        .map_err(|err| format!("无法获取配置路径: {}", err))?;
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("无法创建配置目录: {}", err))?;
    }
    let conn = Connection::open(db_path).map_err(|err| format!("无法打开数据库: {}", err))?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        [],
    )
    .map_err(|err| format!("初始化数据库失败: {}", err))?;
    Ok(conn)
}

async fn run_listener(
    app: AppHandle,
    session: Arc<SessionManager>,
    mapping: PortMapping,
    stop_rx: &mut watch::Receiver<bool>,
) {
    let local_host = mapping
        .local_host
        .clone()
        .unwrap_or_else(|| "0.0.0.0".to_string());
    let bind_addr = format!("{}:{}", local_host, mapping.local_port);

    let listener = match TcpListener::bind(&bind_addr).await {
        Ok(listener) => {
            emit_log(
                &app,
                "info",
                format!(
                    "监听端口 {} -> {}:{}",
                    bind_addr, mapping.remote_host, mapping.remote_port
                ),
            );
            listener
        }
        Err(err) => {
            emit_log(&app, "error", format!("无法绑定 {}: {}", bind_addr, err));
            return;
        }
    };

    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                break;
            }
            accept = listener.accept() => {
                match accept {
                    Ok((stream, addr)) => {
                        let app_handle = app.clone();
                        let session = Arc::clone(&session);
                        let map_cfg = mapping.clone();
                        tauri::async_runtime::spawn(async move {
                            handle_connection(app_handle, session, map_cfg, stream, addr).await;
                        });
                    }
                    Err(err) => {
                        emit_log(
                            &app,
                            "error",
                            format!("监听 {} 失败: {}", bind_addr, err),
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    }
                }
            }
        }
    }

    emit_log(&app, "info", format!("停止监听 {}", bind_addr));
}

async fn handle_connection(
    app: AppHandle,
    session: Arc<SessionManager>,
    mapping: PortMapping,
    mut local_stream: TcpStream,
    addr: std::net::SocketAddr,
) {
    let _ = local_stream.set_nodelay(true);
    emit_log(
        &app,
        "info",
        format!(
            "连接 {} -> {}:{}",
            addr, mapping.remote_host, mapping.remote_port
        ),
    );

    let channel = match session
        .open_direct_channel(
            mapping.remote_host.clone(),
            mapping.remote_port as u32,
            addr.ip().to_string(),
            addr.port() as u32,
        )
        .await
    {
        Ok(channel) => channel,
        Err(err) => {
            emit_log(
                &app,
                "error",
                format!(
                    "无法建立通道 {}:{} - {}",
                    mapping.remote_host, mapping.remote_port, err
                ),
            );
            return;
        }
    };

    let mut channel_stream = channel.into_stream();

    let result = io::copy_bidirectional(&mut local_stream, &mut channel_stream).await;
    let _ = channel_stream.shutdown().await;

    if let Err(err) = result {
        let level = if is_connection_closed(&err) {
            "info"
        } else {
            "warn"
        };
        let message = if is_connection_closed(&err) {
            format!("连接已关闭 {}:{}", mapping.remote_host, mapping.remote_port)
        } else {
            format!(
                "数据转发结束 {}:{} - {}",
                mapping.remote_host, mapping.remote_port, err
            )
        };
        emit_log(&app, level, message);
    }
}

async fn connect_ssh(ssh: &SshConfig) -> Result<russh::client::Handle<ClientHandler>, String> {
    let config = Arc::new(russh::client::Config::default());
    let handler = ClientHandler::default();
    let mut session = russh::client::connect(config, (ssh.host.as_str(), ssh.port), handler)
        .await
        .map_err(|err| format!("SSH 连接失败: {}", DisplayRuSshError(&err)))?;

    let auth = session
        .authenticate_password(ssh.username.clone(), ssh.password.clone())
        .await
        .map_err(|err| format!("SSH 认证失败: {}", DisplayRuSshError(&err)))?;

    if !matches!(auth, AuthResult::Success) {
        return Err("SSH 认证未通过".to_string());
    }

    Ok(session)
}

async fn heartbeat_loop(
    app: AppHandle,
    session: Arc<SessionManager>,
    stop_rx: &mut watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                break;
            }
            _ = interval.tick() => {
                if session.send_ping().await.is_err() {
                    if session.begin_reconnect() {
                        session.set_connected(false);
                        emit_status(&app, true, false);
                        set_tray_icon(&app, false);
                        emit_log(&app, "warn", "SSH 连接中断，正在重连...");
                        reconnect_loop(app.clone(), session.clone(), stop_rx).await;
                        session.end_reconnect();
                    }
                }
            }
        }
    }
}

async fn reconnect_loop(
    app: AppHandle,
    session: Arc<SessionManager>,
    stop_rx: &mut watch::Receiver<bool>,
) {
    let mut delay = RECONNECT_BASE_DELAY;
    loop {
        if *stop_rx.borrow() {
            break;
        }
        match connect_ssh(&session.ssh).await {
            Ok(handle) => {
                session.replace_handle(handle).await;
                session.set_connected(true);
                emit_status(&app, true, true);
                set_tray_icon(&app, true);
                emit_log(&app, "info", "SSH 已重连");
                break;
            }
            Err(err) => {
                emit_log(&app, "warn", format!("SSH 重连失败: {}", err));
                let sleep = tokio::time::sleep(delay);
                tokio::select! {
                    _ = stop_rx.changed() => break,
                    _ = sleep => {}
                }
                delay = std::cmp::min(delay * 2, RECONNECT_MAX_DELAY);
            }
        }
    }
}

fn is_connection_closed(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::NotConnected
            | io::ErrorKind::UnexpectedEof
    )
}

struct DisplayRuSshError<'a>(&'a russh::Error);

impl fmt::Display for DisplayRuSshError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

#[tauri::command]
async fn start_tunnel(
    app: AppHandle,
    state: State<'_, TunnelState>,
    config: TunnelConfig,
) -> Result<(), String> {
    emit_log(&app, "info", "启动 SSH 隧道中...");

    {
        let guard = state
            .inner
            .lock()
            .map_err(|_| "Tunnel state lock poisoned".to_string())?;
        if guard.is_some() {
            return Err("隧道已经在运行".to_string());
        }
    }

    let manager = TunnelManager::start(app.clone(), config).await?;

    let mut guard = state
        .inner
        .lock()
        .map_err(|_| "Tunnel state lock poisoned".to_string())?;
    if guard.is_some() {
        manager.stop();
        return Err("隧道已经在运行".to_string());
    }
    *guard = Some(manager);
    set_tray_icon(&app, true);
    emit_status(&app, true, true);
    Ok(())
}

#[tauri::command]
fn stop_tunnel(app: AppHandle, state: State<TunnelState>) -> Result<(), String> {
    let mut guard = state
        .inner
        .lock()
        .map_err(|_| "Tunnel state lock poisoned".to_string())?;

    if let Some(manager) = guard.take() {
        emit_log(&app, "info", "正在停止隧道...");
        manager.stop();
        emit_log(&app, "info", "隧道已停止");
        set_tray_icon(&app, false);
        emit_status(&app, false, false);
        Ok(())
    } else {
        Err("隧道未运行".to_string())
    }
}

#[tauri::command]
fn tunnel_status(state: State<TunnelState>) -> Result<bool, String> {
    let guard = state
        .inner
        .lock()
        .map_err(|_| "Tunnel state lock poisoned".to_string())?;
    Ok(guard.is_some())
}

#[tauri::command]
fn save_config(app: AppHandle, config: TunnelConfig) -> Result<(), String> {
    let conn = open_db(&app)?;
    let payload =
        serde_json::to_string(&config).map_err(|err| format!("序列化配置失败: {}", err))?;
    conn.execute(
        "INSERT INTO app_settings (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![CONFIG_KEY, payload],
    )
    .map_err(|err| format!("保存配置失败: {}", err))?;
    Ok(())
}

#[tauri::command]
fn load_config(app: AppHandle) -> Result<Option<TunnelConfig>, String> {
    let conn = open_db(&app)?;
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM app_settings WHERE key = ?1",
            params![CONFIG_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(|err| format!("读取配置失败: {}", err))?;
    if let Some(json) = value {
        let config = serde_json::from_str(&json).map_err(|err| format!("解析配置失败: {}", err))?;
        Ok(Some(config))
    } else {
        Ok(None)
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(TunnelState::new())
        .manage(TrayState::new())
        .invoke_handler(tauri::generate_handler![
            start_tunnel,
            stop_tunnel,
            tunnel_status,
            save_config,
            load_config
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }

            let dashboard =
                MenuItem::with_id(app, "dashboard", "Dashboard", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&dashboard, &quit])?;

            let icon = tray_icon_stopped();
            let tray = TrayIconBuilder::new()
                .icon(icon)
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| {
                    match event.id.as_ref() {
                        "dashboard" => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .build(app)?;

            tray.set_tooltip(Some("SSH 隧道"))?;
            if let Ok(mut guard) = app.state::<TrayState>().tray.lock() {
                *guard = Some(tray);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
