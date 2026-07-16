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
    #[serde(default)]
    remark: String,
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
    let mut conn = Connection::open(db_path).map_err(|err| format!("无法打开数据库: {}", err))?;
    init_db(&mut conn)?;
    migrate_legacy_config(&mut conn)?;
    Ok(conn)
}

fn init_db(conn: &mut Connection) -> Result<(), String> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        [],
    )
    .map_err(|err| format!("初始化数据库失败: {}", err))?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS ssh_config (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            host TEXT NOT NULL,
            port INTEGER NOT NULL,
            username TEXT NOT NULL,
            password TEXT NOT NULL
        )",
        [],
    )
    .map_err(|err| format!("初始化 SSH 配置表失败: {}", err))?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS port_mappings (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            position INTEGER NOT NULL,
            remote_host TEXT NOT NULL,
            remote_port INTEGER NOT NULL,
            local_host TEXT NOT NULL,
            local_port INTEGER NOT NULL,
            remark TEXT NOT NULL DEFAULT ''
        )",
        [],
    )
    .map_err(|err| format!("初始化端口映射表失败: {}", err))?;
    Ok(())
}

fn migrate_legacy_config(conn: &mut Connection) -> Result<(), String> {
    let structured_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM ssh_config", [], |row| row.get(0))
        .map_err(|err| format!("检查配置迁移状态失败: {}", err))?;
    if structured_count > 0 {
        return Ok(());
    }

    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM app_settings WHERE key = ?1",
            params![CONFIG_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(|err| format!("读取旧配置失败: {}", err))?;

    let Some(json) = value else {
        return Ok(());
    };

    let config: TunnelConfig =
        serde_json::from_str(&json).map_err(|err| format!("迁移旧配置失败: {}", err))?;
    save_config_to_db(conn, &config)?;
    conn.execute(
        "DELETE FROM app_settings WHERE key = ?1",
        params![CONFIG_KEY],
    )
    .map_err(|err| format!("清理旧配置失败: {}", err))?;
    Ok(())
}

fn save_config_to_db(conn: &mut Connection, config: &TunnelConfig) -> Result<(), String> {
    let tx = conn
        .transaction()
        .map_err(|err| format!("开始保存配置失败: {}", err))?;
    tx.execute("DELETE FROM ssh_config", [])
        .map_err(|err| format!("清理 SSH 配置失败: {}", err))?;
    tx.execute("DELETE FROM port_mappings", [])
        .map_err(|err| format!("清理端口映射失败: {}", err))?;
    tx.execute(
        "INSERT INTO ssh_config (id, host, port, username, password)
         VALUES (1, ?1, ?2, ?3, ?4)",
        params![
            config.ssh.host,
            config.ssh.port,
            config.ssh.username,
            config.ssh.password
        ],
    )
    .map_err(|err| format!("保存 SSH 配置失败: {}", err))?;

    for (index, mapping) in config.mappings.iter().enumerate() {
        let local_host = mapping
            .local_host
            .clone()
            .unwrap_or_else(|| "0.0.0.0".to_string());
        tx.execute(
            "INSERT INTO port_mappings (
                position, remote_host, remote_port, local_host, local_port, remark
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                index as i64,
                mapping.remote_host,
                mapping.remote_port,
                local_host,
                mapping.local_port,
                mapping.remark
            ],
        )
        .map_err(|err| format!("保存端口映射失败: {}", err))?;
    }

    tx.commit()
        .map_err(|err| format!("提交配置保存失败: {}", err))?;
    Ok(())
}

fn load_config_from_db(conn: &Connection) -> Result<Option<TunnelConfig>, String> {
    let ssh = conn
        .query_row(
            "SELECT host, port, username, password FROM ssh_config WHERE id = 1",
            [],
            |row| {
                Ok(SshConfig {
                    host: row.get(0)?,
                    port: row.get(1)?,
                    username: row.get(2)?,
                    password: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|err| format!("读取 SSH 配置失败: {}", err))?;

    let Some(ssh) = ssh else {
        return Ok(None);
    };

    let mut stmt = conn
        .prepare(
            "SELECT remote_host, remote_port, local_host, local_port, remark
             FROM port_mappings
             ORDER BY position ASC, id ASC",
        )
        .map_err(|err| format!("读取端口映射失败: {}", err))?;
    let mappings = stmt
        .query_map([], |row| {
            Ok(PortMapping {
                remote_host: row.get(0)?,
                remote_port: row.get(1)?,
                local_host: Some(row.get(2)?),
                local_port: row.get(3)?,
                remark: row.get(4)?,
            })
        })
        .map_err(|err| format!("读取端口映射失败: {}", err))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("解析端口映射失败: {}", err))?;

    Ok(Some(TunnelConfig { ssh, mappings }))
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
    let mut conn = open_db(&app)?;
    save_config_to_db(&mut conn, &config)
}

#[tauri::command]
fn load_config(app: AppHandle) -> Result<Option<TunnelConfig>, String> {
    let conn = open_db(&app)?;
    load_config_from_db(&conn)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> TunnelConfig {
        TunnelConfig {
            ssh: SshConfig {
                host: "example.com".to_string(),
                port: 22,
                username: "user".to_string(),
                password: "secret".to_string(),
            },
            mappings: vec![
                PortMapping {
                    remote_host: "db.internal".to_string(),
                    remote_port: 5432,
                    local_host: Some("127.0.0.1".to_string()),
                    local_port: 15432,
                    remark: "postgres".to_string(),
                },
                PortMapping {
                    remote_host: "redis.internal".to_string(),
                    remote_port: 6379,
                    local_host: None,
                    local_port: 16379,
                    remark: "".to_string(),
                },
            ],
        }
    }

    #[test]
    fn saves_and_loads_structured_config() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        init_db(&mut conn).expect("init db");

        let config = sample_config();
        save_config_to_db(&mut conn, &config).expect("save config");

        let loaded = load_config_from_db(&conn)
            .expect("load config")
            .expect("config exists");
        assert_eq!(loaded.ssh.host, "example.com");
        assert_eq!(loaded.mappings.len(), 2);
        assert_eq!(loaded.mappings[0].remark, "postgres");
        assert_eq!(
            loaded.mappings[1].local_host.as_deref(),
            Some("0.0.0.0")
        );
    }

    #[test]
    fn migrates_legacy_json_config() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        init_db(&mut conn).expect("init db");

        let config = sample_config();
        let payload = serde_json::to_string(&config).expect("serialize config");
        conn.execute(
            "INSERT INTO app_settings (key, value) VALUES (?1, ?2)",
            params![CONFIG_KEY, payload],
        )
        .expect("insert legacy config");

        migrate_legacy_config(&mut conn).expect("migrate config");

        let loaded = load_config_from_db(&conn)
            .expect("load config")
            .expect("config exists");
        assert_eq!(loaded.ssh.username, "user");
        assert_eq!(loaded.mappings[0].remote_host, "db.internal");

        let legacy_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM app_settings WHERE key = ?1",
                params![CONFIG_KEY],
                |row| row.get(0),
            )
            .expect("count legacy rows");
        assert_eq!(legacy_count, 0);
    }
}
