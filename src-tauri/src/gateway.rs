use std::{collections::HashMap, fs, path::PathBuf, time::Duration};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::{
    sync::{mpsc, oneshot, Mutex},
    time::{sleep, timeout},
};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

const DEFAULT_GATEWAY_URL: &str = "ws://localhost:18789";
const DEFAULT_SESSION_KEY: &str = "agent:main:main";
const SETTINGS_FILE: &str = "gateway-settings.json";
const RPC_TIMEOUT: Duration = Duration::from_secs(10);
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

type GatewaySocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySettings {
    pub gateway_url: String,
    pub gateway_token: Option<String>,
    pub session_key: Option<String>,
    pub auto_connect: bool,
}

impl Default for GatewaySettings {
    fn default() -> Self {
        Self {
            gateway_url: DEFAULT_GATEWAY_URL.to_string(),
            gateway_token: None,
            session_key: Some(DEFAULT_SESSION_KEY.to_string()),
            auto_connect: false,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySettingsInput {
    gateway_url: Option<String>,
    gateway_token: Option<String>,
    session_key: Option<String>,
    auto_connect: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayPublicSettings {
    gateway_url: String,
    session_key: String,
    auto_connect: bool,
    has_gateway_token: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayStatus {
    state: String,
    message: Option<String>,
    session_key: Option<String>,
    agent: Option<Value>,
}

#[derive(Debug)]
enum BridgeCommand {
    Rpc {
        method: String,
        params: Value,
        response: oneshot::Sender<Result<Value, String>>,
    },
    Shutdown,
}

#[derive(Debug)]
struct BridgeInner {
    settings: GatewaySettings,
    status: GatewayStatus,
    command_tx: Option<mpsc::UnboundedSender<BridgeCommand>>,
    session_key: Option<String>,
}

pub struct GatewayBridge {
    inner: Mutex<BridgeInner>,
}

impl Default for GatewayBridge {
    fn default() -> Self {
        Self {
            inner: Mutex::new(BridgeInner {
                settings: GatewaySettings::default(),
                status: status_payload("offline", None, None, None),
                command_tx: None,
                session_key: None,
            }),
        }
    }
}

impl GatewayBridge {
    pub async fn set_settings(&self, settings: GatewaySettings) {
        let mut inner = self.inner.lock().await;
        inner.settings = normalize_settings(settings);
    }

    async fn public_settings(&self) -> GatewayPublicSettings {
        let inner = self.inner.lock().await;
        public_settings(&inner.settings)
    }

    async fn status(&self) -> GatewayStatus {
        self.inner.lock().await.status.clone()
    }

    async fn start(&self, app: AppHandle, settings: GatewaySettings) {
        let settings = normalize_settings(settings);
        let (tx, rx) = mpsc::unbounded_channel();
        let old_tx = {
            let mut inner = self.inner.lock().await;
            inner.settings = settings.clone();
            inner.command_tx.replace(tx)
        };

        if let Some(old_tx) = old_tx {
            let _ = old_tx.send(BridgeCommand::Shutdown);
        }

        tauri::async_runtime::spawn(gateway_task(app, settings, rx));
    }

    async fn stop(&self, app: &AppHandle) {
        let old_tx = {
            let mut inner = self.inner.lock().await;
            inner.command_tx.take()
        };

        if let Some(old_tx) = old_tx {
            let _ = old_tx.send(BridgeCommand::Shutdown);
        }

        update_status(app, "offline", None, None, None).await;
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let tx = {
            let inner = self.inner.lock().await;
            inner.command_tx.clone()
        }
        .ok_or_else(|| "Gateway is offline".to_string())?;

        let (response_tx, response_rx) = oneshot::channel();
        tx.send(BridgeCommand::Rpc {
            method: method.to_string(),
            params,
            response: response_tx,
        })
        .map_err(|_| "Gateway bridge is not running".to_string())?;

        response_rx
            .await
            .map_err(|_| "Gateway bridge stopped before responding".to_string())?
    }

    async fn current_session_key(&self) -> String {
        let inner = self.inner.lock().await;
        inner
            .session_key
            .clone()
            .or_else(|| inner.settings.session_key.clone())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_SESSION_KEY.to_string())
    }
}

pub async fn initialize_gateway(app: &AppHandle) {
    let settings = load_settings(app);
    let bridge = app.state::<GatewayBridge>();
    bridge.set_settings(settings.clone()).await;

    if settings.auto_connect {
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let bridge = app_handle.state::<GatewayBridge>();
            bridge.start(app_handle.clone(), settings).await;
        });
    }
}

#[tauri::command]
pub async fn gateway_get_settings(
    state: State<'_, GatewayBridge>,
) -> Result<GatewayPublicSettings, String> {
    Ok(state.public_settings().await)
}

#[tauri::command]
pub async fn gateway_get_status(state: State<'_, GatewayBridge>) -> Result<GatewayStatus, String> {
    Ok(state.status().await)
}

#[tauri::command]
pub async fn gateway_save_settings(
    app: AppHandle,
    state: State<'_, GatewayBridge>,
    input: GatewaySettingsInput,
) -> Result<GatewayPublicSettings, String> {
    let current = {
        let inner = state.inner.lock().await;
        inner.settings.clone()
    };

    let mut next = current;

    if let Some(gateway_url) = input.gateway_url {
        next.gateway_url = gateway_url;
    }

    if let Some(gateway_token) = input.gateway_token {
        let token = gateway_token.trim().to_string();
        next.gateway_token = if token.is_empty() { None } else { Some(token) };
    }

    if let Some(session_key) = input.session_key {
        next.session_key = Some(session_key);
    }

    next.auto_connect = input.auto_connect;
    next = normalize_settings(next);

    save_settings(&app, &next)?;
    state.set_settings(next.clone()).await;

    if next.auto_connect {
        state.start(app, next.clone()).await;
    }

    Ok(public_settings(&next))
}

#[tauri::command]
pub async fn gateway_connect(
    app: AppHandle,
    state: State<'_, GatewayBridge>,
) -> Result<(), String> {
    let settings = {
        let inner = state.inner.lock().await;
        inner.settings.clone()
    };

    state.start(app, settings).await;
    Ok(())
}

#[tauri::command]
pub async fn gateway_disconnect(
    app: AppHandle,
    state: State<'_, GatewayBridge>,
) -> Result<(), String> {
    state.stop(&app).await;
    Ok(())
}

#[tauri::command]
pub async fn gateway_send_message(
    state: State<'_, GatewayBridge>,
    message: String,
) -> Result<Value, String> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Err("Message is empty".to_string());
    }

    let session_key = state.current_session_key().await;
    state
        .call(
            "sessions.send",
            json!({ "key": session_key, "message": trimmed }),
        )
        .await
}

#[tauri::command]
pub async fn gateway_abort(state: State<'_, GatewayBridge>) -> Result<Value, String> {
    let session_key = state.current_session_key().await;
    state
        .call("chat.abort", json!({ "sessionKey": session_key }))
        .await
}

async fn gateway_task(
    app: AppHandle,
    settings: GatewaySettings,
    mut rx: mpsc::UnboundedReceiver<BridgeCommand>,
) {
    loop {
        update_status(&app, "reconnecting", None, None, None).await;

        match connect_and_run(&app, &settings, &mut rx).await {
            TaskExit::Shutdown => {
                update_status(&app, "offline", None, None, None).await;
                break;
            }
            TaskExit::Fatal(message) => {
                update_status(&app, "error", Some(message), None, None).await;
                break;
            }
            TaskExit::Disconnected(message) => {
                update_status(&app, "error", Some(message), None, None).await;

                tokio::select! {
                    maybe_cmd = rx.recv() => {
                        match maybe_cmd {
                            Some(BridgeCommand::Shutdown) | None => {
                                update_status(&app, "offline", None, None, None).await;
                                break;
                            }
                            Some(BridgeCommand::Rpc { response, .. }) => {
                                let _ = response.send(Err("Gateway is reconnecting".to_string()));
                            }
                        }
                    }
                    _ = sleep(RECONNECT_DELAY) => {}
                }
            }
        }
    }
}

enum TaskExit {
    Shutdown,
    Fatal(String),
    Disconnected(String),
}

async fn connect_and_run(
    app: &AppHandle,
    settings: &GatewaySettings,
    rx: &mut mpsc::UnboundedReceiver<BridgeCommand>,
) -> TaskExit {
    let socket_result = connect_async(settings.gateway_url.as_str()).await;
    let (mut socket, _) = match socket_result {
        Ok(result) => result,
        Err(error) => return TaskExit::Disconnected(error.to_string()),
    };

    if let Err(error) = authenticate(app, &mut socket, settings).await {
        return TaskExit::Fatal(error);
    }

    let init = initialize_session(app, &mut socket, settings).await;
    let (session_key, agent) = match init {
        Ok(init) => init,
        Err(error) => return TaskExit::Fatal(error),
    };

    {
        let bridge = app.state::<GatewayBridge>();
        let mut inner = bridge.inner.lock().await;
        inner.session_key = Some(session_key.clone());
    }

    update_status(app, "connected", None, Some(session_key), agent).await;

    run_socket_loop(app, socket, rx).await
}

async fn authenticate(
    app: &AppHandle,
    socket: &mut GatewaySocket,
    settings: &GatewaySettings,
) -> Result<(), String> {
    let challenge = timeout(RPC_TIMEOUT, async {
        while let Some(message) = socket.next().await {
            let message = message.map_err(|error| error.to_string())?;
            if let Some(value) = parse_gateway_message(message)? {
                if value.get("type").and_then(Value::as_str) == Some("event")
                    && value.get("event").and_then(Value::as_str) == Some("connect.challenge")
                {
                    return Ok(());
                }
                forward_gateway_event(app, &value);
            }
        }
        Err("Gateway closed before connect.challenge".to_string())
    })
    .await
    .map_err(|_| "Timed out waiting for connect.challenge".to_string())?;

    challenge?;

    let payload = call_init_rpc(
        app,
        socket,
        "connect-1",
        "connect",
        json!({
            "minProtocol": 3,
            "maxProtocol": 3,
            "client": {
                "id": "gateway-client",
                "version": env!("CARGO_PKG_VERSION"),
                "platform": "browser",
                "mode": "backend"
            },
            "role": "operator",
            "scopes": ["operator.read", "operator.write", "operator.approvals"],
            "auth": { "token": settings.gateway_token.clone().unwrap_or_default() },
            "locale": "zh-CN"
        }),
    )
    .await?;

    let features = payload.get("features").cloned().unwrap_or(Value::Null);
    let bridge = app.state::<GatewayBridge>();
    let mut inner = bridge.inner.lock().await;
    inner.status = status_payload(
        "reconnecting",
        Some("Authenticated".to_string()),
        None,
        None,
    );
    drop(inner);

    let _ = app.emit("gateway:features", sanitize_value(features));
    Ok(())
}

async fn initialize_session(
    app: &AppHandle,
    socket: &mut GatewaySocket,
    settings: &GatewaySettings,
) -> Result<(String, Option<Value>), String> {
    let agent = call_init_rpc(app, socket, "init-agent", "agent.identity.get", json!({}))
        .await
        .ok()
        .map(sanitize_value);

    let preferred = settings
        .session_key
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_SESSION_KEY.to_string());

    let sessions_payload =
        call_init_rpc(app, socket, "init-sessions", "sessions.list", json!({})).await;
    let session_keys = sessions_payload
        .as_ref()
        .map(extract_session_keys)
        .unwrap_or_default();

    let session_key = if session_keys.iter().any(|key| key == &preferred) {
        preferred
    } else if let Some(first) = session_keys.first() {
        first.clone()
    } else {
        let created = call_init_rpc(
            app,
            socket,
            "init-create-session",
            "sessions.create",
            json!({}),
        )
        .await?;
        extract_single_session_key(&created).unwrap_or(preferred)
    };

    let _ = call_init_rpc(
        app,
        socket,
        "init-subscribe-messages",
        "sessions.messages.subscribe",
        json!({ "key": session_key }),
    )
    .await?;

    let _ = call_init_rpc(
        app,
        socket,
        "init-subscribe-sessions",
        "sessions.subscribe",
        json!({}),
    )
    .await;

    if let Ok(history) = call_init_rpc(
        app,
        socket,
        "init-history",
        "chat.history",
        json!({ "sessionKey": session_key, "limit": 30 }),
    )
    .await
    {
        let _ = app.emit(
            "gateway:history",
            json!({
                "sessionKey": session_key,
                "messages": sanitize_value(history).get("messages").cloned().unwrap_or(Value::Array(vec![]))
            }),
        );
    }

    Ok((session_key, agent))
}

async fn call_init_rpc(
    app: &AppHandle,
    socket: &mut GatewaySocket,
    id: &str,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let request = json!({
        "type": "req",
        "id": id,
        "method": method,
        "params": params
    });

    socket
        .send(Message::Text(request.to_string()))
        .await
        .map_err(|error| error.to_string())?;

    let result = timeout(RPC_TIMEOUT, async {
        while let Some(message) = socket.next().await {
            let message = message.map_err(|error| error.to_string())?;
            let Some(value) = parse_gateway_message(message)? else {
                continue;
            };

            if value.get("type").and_then(Value::as_str) == Some("res")
                && value.get("id").and_then(Value::as_str) == Some(id)
            {
                return gateway_response_result(&value);
            }

            forward_gateway_event(app, &value);
        }

        Err(format!("Gateway closed while waiting for {method}"))
    })
    .await
    .map_err(|_| format!("{method} timed out"))?;

    result
}

async fn run_socket_loop(
    app: &AppHandle,
    socket: GatewaySocket,
    rx: &mut mpsc::UnboundedReceiver<BridgeCommand>,
) -> TaskExit {
    let (mut writer, mut reader) = socket.split();
    let mut pending: HashMap<String, oneshot::Sender<Result<Value, String>>> = HashMap::new();
    let mut next_id: u64 = 1000;

    loop {
        tokio::select! {
            maybe_cmd = rx.recv() => {
                match maybe_cmd {
                    Some(BridgeCommand::Shutdown) | None => {
                        let _ = writer.send(Message::Close(None)).await;
                        return TaskExit::Shutdown;
                    }
                    Some(BridgeCommand::Rpc { method, params, response }) => {
                        next_id += 1;
                        let id = format!("rpc-{next_id}");
                        let request = json!({
                            "type": "req",
                            "id": id,
                            "method": method,
                            "params": params,
                        });

                        if let Err(error) = writer.send(Message::Text(request.to_string())).await {
                            let _ = response.send(Err(error.to_string()));
                            fail_pending(&mut pending, "Gateway connection dropped");
                            return TaskExit::Disconnected(error.to_string());
                        }

                        pending.insert(id, response);
                    }
                }
            }
            message = reader.next() => {
                let Some(message) = message else {
                    fail_pending(&mut pending, "Gateway connection closed");
                    return TaskExit::Disconnected("Gateway connection closed".to_string());
                };

                let value = match message {
                    Ok(message) => match parse_gateway_message(message) {
                        Ok(Some(value)) => value,
                        Ok(None) => continue,
                        Err(error) => {
                            return TaskExit::Disconnected(error);
                        }
                    },
                    Err(error) => {
                        fail_pending(&mut pending, "Gateway read failed");
                        return TaskExit::Disconnected(error.to_string());
                    }
                };

                if value.get("type").and_then(Value::as_str) == Some("res") {
                    if let Some(id) = value.get("id").and_then(Value::as_str) {
                        if let Some(response) = pending.remove(id) {
                            let _ = response.send(gateway_response_result(&value));
                        }
                    }
                    continue;
                }

                forward_gateway_event(app, &value);

                if value.get("event").and_then(Value::as_str) == Some("shutdown") {
                    fail_pending(&mut pending, "Gateway shutdown");
                    return TaskExit::Disconnected("Gateway shutdown".to_string());
                }
            }
        }
    }
}

fn fail_pending(
    pending: &mut HashMap<String, oneshot::Sender<Result<Value, String>>>,
    message: &str,
) {
    for (_, response) in pending.drain() {
        let _ = response.send(Err(message.to_string()));
    }
}

fn parse_gateway_message(message: Message) -> Result<Option<Value>, String> {
    match message {
        Message::Text(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| error.to_string()),
        Message::Binary(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| error.to_string()),
        Message::Close(_) => Err("Gateway closed the connection".to_string()),
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => Ok(None),
    }
}

fn gateway_response_result(value: &Value) -> Result<Value, String> {
    if value.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(value.get("payload").cloned().unwrap_or(Value::Null));
    }

    let error = value
        .get("error")
        .and_then(|error| error.get("message").or(Some(error)))
        .and_then(Value::as_str)
        .unwrap_or("Gateway request failed");

    Err(error.to_string())
}

fn forward_gateway_event(app: &AppHandle, value: &Value) {
    if value.get("type").and_then(Value::as_str) != Some("event") {
        return;
    }

    let Some(event_name) = value.get("event").and_then(Value::as_str) else {
        return;
    };

    let forward = matches!(
        event_name,
        "session.message"
            | "session.tool"
            | "agent"
            | "chat"
            | "health"
            | "shutdown"
            | "session.created"
            | "sessions.changed"
    );

    if !forward {
        return;
    }

    let payload = value.get("payload").cloned().unwrap_or(Value::Null);
    let _ = app.emit(
        "gateway:event",
        json!({
            "event": event_name,
            "payload": sanitize_value(payload)
        }),
    );
}

async fn update_status(
    app: &AppHandle,
    state: &str,
    message: Option<String>,
    session_key: Option<String>,
    agent: Option<Value>,
) {
    let payload = status_payload(state, message, session_key, agent);

    {
        let bridge = app.state::<GatewayBridge>();
        let mut inner = bridge.inner.lock().await;
        inner.status = payload.clone();
    }

    let _ = app.emit("gateway:status", &payload);
}

fn status_payload(
    state: &str,
    message: Option<String>,
    session_key: Option<String>,
    agent: Option<Value>,
) -> GatewayStatus {
    GatewayStatus {
        state: state.to_string(),
        message,
        session_key,
        agent,
    }
}

fn normalize_settings(mut settings: GatewaySettings) -> GatewaySettings {
    settings.gateway_url = settings.gateway_url.trim().to_string();
    if settings.gateway_url.is_empty() {
        settings.gateway_url = DEFAULT_GATEWAY_URL.to_string();
    }

    settings.gateway_token = settings
        .gateway_token
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty());

    settings.session_key = Some(
        settings
            .session_key
            .unwrap_or_else(|| DEFAULT_SESSION_KEY.to_string())
            .trim()
            .to_string(),
    )
    .filter(|value| !value.is_empty());

    settings
}

fn public_settings(settings: &GatewaySettings) -> GatewayPublicSettings {
    GatewayPublicSettings {
        gateway_url: settings.gateway_url.clone(),
        session_key: settings
            .session_key
            .clone()
            .unwrap_or_else(|| DEFAULT_SESSION_KEY.to_string()),
        auto_connect: settings.auto_connect,
        has_gateway_token: settings.gateway_token.is_some(),
    }
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?;
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir.join(SETTINGS_FILE))
}

fn load_settings(app: &AppHandle) -> GatewaySettings {
    let Ok(path) = settings_path(app) else {
        return GatewaySettings::default();
    };

    let Ok(content) = fs::read_to_string(path) else {
        return GatewaySettings::default();
    };

    serde_json::from_str::<GatewaySettings>(&content)
        .map(normalize_settings)
        .unwrap_or_default()
}

fn save_settings(app: &AppHandle, settings: &GatewaySettings) -> Result<(), String> {
    let path = settings_path(app)?;
    let content = serde_json::to_string_pretty(settings).map_err(|error| error.to_string())?;
    fs::write(path, content).map_err(|error| error.to_string())
}

fn extract_session_keys(payload: &Value) -> Vec<String> {
    let sessions = payload
        .get("sessions")
        .and_then(Value::as_array)
        .or_else(|| payload.as_array());

    sessions
        .into_iter()
        .flatten()
        .filter_map(extract_single_session_key)
        .collect()
}

fn extract_single_session_key(value: &Value) -> Option<String> {
    value
        .get("sessionKey")
        .or_else(|| value.get("key"))
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("session")
                .and_then(|session| session.get("sessionKey").or_else(|| session.get("key")))
                .and_then(Value::as_str)
        })
        .map(ToString::to_string)
}

fn sanitize_value(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    if matches!(
                        lower.as_str(),
                        "token"
                            | "auth"
                            | "authorization"
                            | "secret"
                            | "apikey"
                            | "api_key"
                            | "password"
                    ) {
                        (key, Value::String("[redacted]".to_string()))
                    } else {
                        (key, sanitize_value(value))
                    }
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(sanitize_value).collect()),
        other => other,
    }
}
