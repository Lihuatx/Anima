# OpenClaw Gateway WebSocket API 接入指南

通过 Gateway 的 WebSocket 接口，你可以从任何客户端（桌宠、App、网页、脚本）接入 OpenClaw，与 AI Agent 进行实时对话。

---

## 目录

1. [协议概述](#1-协议概述)
2. [连接与认证](#2-连接与认证)
3. [核心概念](#3-核心概念)
4. [RPC 方法速查](#4-rpc-方法速查)
5. [发送消息](#5-发送消息)
6. [接收消息（实时事件）](#6-接收消息实时事件)
7. [获取历史消息](#7-获取历史消息)
8. [订阅会话变更](#8-订阅会话变更)
9. [Agent 信息查询](#9-agent-信息查询)
10. [TTS 语音合成](#10-tts-语音合成)
11. [完整示例](#11-完整示例)
12. [常见问题](#12-常见问题)

---

## 1. 协议概述

### 传输层

- **协议**: WebSocket（文本帧，JSON 负载）
- **默认端口**: `18789`（由 `gateway.port` 配置）
- **地址格式**: `ws://127.0.0.1:18789`
- **帧类型**:
  - `req` — 客户端请求（需携带唯一 id）
  - `res` — 网关响应（携带对应 id）
  - `event` — 服务端推送的实时事件

### 连接流程

```
客户端                          Gateway
  │                               │
  │  ─── WebSocket 连接 ──────→   │
  │                               │
  │  ←── connect.challenge ────   │  (含 nonce)
  │                               │
  │  ─── connect (含 token) ──→   │
  │                               │
  │  ←── hello-ok ─────────────   │  (认证成功，进入就绪)
  │                               │
  │  ─── 各种 RPC 调用 ────────→  │
  │  ←── 响应 / 实时事件 ──────   │
```

---

## 2. 连接与认证

### 2.1 获取连接信息

从 `openclaw.json` 配置中获取：

```json5
{
  gateway: {
    port: 18789,                    // WebSocket 端口
    auth: { token: "your-token" },  // 共享密钥（永久有效）
  },
}
```

> **Token 说明**: `gateway.auth.token` 是共享密钥，**永久有效**，不会过期。
> 多个客户端可以使用同一个 token 同时连接。修改配置后重启 Gateway 即可更换 token。

### 2.2 连接步骤

#### 步骤 1：建立 WebSocket

```javascript
const conn = new WebSocket('ws://127.0.0.1:18789');
```

#### 步骤 2：等待 challenge，发送 connect 请求

客户端连上后，Gateway 会立即推送一个 `connect.challenge` 事件：

```json
{
  "type": "event",
  "event": "connect.challenge",
  "payload": { "nonce": "uuid", "ts": 1737264000000 }
}
```

客户端需要回复 `connect` 请求：

```json
{
  "type": "req",
  "id": "1",
  "method": "connect",
  "params": {
    "minProtocol": 3,
    "maxProtocol": 3,
    "client": {
      "id": "my-app",
      "version": "1.0.0",
      "platform": "web",
      "mode": "backend"
    },
    "role": "operator",
    "scopes": ["operator.read", "operator.write"],
    "auth": { "token": "your-token-here" },
    "locale": "zh-CN"
  }
}
```

#### 步骤 3：接收 hello-ok

认证成功后的响应：

```json
{
  "type": "res",
  "id": "1",
  "ok": true,
  "payload": {
    "type": "hello-ok",
    "protocol": 3,
    "server": { "version": "2026.5.7", "connId": "..." },
    "features": {
      "methods": ["health", "sessions.list", "sessions.send", "chat.send", ...],
      "events": ["session.message", "chat", "health", ...]
    },
    "auth": { "role": "operator", "scopes": ["operator.read", "operator.write"] },
    "policy": {
      "maxPayload": 26214400,
      "maxBufferedBytes": 52428800,
      "tickIntervalMs": 15000
    }
  }
}
```

认证完成后即可开始调用各种 RPC 方法。

### 2.3 关键参数说明

| 参数 | 说明 | 建议值 |
|------|------|--------|
| `client.id` | 客户端标识 | 自定义，如 `"desktop-pet"` |
| `client.mode` | 客户端运行模式 | `"backend"`（本地 loopback 连接时可省略 device 认证） |
| `role` | 角色 | `"operator"` |
| `scopes` | 权限范围 | `["operator.read", "operator.write"]` |
| `auth.token` | 共享密钥 | 从 `gateway.auth.token` 获取 |

> **本地连接**：使用 `client.mode: "backend"` + `client.id: "gateway-client"` 可以省略 device 签名认证，仅需 token。
> **远程连接**：需配置 `gateway.bind: "0.0.0.0"` 并走 WSS 加密通道（推荐 Tailscale 或反向代理）。
> **浏览器连接**：浏览器发起的 WS 连接权限受限（`missing scope: operator.read`），建议使用 Node.js 或原生客户端。

---

## 3. 核心概念

### 3.1 会话（Session）

会话是对话的基本单位。每个会话有一个唯一 key，格式如 `agent:main:main`。

- **会话 key** 是所有消息操作的标识符
- 一个 Agent 可能有多个活跃会话
- 消息通过 `sessions.send` 发送到指定会话

### 3.2 消息格式

消息内容可以是纯文本字符串，也可以是 Content Block 数组：

```json
// 纯文本
{ "role": "user", "content": "你好" }

// Content Block 数组（AI 回复的常见格式）
{
  "role": "assistant",
  "content": [
    { "type": "text", "text": "你好！我是 vv。" },
    { "type": "thinking", "thinking": "思考过程..." },
    { "type": "tool_use", "name": "process", "input": {...} },
    { "type": "tool_call", "function": { "name": "...", "arguments": "..." } }
  ]
}
```

### 3.3 事件推送

订阅后，会话中的新消息会通过 `session.message` 事件实时推送，无需轮询。

---

## 4. RPC 方法速查

### 系统信息

| 方法 | 用途 |
|------|------|
| `health` | 获取 Gateway 健康状态 |
| `gateway.identity.get` | 获取 Gateway 设备标识 |

### 会话管理

| 方法 | 用途 |
|------|------|
| `sessions.list` | 列出所有会话 |
| `sessions.subscribe` | 订阅会话变更事件 |
| `sessions.messages.subscribe` | 订阅某会话的新消息事件 |
| `sessions.messages.unsubscribe` | 取消订阅消息 |
| `sessions.send` | 向会话发送消息 |
| `sessions.create` | 创建新会话 |
| `sessions.delete` | 删除会话 |

### 对话

| 方法 | 用途 |
|------|------|
| `chat.history` | 获取聊天历史 |
| `chat.send` | 发送消息（需 idempotencyKey） |
| `chat.abort` | 中止当前对话 |

### Agent

| 方法 | 用途 |
|------|------|
| `agent.identity.get` | 获取 Agent 身份信息（名字、头像等） |

### 语音

| 方法 | 用途 |
|------|------|
| `talk.speak` | TTS 语音合成 |
| `tts.status` | TTS 服务状态 |
| `tts.convert` | 文本转语音 |

### 工具与技能

| 方法 | 用途 |
|------|------|
| `tools.catalog` | 获取工具列表 |
| `tools.invoke` | 直接调用工具 |
| `commands.list` | 获取可用命令列表 |

---

## 5. 发送消息

### 5.1 使用 `sessions.send`（推荐）

最简单的发送方式，直接将文本消息发送到指定会话。

**请求**：
```json
{
  "type": "req",
  "id": "4",
  "method": "sessions.send",
  "params": {
    "key": "agent:main:main",
    "message": "你好，我是桌宠"
  }
}
```

**响应**：
```json
{
  "type": "res",
  "id": "4",
  "ok": true,
  "payload": {
    "runId": "84039c88-...",
    "status": "started",
    "messageSeq": 120
  }
}
```

发送成功后，AI 的回复会通过 `session.message` 事件推送回来。

### 5.2 使用 `chat.send`（需 idempotencyKey）

适用于需要幂等性的场景（防止重复发送）：

**请求**：
```json
{
  "type": "req",
  "id": "5",
  "method": "chat.send",
  "params": {
    "sessionKey": "agent:main:main",
    "message": "你好",
    "idempotencyKey": "unique-key-123"
  }
}
```

> `chat.send` 支持更丰富的参数，如 `model`、`systemPrompt` 等，适合高级对话管理。

### 5.3 JavaScript 示例

```javascript
function sessionsSend(key, message) {
  const id = String(++msgId);
  conn.send(JSON.stringify({
    type: 'req', id,
    method: 'sessions.send',
    params: { key, message }
  }));
  return id;
}

// 使用
sessionsSend('agent:main:main', '嗨！');
```

---

## 6. 接收消息（实时事件）

### 6.1 订阅消息事件

在发送或接收消息前，需要先订阅消息事件：

```json
{
  "type": "req",
  "id": "3",
  "method": "sessions.messages.subscribe",
  "params": {
    "key": "agent:main:main"
  }
}
```

成功后响应：
```json
{ "type": "res", "id": "3", "ok": true, "payload": { "subscribed": true } }
```

### 6.2 `session.message` 事件格式

当有新消息时，Gateway 会推送：

```json
{
  "type": "event",
  "event": "session.message",
  "payload": {
    "sessionKey": "agent:main:main",
    "message": {
      "role": "user",           // "user" | "assistant"
      "content": "你好！我是谁？",  // 或 content blocks 数组
      "timestamp": 1778402008665,
      "__openclaw": { "seq": 120 }
    },
    "messageSeq": 120,
    "session": {
      "key": "agent:main:main",
      "kind": "direct",
      "origin": {
        "provider": "webchat",
        "surface": "webchat",
        "chatType": "direct"
      }
      // ... 更多会话元数据
    }
  }
}
```

### 6.3 消息内容格式

`message.content` 有两种格式：

**纯文本格式**（用户消息通常为此格式）：
```javascript
typeof content === 'string' // "你好"
```

**Content Block 格式**（AI 回复通常为此格式）：
```javascript
Array.isArray(content)
// [ { "type": "text", "text": "你好！" }, { "type": "tool_call", ... } ]
```

兼容处理的函数：
```javascript
function getText(content) {
  if (!content) return '';
  if (typeof content === 'string') return content;
  if (Array.isArray(content)) {
    return content.map(c => c.text || c.value || '').join(' ');
  }
  return String(content);
}
```

### 6.4 处理消息的 JavaScript 示例

```javascript
conn.on('message', (data) => {
  const msg = JSON.parse(data.toString());
  
  if (msg.type === 'event' && msg.event === 'session.message') {
    const p = msg.payload;
    const role = p.message.role === 'assistant' ? '🤖 AI' : '👤 用户';
    const text = getText(p.message.content);
    const session = p.sessionKey;
    
    console.log(`[${session}] ${role}: ${text}`);
    
    // 🎯 在这里触发你的桌宠动画、语音等
    // if (role === 'AI') 桌宠开始说话动画
  }
});
```

### 6.5 流式增量（Token-Level Streaming）

**`agent` 事件是 Gateway 的流式通道！** AI 回复时，会通过 `agent` 事件逐 token 推送。

#### agent 事件流

| `agent` stream 类型 | 说明 |
|---------------------|------|
| `assistant` | **AI 文本回复的流式 delta**（逐 token 推送） |
| `item` | 工具调用生命周期（开始/结束） |
| `lifecycle` | 运行生命周期（phase: started/error/completed） |

#### assistant 流（文本逐 token 推送）

```json
{
  "type": "event",
  "event": "agent",
  "payload": {
    "runId": "42848fd7-...",
    "sessionKey": "agent:main:main",
    "stream": "assistant",       // 流式文本
    "data": {
      "text": "完整累积文本",      // 到当前为止的完整文本
      "delta": "新推送的片段"       // 仅本次新增的增量
    },
    "seq": 2,
    "ts": 1778404022867
  }
}
```

#### item 流（工具调用生命周期）

```json
{
  "type": "event",
  "event": "agent",
  "payload": {
    "runId": "42848fd7-...",
    "sessionKey": "agent:main:main",
    "stream": "item",             // 工具/物品事件
    "data": {
      "itemId": "tool:call_00_...",
      "phase": "start",           // "start" | "end"
      "kind": "tool",
      "title": "process swift-shell",
      "status": "running",        // "running" | "completed" | "error"
      "name": "process",
      "meta": "swift-shell",
      "toolCallId": "call_00_..."
    }
  }
}
```

#### lifecycle 流

```json
{
  "type": "event",
  "event": "agent",
  "payload": {
    "stream": "lifecycle",
    "data": { "phase": "started", "endedAt": ..., "error": "..." }
  }
}
```

> 💡 **桌宠开发建议**：
> - 订阅 `agent(stream: "assistant")` → 逐 token 显示 AI 回复（打字机效果）
> - 订阅 `agent(stream: "item")` → 检测工具调用，显示「正在查天气...」等状态
> - 收到 `chat(state: "delta")` 时表示流式进行中
> - 收到 `chat(state: "final")` 时表示本轮对话结束


```json
{
  "type": "event",
  "event": "chat",
  "payload": {
    "runId": "84039c88-...",
    "sessionKey": "agent:main:main",
    "seq": 1,
    "state": "processing"  // "processing" | "final"
  }
}
```

---

## 7. 获取历史消息

### 7.1 请求

```json
{
  "type": "req",
  "id": "5",
  "method": "chat.history",
  "params": {
    "sessionKey": "agent:main:main",
    "limit": 10
  }
}
```

### 7.2 响应

```json
{
  "type": "res",
  "id": "5",
  "ok": true,
  "payload": {
    "messages": [
      {
        "role": "user",
        "content": "你好",
        "timestamp": 1778401000000
      },
      {
        "role": "assistant",
        "content": [
          { "type": "text", "text": "你好！我是 vv。" }
        ],
        "timestamp": 1778401005000
      }
    ]
  }
}
```

### 7.3 注意

- `chat.history` 返回的是**显示优化后**的消息，会清理系统指令标签、工具调用 XML 等对用户不可见的内容
- 消息按时间顺序排列，最新的在最后

---

## 8. 订阅会话变更

### 8.1 订阅

```json
{
  "type": "req",
  "id": "5",
  "method": "sessions.subscribe",
  "params": {}
}
```

### 8.2 会话变更事件

当有新会话创建或会话状态变化时，会收到 `session.created` 或 `sessions.changed` 事件。

```json
{
  "type": "event",
  "event": "session.created",
  "payload": {
    "sessionKey": "agent:main:new-session",
    "kind": "direct"
  }
}
```

---

## 9. Agent 信息查询

### 请求

```json
{
  "type": "req",
  "id": "6",
  "method": "agent.identity.get",
  "params": {}
}
```

### 响应

```json
{
  "type": "res",
  "id": "6",
  "ok": true,
  "payload": {
    "agentId": "main",
    "name": "vv",
    "avatar": "🦊",
    "emoji": "🦊"
  }
}
```

---

## 10. TTS 语音合成

让桌宠"说话"——将文本转为语音音频。

### 请求

```json
{
  "type": "req",
  "id": "7",
  "method": "talk.speak",
  "params": {
    "text": "你好，我是你的桌宠助手",
    "persona": "default"
  }
}
```

### 响应

```json
{
  "type": "res",
  "id": "7",
  "ok": true,
  "payload": {
    "audio": "<base64 encoded audio data>",
    "format": "wav"
  }
}
```

也可以使用 `tts.convert` 进行转换，或通过 `tts.status` 查询可用的语音服务。

---

## 11. 完整示例

### 11.1 Node.js 完整客户端

```javascript
import WebSocket from 'ws';

const WS_URL = 'ws://127.0.0.1:18789';
const TOKEN = 'your-gateway-token';

const conn = new WebSocket(WS_URL);
let msgId = 0;
let sessionKey = null;

// 消息内容提取
function getText(content) {
  if (!content) return '';
  if (typeof content === 'string') return content;
  if (Array.isArray(content)) return content.map(c => c.text || c.value || '').join(' ');
  return String(content);
}

// RPC 调用工具函数
function call(method, params = {}) {
  return new Promise((resolve, reject) => {
    const id = String(++msgId);
    const handler = (data) => {
      const msg = JSON.parse(data.toString());
      if (msg.type === 'res' && msg.id === id) {
        conn.removeListener('message', handler);
        msg.ok ? resolve(msg.payload) : reject(new Error(msg.error?.message));
      }
    };
    conn.on('message', handler);
    conn.send(JSON.stringify({ type: 'req', id, method, params }));
    setTimeout(() => reject(new Error(`${method} timeout`)), 5000);
  });
}

async function main() {
  // 1️⃣ 连接
  await new Promise((resolve, reject) => {
    conn.on('open', () => console.log('✅ 已连接'));
    conn.on('error', reject);

    conn.on('message', function handler(data) {
      const msg = JSON.parse(data.toString());
      if (msg.event === 'connect.challenge') {
        conn.removeListener('message', handler);
        // 认证
        conn.send(JSON.stringify({
          type: 'req', id: '1', method: 'connect',
          params: {
            minProtocol: 3, maxProtocol: 3,
            client: { id: 'desktop-pet', version: '1.0.0', platform: 'web', mode: 'backend' },
            role: 'operator',
            scopes: ['operator.read', 'operator.write'],
            auth: { token: TOKEN },
            locale: 'zh-CN'
          }
        }));
        // 等认证结果
        conn.on('message', function authHandler(d) {
          const m = JSON.parse(d.toString());
          if (m.type === 'res' && m.id === '1') {
            conn.removeListener('message', authHandler);
            if (m.ok) { console.log('✅ 认证成功'); resolve(); }
            else reject(new Error(m.error?.message));
          }
        });
      }
    });
  });

  // 2️⃣ 获取会话列表
  const sessions = await call('sessions.list');
  sessionKey = sessions.sessions?.[0]?.key;
  console.log(`📋 当前会话: ${sessionKey}`);

  // 3️⃣ 订阅消息事件
  if (sessionKey) {
    await call('sessions.messages.subscribe', { key: sessionKey });
    console.log('✅ 已订阅消息');
  }

  // 4️⃣ 发送一条消息
  if (sessionKey) {
    await call('sessions.send', { key: sessionKey, message: '桌宠来报到！' });
    console.log('📤 消息已发送');
  }

  // 5️⃣ 监听实时事件
  conn.on('message', (data) => {
    const msg = JSON.parse(data.toString());
    if (msg.type === 'event' && msg.event === 'session.message') {
      const p = msg.payload;
      const role = p.message.role === 'assistant' ? '🤖 AI' : '👤 我';
      const text = getText(p.message.content);
      console.log(`💬 ${role}: ${text.slice(0, 200)}`);
      // 🎯 触发你的桌宠动画！
    }
  });

  console.log('🎯 监听中...');
}

main().catch(console.error);
```

### 11.2 简单浏览器控制台测试（仅用于验证连通性）

> ⚠️ 注意：浏览器环境发送的 WS 连接可能因安全策略导致权限受限（`missing scope: operator.read`）。
> 建议使用 Node.js 或原生客户端进行开发。

如需快速验证 Gateway 是否在线，打开 `http://localhost:18789/` 看到 Dashboard 即说明服务正常运行。

---

## 12. 常见问题

### Q: Token 会过期吗？

**不会。** `gateway.auth.token` 是共享密钥，永久有效，直到你手动修改配置并重启 Gateway。多个客户端可以同时使用同一个 token。

### Q: 浏览器连接报错 "missing scope: operator.read"？

因为浏览器 WebSocket 的 `Origin` 头被 Gateway 识别为不可信来源，即使 token 正确也会限制权限。**推荐使用 Node.js 或原生应用进行开发。**

### Q: `chat.send` 报错 "must have required property 'idempotencyKey'"？

`chat.send` 是副作用方法（side-effecting），需要 `idempotencyKey` 来防止重复发送。传一个唯一字符串即可：
```json
{ "sessionKey": "...", "message": "...", "idempotencyKey": "uuid-or-timestamp" }
```

也可以用 `sessions.send` 替代，它不需要 idempotencyKey。

### Q: 从 Windows 如何连接 WSL 里的 Gateway？

WSL2 默认会转发 Windows 的 `localhost:18789` 到 WSL 中。如果 Gateway 绑定在 `127.0.0.1`（loopback），Windows 端也可通过 `ws://localhost:18789` 访问。

如果连接不通，检查 WSL2 的网络配置或考虑将 Gateway 绑定改为 `bind: "0.0.0.0"`。

### Q: 怎么创建新会话？

使用 `sessions.create` 方法：
```json
{
  "type": "req",
  "id": "8",
  "method": "sessions.create",
  "params": {}
}
```

### Q: 远程连接需要哪些配置？

1. Gateway 绑定到 `0.0.0.0`（或特定公网 IP）
2. 配置 **WSS**（WebSocket Secure）加密（如通过 Nginx/Caddy 反向代理）
3. 确保 `gateway.auth.token` 已设置
4. 建议使用 Tailscale VPN 隧道替代直接暴露端口

### Q: 消息内容有时显示 `[object Object]`？

因为消息 content 可能是 Content Block 数组而非纯字符串。使用上面提供的 `getText()` 函数兼容处理两种格式。

### Q: 权限和作用域有什么区别？

- `operator.read` — 读取会话、消息、Agent 信息
- `operator.write` — 发送消息、创建会话、调用工具
- `operator.admin` — 配置修改、系统管理

桌宠应用通常只需要 `operator.read` + `operator.write`。

---

## 附录：事件/方法完整列表

### 所有 RPC 方法

```
health
diagnostics.stability
status
gateway.identity.get
system-presence
system-event
last-heartbeat
set-heartbeats
sessions.list
sessions.subscribe
sessions.unsubscribe
sessions.messages.subscribe
sessions.messages.unsubscribe
sessions.preview
sessions.describe
sessions.create
sessions.send
sessions.abort
sessions.patch
sessions.reset
sessions.delete
sessions.compact
sessions.get
chat.history
chat.send
chat.abort
chat.inject
agent.identity.get
agent.wait
talk.speak
talk.config
talk.mode
tts.status
tts.providers
tts.enable
tts.disable
tts.setProvider
tts.convert
models.list
models.authStatus
tools.catalog
tools.effective
tools.invoke
commands.list
cron.list
cron.status
cron.add
cron.update
cron.remove
cron.run
cron.runs
config.get
config.set
config.apply
config.patch
config.schema
config.schema.lookup
agents.list
agents.create
agents.update
agents.delete
exec.approval.request     // 发起审批请求
exec.approval.get         // 查看审批详情
exec.approval.list        // 列出待审批事项
exec.approval.resolve     // 同意/拒绝审批
exec.approval.waitDecision // 等待审批结果
exec.approvals.get        // 查看审批策略
exec.approvals.set        // 设置审批策略
plugin.approval.request
plugin.approval.list
plugin.approval.waitDecision
plugin.approval.resolve
```

> **审批同意/拒绝**：调用 `exec.approval.resolve`，传入 `id`（审批 ID）和 `decision`（`"approve"` 或 `"deny"`），需 `operator.approvals` 权限。

```json
{
  "type": "req",
  "id": "10",
  "method": "exec.approval.resolve",
  "params": {
    "id": "approval-uuid-here",
    "decision": "approve"
    // 或 "deny"
  }
}
```

> 完整方法列表可通过认证后的 `hello-ok.features.methods` 动态获取。

### 所有事件

```
connect.challenge       // 连接挑战（handshake）
agent                   // 🎯 流式通道：assistant/item/lifecycle
session.message         // 🎯 完整消息（用户或AI）
session.tool            // 🎯 工具调用事件
session.created         // 新会话创建
sessions.changed        // 会话变更
chat                    // 对话状态（processing/delta/final）
health                  // 健康状态
tick                    // 心跳保活
presence                // 客户端在线状态
heartbeat               // 心跳事件流
cron                    // 定时任务
shutdown                // 服务关闭
voicewake.changed       // 语音唤醒配置变更
exec.approval.requested
exec.approval.resolved
node.pair.requested
node.pair.resolved
device.pair.requested
device.pair.resolved
```

---

> 💡 **最佳实践**：保持一个长连接的 WebSocket（不断开），事件订阅后持续监听。
> Gateway 内置了心跳保活机制（默认 15s tick），无需手动 ping-pong。

> 🔒 **安全提醒**：Token 即密码。不要将 token 硬编码到公开分发的前端代码中。
> 桌宠应当通过后端代理连接 Gateway，或使用 Tailscale 等安全隧道。
