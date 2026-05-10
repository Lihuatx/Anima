<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { computed, nextTick, onMounted, onUnmounted, ref } from 'vue'

type ConnectionState = 'connected' | 'reconnecting' | 'offline' | 'error'
type MessageRole = 'user' | 'assistant' | 'system'

interface GatewayPublicSettings {
  gatewayUrl: string
  sessionKey: string
  autoConnect: boolean
  hasGatewayToken: boolean
}

interface GatewayStatus {
  state: ConnectionState
  message?: string | null
  sessionKey?: string | null
  agent?: unknown
}

interface ChatMessage {
  id: string
  role: MessageRole
  text: string
  timestamp?: number
  streaming?: boolean
}

type ToolStatus = 'running' | 'completed' | 'error'

interface ToolTimelineItem {
  id: string
  runId?: string
  title: string
  name?: string
  meta?: string
  status: ToolStatus
  startedAt?: number
  endedAt?: number
  error?: string
}

const draft = ref('')
const placementSide = ref<'left' | 'right'>('left')
const connectionState = ref<ConnectionState>('offline')
const connectionMessage = ref('')
const sessionKey = ref('agent:main:main')
const gatewayUrl = ref('ws://localhost:18789')
const gatewayToken = ref('')
const autoConnect = ref(false)
const hasGatewayToken = ref(false)
const settingsBusy = ref(false)
const sending = ref(false)
const running = ref(false)
const runStatus = ref<'idle' | 'thinking' | 'answering' | 'tool' | 'completed' | 'error'>('idle')
const activityExpanded = ref(false)
const messages = ref<ChatMessage[]>([
  {
    id: 'welcome',
    role: 'assistant',
    text: 'Yulia is ready. Configure Gateway Bridge to start chatting.',
  },
])
const tools = ref<ToolTimelineItem[]>([])
const messagesRef = ref<HTMLElement>()
const activeRunId = ref<string | null>(null)
const activeAssistantMessageId = ref<string | null>(null)

let unlistenPlacement: UnlistenFn | null = null
let unlistenStatus: UnlistenFn | null = null
let unlistenHistory: UnlistenFn | null = null
let unlistenGatewayEvent: UnlistenFn | null = null

const visibleTools = computed(() => tools.value.slice(-12))
const hasActivity = computed(() => running.value || visibleTools.value.length > 0)
const toolSummary = computed(() => {
  const count = visibleTools.value.length
  const runningCount = visibleTools.value.filter((tool) => tool.status === 'running').length
  if (!count) {
    return running.value ? 'No tool calls' : 'No recent tools'
  }
  if (runningCount) {
    return `${runningCount} running / ${count} total`
  }
  return `${count} tool call${count === 1 ? '' : 's'}`
})
const activityLabel = computed(() => {
  if (runStatus.value === 'answering') {
    return 'Streaming response'
  }
  if (runStatus.value === 'tool') {
    return 'Using tools'
  }
  if (runStatus.value === 'completed') {
    return 'Run complete'
  }
  if (runStatus.value === 'error') {
    return 'Run failed'
  }
  if (runStatus.value === 'thinking') {
    return 'Thinking'
  }
  return 'Agent activity'
})

async function syncPosition() {
  const placement = await invoke<{ side: 'left' | 'right' }>('sync_chat_window_position')
  placementSide.value = placement.side
}

function applySettings(settings: GatewayPublicSettings) {
  gatewayUrl.value = settings.gatewayUrl || 'ws://localhost:18789'
  sessionKey.value = settings.sessionKey || 'agent:main:main'
  autoConnect.value = settings.autoConnect
  hasGatewayToken.value = settings.hasGatewayToken
  gatewayToken.value = ''
}

function applyStatus(status: GatewayStatus) {
  connectionState.value = status.state
  connectionMessage.value = status.message || ''
  if (status.sessionKey) {
    sessionKey.value = status.sessionKey
  }
}

function contentText(content: unknown): string {
  if (!content) {
    return ''
  }

  if (typeof content === 'string') {
    return content
  }

  if (Array.isArray(content)) {
    return content
      .map((block) => {
        if (!block || typeof block !== 'object') {
          return ''
        }

        const record = block as Record<string, unknown>
        if (typeof record.text === 'string') {
          return record.text
        }
        if (typeof record.value === 'string') {
          return record.value
        }
        return ''
      })
      .filter(Boolean)
      .join('\n')
  }

  if (typeof content === 'object') {
    return JSON.stringify(content)
  }

  return String(content)
}

function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' ? value as Record<string, unknown> : {}
}

function textField(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() ? value : undefined
}

function numberField(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}

function normalizedText(value: string) {
  return value.replace(/\s+/g, ' ').trim()
}

function hasRecentDuplicateUserMessage(text: string, timestamp?: number) {
  const normalized = normalizedText(text)
  const now = timestamp || Date.now()

  return messages.value
    .slice(-8)
    .some((message) => {
      if (message.role !== 'user' || normalizedText(message.text) !== normalized) {
        return false
      }

      if (!message.timestamp || !timestamp) {
        return true
      }

      return Math.abs(now - message.timestamp) < 5000
    })
}

function pushMessage(message: ChatMessage) {
  const existing = messages.value.findIndex((item) => item.id === message.id)
  if (existing >= 0) {
    messages.value[existing] = message
  }
  else {
    messages.value.push(message)
  }
  void scrollToBottom()
}

async function scrollToBottom() {
  await nextTick()
  const el = messagesRef.value
  if (el) {
    el.scrollTop = el.scrollHeight
  }
}

function updateActiveAssistantMessage(text: string, streaming: boolean) {
  if (!text && !streaming) {
    return
  }

  const id = activeAssistantMessageId.value || `stream-${activeRunId.value || Date.now()}`
  activeAssistantMessageId.value = id
  pushMessage({ id, role: 'assistant', text, streaming })
}

function finalizeActiveAssistant(text?: string, timestamp?: number, release = true) {
  const id = activeAssistantMessageId.value
  if (!id) {
    return false
  }

  const existing = messages.value.find((item) => item.id === id)
  if (!existing) {
    return false
  }

  pushMessage({
    ...existing,
    text: text || existing.text,
    timestamp,
    streaming: false,
  })
  if (release) {
    activeAssistantMessageId.value = null
  }
  return true
}

function upsertTool(item: ToolTimelineItem) {
  const existing = tools.value.findIndex((tool) => tool.id === item.id)
  if (existing >= 0) {
    tools.value[existing] = {
      ...tools.value[existing],
      ...item,
      startedAt: tools.value[existing].startedAt || item.startedAt,
    }
  }
  else {
    tools.value.push(item)
  }

  if (tools.value.length > 40) {
    tools.value = tools.value.slice(-40)
  }
}

function completeRunningTools(runId?: string) {
  const endedAt = Date.now()
  tools.value = tools.value.map((tool) => {
    if (tool.status !== 'running') {
      return tool
    }
    if (runId && tool.runId && tool.runId !== runId) {
      return tool
    }
    return {
      ...tool,
      status: 'completed',
      endedAt: tool.endedAt || endedAt,
    }
  })
}

function collapseActivity() {
  activityExpanded.value = false
}

function toolStatusLabel(status: ToolStatus) {
  if (status === 'completed') {
    return 'Done'
  }
  if (status === 'error') {
    return 'Error'
  }
  return 'Running'
}

function formatToolTime(value?: number) {
  if (!value) {
    return ''
  }
  return new Date(value).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })
}

async function saveSettings() {
  settingsBusy.value = true
  try {
    const input: Record<string, unknown> = {
      gatewayUrl: gatewayUrl.value,
      sessionKey: sessionKey.value,
      autoConnect: autoConnect.value,
    }

    if (gatewayToken.value.trim()) {
      input.gatewayToken = gatewayToken.value.trim()
    }

    const settings = await invoke<GatewayPublicSettings>('gateway_save_settings', { input })
    applySettings(settings)
    connectionMessage.value = 'Settings saved'
  }
  catch (error) {
    connectionState.value = 'error'
    connectionMessage.value = error instanceof Error ? error.message : String(error)
  }
  finally {
    settingsBusy.value = false
  }
}

async function connectGateway() {
  await saveSettings()
  if (!autoConnect.value) {
    await invoke('gateway_connect')
  }
}

async function disconnectGateway() {
  await invoke('gateway_disconnect')
}

async function submitMessage() {
  const text = draft.value.trim()
  if (!text || sending.value) {
    return
  }

  sending.value = true
  try {
    await invoke('gateway_send_message', { message: text })
    draft.value = ''
    running.value = true
  }
  catch (error) {
    connectionState.value = 'error'
    connectionMessage.value = error instanceof Error ? error.message : String(error)
  }
  finally {
    sending.value = false
  }
}

async function abortRun() {
  try {
    await invoke('gateway_abort')
    running.value = false
    runStatus.value = 'idle'
  }
  catch (error) {
    connectionState.value = 'error'
    connectionMessage.value = error instanceof Error ? error.message : String(error)
  }
}

function handleHistory(payload: { messages?: unknown[] }) {
  const history = payload.messages || []
  messages.value = history.slice(-30).map((message, index) => {
    const record = (message || {}) as Record<string, unknown>
    const role: MessageRole = record.role === 'user' || record.role === 'assistant' ? record.role : 'system'
    return {
      id: `history-${index}-${record.timestamp || ''}`,
      role,
      text: contentText(record.content),
      timestamp: typeof record.timestamp === 'number' ? record.timestamp : undefined,
    }
  }).filter((message) => message.text)
  activeAssistantMessageId.value = null
  void scrollToBottom()
}

function handleSessionMessage(messagePayload: Record<string, unknown>) {
  const message = asRecord(messagePayload.message)
  const role = message.role === 'user' || message.role === 'assistant' ? message.role : 'system'
  const text = contentText(message.content)

  if (!text) {
    return
  }

  const timestamp = numberField(message.timestamp)
  if (role === 'user' && hasRecentDuplicateUserMessage(text, timestamp)) {
    return
  }

  if (role === 'assistant' && finalizeActiveAssistant(text, numberField(message.timestamp))) {
    running.value = false
    runStatus.value = 'completed'
    completeRunningTools(activeRunId.value || undefined)
    collapseActivity()
    activeRunId.value = null
    return
  }

  const id = `session-${messagePayload.messageSeq || message.timestamp || Date.now()}`
  pushMessage({
    id,
    role,
    text,
    timestamp,
  })

  if (role === 'assistant') {
    running.value = false
    runStatus.value = 'completed'
    completeRunningTools(activeRunId.value || undefined)
    collapseActivity()
    activeRunId.value = null
  }
}

function handleAgentAssistant(eventPayload: Record<string, unknown>, data: Record<string, unknown>) {
  running.value = true
  runStatus.value = 'answering'

  const runId = textField(eventPayload.runId) || activeRunId.value || 'current'
  activeRunId.value = runId

  if (!activeAssistantMessageId.value) {
    activeAssistantMessageId.value = `stream-${runId}`
  }

  const existing = messages.value.find((item) => item.id === activeAssistantMessageId.value)
  const text = textField(data.text)
    || `${existing?.text || ''}${textField(data.delta) || ''}`

  updateActiveAssistantMessage(text, true)
}

function handleAgentItem(eventPayload: Record<string, unknown>, data: Record<string, unknown>) {
  running.value = true
  runStatus.value = 'tool'

  const runId = textField(eventPayload.runId) || activeRunId.value || undefined
  if (runId) {
    activeRunId.value = runId
  }

  const itemId = textField(data.itemId)
    || textField(data.toolCallId)
    || `${runId || 'tool'}-${eventPayload.seq || tools.value.length}`
  const phase = textField(data.phase)
  const rawStatus = textField(data.status)
  const status: ToolStatus = rawStatus === 'error' || rawStatus === 'failed'
    ? 'error'
    : phase === 'end' || rawStatus === 'completed' || rawStatus === 'done' || rawStatus === 'succeeded'
      ? 'completed'
      : 'running'
  const title = textField(data.title)
    || textField(data.name)
    || textField(data.kind)
    || 'Tool call'

  upsertTool({
    id: itemId,
    runId,
    title,
    name: textField(data.name),
    meta: textField(data.meta),
    status,
    startedAt: phase === 'end' ? undefined : numberField(eventPayload.ts) || Date.now(),
    endedAt: phase === 'end' || status !== 'running' ? numberField(eventPayload.ts) || Date.now() : undefined,
    error: textField(data.error) || textField(data.message),
  })

  if (status === 'error') {
    runStatus.value = 'error'
  }
}

function handleAgentLifecycle(eventPayload: Record<string, unknown>, data: Record<string, unknown>) {
  const runId = textField(eventPayload.runId)
  if (runId) {
    activeRunId.value = runId
  }

  const phase = data.phase
  if (phase === 'started') {
    running.value = true
    runStatus.value = 'thinking'
    return
  }

  if (phase === 'completed') {
    running.value = false
    runStatus.value = 'completed'
    finalizeActiveAssistant(undefined, numberField(data.endedAt), false)
    completeRunningTools(runId || undefined)
    collapseActivity()
    activeRunId.value = null
    return
  }

  if (phase === 'error') {
    running.value = false
    runStatus.value = 'error'
    finalizeActiveAssistant(undefined, numberField(data.endedAt), false)
    completeRunningTools(runId || undefined)
    collapseActivity()
    connectionState.value = 'error'
    connectionMessage.value = textField(data.error) || 'Agent run failed'
  }
}

function handleSessionTool(toolPayload: Record<string, unknown>) {
  const itemId = textField(toolPayload.itemId)
    || textField(toolPayload.toolCallId)
    || textField(toolPayload.id)
    || `session-tool-${Date.now()}`
  const rawStatus = textField(toolPayload.status)
  const status: ToolStatus = rawStatus === 'error' || rawStatus === 'failed'
    ? 'error'
    : rawStatus === 'completed' || rawStatus === 'done'
      ? 'completed'
      : 'running'

  upsertTool({
    id: itemId,
    runId: textField(toolPayload.runId),
    title: textField(toolPayload.title) || textField(toolPayload.name) || 'Tool call',
    name: textField(toolPayload.name),
    meta: textField(toolPayload.meta),
    status,
    startedAt: numberField(toolPayload.startedAt) || Date.now(),
    endedAt: numberField(toolPayload.endedAt),
    error: textField(toolPayload.error) || textField(toolPayload.message),
  })
}

function handleGatewayEvent(payload: { event: string; payload: Record<string, unknown> }) {
  if (payload.event === 'session.message') {
    handleSessionMessage(payload.payload || {})
    return
  }

  if (payload.event === 'agent') {
    const eventPayload = payload.payload || {}
    const data = asRecord(eventPayload.data)
    const stream = eventPayload.stream

    if (stream === 'lifecycle') {
      handleAgentLifecycle(eventPayload, data)
      return
    }

    if (stream === 'assistant') {
      handleAgentAssistant(eventPayload, data)
      return
    }

    if (stream === 'item') {
      handleAgentItem(eventPayload, data)
    }
    return
  }

  if (payload.event === 'session.tool') {
    handleSessionTool(payload.payload || {})
    return
  }

  if (payload.event === 'chat') {
    const state = payload.payload?.state
    running.value = state === 'processing' || state === 'delta'
    if (running.value && runStatus.value === 'idle') {
      runStatus.value = 'thinking'
    }
    if (state === 'final') {
      runStatus.value = 'completed'
      finalizeActiveAssistant(undefined, undefined, false)
      completeRunningTools(activeRunId.value || undefined)
      collapseActivity()
      activeRunId.value = null
    }
    return
  }

  if (payload.event === 'shutdown') {
    connectionState.value = 'offline'
    connectionMessage.value = 'Gateway shutdown'
    running.value = false
    runStatus.value = 'idle'
  }
}

onMounted(async () => {
  unlistenPlacement = await listen<{ side: 'left' | 'right' }>('chat:placement', (event) => {
    placementSide.value = event.payload.side
  })
  unlistenStatus = await listen<GatewayStatus>('gateway:status', (event) => {
    applyStatus(event.payload)
  })
  unlistenHistory = await listen<{ messages?: unknown[] }>('gateway:history', (event) => {
    handleHistory(event.payload)
  })
  unlistenGatewayEvent = await listen<{ event: string; payload: Record<string, unknown> }>('gateway:event', (event) => {
    handleGatewayEvent(event.payload)
  })

  const settings = await invoke<GatewayPublicSettings>('gateway_get_settings')
  applySettings(settings)
  const status = await invoke<GatewayStatus>('gateway_get_status')
  applyStatus(status)
  await syncPosition()
})

onUnmounted(() => {
  if (unlistenPlacement) {
    unlistenPlacement()
    unlistenPlacement = null
  }
  if (unlistenStatus) {
    unlistenStatus()
    unlistenStatus = null
  }
  if (unlistenHistory) {
    unlistenHistory()
    unlistenHistory = null
  }
  if (unlistenGatewayEvent) {
    unlistenGatewayEvent()
    unlistenGatewayEvent = null
  }
})
</script>

<template>
  <main class="chat-shell">
    <section
      class="chat"
      :class="`chat--${placementSide}`"
    >
      <header class="chat__header">
        <div>
          <h1>Anima</h1>
          <p>
            Gateway {{ connectionState }}
            <span v-if="sessionKey && connectionState === 'connected'"> / {{ sessionKey }}</span>
          </p>
        </div>
        <span
          class="chat__status-dot"
          :class="`chat__status-dot--${connectionState}`"
        />
      </header>

      <details class="settings">
        <summary>Gateway Settings</summary>
        <div class="settings__grid">
          <label>
            <span>URL</span>
            <input
              v-model="gatewayUrl"
              type="text"
              spellcheck="false"
            >
          </label>
          <label>
            <span>Token</span>
            <input
              v-model="gatewayToken"
              type="password"
              :placeholder="hasGatewayToken ? 'Saved; leave blank to keep' : 'Gateway token'"
            >
          </label>
          <label>
            <span>Session</span>
            <input
              v-model="sessionKey"
              type="text"
              spellcheck="false"
            >
          </label>
          <label class="settings__check">
            <input
              v-model="autoConnect"
              type="checkbox"
            >
            <span>Auto connect</span>
          </label>
        </div>
        <div class="settings__actions">
          <button
            type="button"
            :disabled="settingsBusy"
            @click="saveSettings"
          >
            Save
          </button>
          <button
            type="button"
            :disabled="settingsBusy"
            @click="connectGateway"
          >
            Connect
          </button>
          <button
            type="button"
            @click="disconnectGateway"
          >
            Disconnect
          </button>
        </div>
        <p
          v-if="connectionMessage"
          class="settings__message"
        >
          {{ connectionMessage }}
        </p>
      </details>

      <section
        v-if="hasActivity"
        class="activity"
        aria-label="Agent activity"
      >
        <div class="activity__header">
          <button
            type="button"
            :aria-expanded="activityExpanded"
            @click="activityExpanded = !activityExpanded"
          >
            <strong>{{ activityLabel }}</strong>
            <small>{{ toolSummary }}</small>
          </button>
          <span v-if="running">Live</span>
        </div>
        <ol
          v-if="activityExpanded && visibleTools.length"
          class="tool-timeline"
        >
          <li
            v-for="tool in visibleTools"
            :key="tool.id"
            class="tool"
            :class="`tool--${tool.status}`"
          >
            <div class="tool__main">
              <strong>{{ tool.title }}</strong>
              <span>{{ toolStatusLabel(tool.status) }}</span>
            </div>
            <p v-if="tool.meta || tool.name">
              {{ tool.meta || tool.name }}
            </p>
            <small>
              {{ formatToolTime(tool.startedAt) }}
              <template v-if="tool.endedAt"> -> {{ formatToolTime(tool.endedAt) }}</template>
            </small>
            <p
              v-if="tool.error"
              class="tool__error"
            >
              {{ tool.error }}
            </p>
          </li>
        </ol>
      </section>

      <div
        ref="messagesRef"
        class="chat__messages"
      >
        <article
          v-for="message in messages"
          :key="message.id"
          class="message"
          :class="`message--${message.role}`"
        >
          <span>{{ message.text }}</span>
          <em v-if="message.streaming">Streaming</em>
        </article>
      </div>

      <form
        class="composer"
        @submit.prevent="submitMessage"
      >
        <textarea
          v-model="draft"
          rows="2"
          placeholder="Message Anima"
          :disabled="connectionState !== 'connected' || sending"
        />
        <button
          v-if="running"
          type="button"
          @click="abortRun"
        >
          Stop
        </button>
        <button
          v-else
          type="submit"
          :disabled="connectionState !== 'connected' || sending"
        >
          {{ sending ? 'Sending' : 'Send' }}
        </button>
      </form>
    </section>
  </main>
</template>

<style scoped>
.chat-shell {
  box-sizing: border-box;
  width: 100vw;
  height: 100vh;
  padding: 0;
  background: transparent;
}

.chat {
  position: relative;
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 100%;
  border: 1px solid rgba(39, 50, 48, 0.14);
  border-radius: 8px;
  background: rgba(249, 251, 248, 0.92);
  box-shadow: 0 18px 42px rgba(25, 34, 32, 0.18);
  backdrop-filter: blur(18px);
  overflow: hidden;
}

.chat::after {
  position: absolute;
  top: 70px;
  width: 13px;
  height: 13px;
  border-top: 1px solid rgba(39, 50, 48, 0.14);
  border-right: 1px solid rgba(39, 50, 48, 0.14);
  background: rgba(249, 251, 248, 0.92);
  content: "";
}

.chat--left::after {
  right: -7px;
  transform: rotate(45deg);
}

.chat--right::after {
  left: -7px;
  transform: rotate(225deg);
}

.chat__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 14px 16px 12px;
  border-bottom: 1px solid rgba(39, 50, 48, 0.1);
}

.chat__header h1 {
  margin: 0;
  font-size: 17px;
  font-weight: 680;
  line-height: 1.2;
}

.chat__header p {
  margin: 3px 0 0;
  color: #66716e;
  font-size: 12px;
  line-height: 1.3;
}

.chat__status-dot {
  width: 9px;
  height: 9px;
  border-radius: 999px;
  background: #9aada6;
  box-shadow: 0 0 0 4px rgba(154, 173, 166, 0.16);
  flex: 0 0 auto;
}

.chat__status-dot--connected {
  background: #1d9a6c;
  box-shadow: 0 0 0 4px rgba(29, 154, 108, 0.16);
}

.chat__status-dot--reconnecting {
  background: #d8962c;
  box-shadow: 0 0 0 4px rgba(216, 150, 44, 0.18);
}

.chat__status-dot--error {
  background: #c64c42;
  box-shadow: 0 0 0 4px rgba(198, 76, 66, 0.18);
}

.settings {
  border-bottom: 1px solid rgba(39, 50, 48, 0.1);
  padding: 8px 12px 10px;
}

.settings summary {
  color: #32413e;
  cursor: default;
  font-size: 12px;
  font-weight: 650;
  line-height: 1.4;
}

.settings__grid {
  display: grid;
  grid-template-columns: 1fr;
  gap: 8px;
  margin-top: 9px;
}

.settings label {
  display: grid;
  gap: 4px;
  color: #5b6764;
  font-size: 11px;
  line-height: 1.25;
}

.settings input[type="text"],
.settings input[type="password"] {
  box-sizing: border-box;
  min-width: 0;
  width: 100%;
  border: 1px solid rgba(39, 50, 48, 0.16);
  border-radius: 7px;
  padding: 7px 8px;
  background: rgba(255, 255, 255, 0.74);
  color: #1f2d2a;
  font-size: 12px;
  outline: none;
}

.settings__check {
  display: flex !important;
  grid-template-columns: none !important;
  align-items: center;
  flex-direction: row;
}

.settings__actions {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 8px;
  margin-top: 9px;
}

.settings__actions button {
  min-width: 0;
  border: 1px solid rgba(39, 50, 48, 0.16);
  border-radius: 7px;
  padding: 7px 8px;
  background: rgba(255, 255, 255, 0.82);
  color: #22302d;
  font-size: 12px;
  font-weight: 650;
}

.settings__message {
  margin: 8px 0 0;
  color: #6d5550;
  font-size: 11px;
  line-height: 1.35;
  overflow-wrap: anywhere;
}

.chat__messages {
  flex: 1;
  padding: 14px 16px;
  overflow: auto;
}

.message {
  max-width: 100%;
  color: #1f2d2a;
  font-size: 13px;
  line-height: 1.55;
}

.message + .message {
  margin-top: 9px;
}

.message span {
  display: inline-block;
  padding: 9px 11px;
  border-radius: 8px;
  background: rgba(255, 255, 255, 0.72);
  overflow-wrap: anywhere;
  white-space: pre-wrap;
}

.message em {
  display: inline-block;
  margin-left: 8px;
  color: #6f7a76;
  font-size: 10px;
  font-style: normal;
  line-height: 1;
  vertical-align: middle;
}

.message--user {
  text-align: right;
}

.message--user span {
  background: rgba(45, 111, 99, 0.12);
}

.message--system span {
  background: rgba(216, 150, 44, 0.16);
  color: #59452b;
}

.activity {
  flex: 0 0 auto;
  max-height: 126px;
  padding: 7px 12px;
  border-bottom: 1px solid rgba(39, 50, 48, 0.1);
  overflow: auto;
}

.activity__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  color: #53605c;
  font-size: 11px;
  line-height: 1.25;
}

.activity__header button {
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  gap: 2px;
  min-width: 0;
  border: 0;
  padding: 0 0 0 13px;
  background: transparent;
  color: inherit;
  text-align: left;
}

.activity__header button::before {
  position: absolute;
  margin-top: 2px;
  margin-left: -13px;
  color: #64706c;
  content: "▸";
}

.activity__header button[aria-expanded="true"]::before {
  content: "▾";
}

.activity__header strong {
  color: #263431;
  font-weight: 680;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.activity__header small {
  color: #72807b;
  font-size: 10px;
  font-weight: 520;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.activity__header span {
  border-radius: 999px;
  padding: 3px 6px;
  background: rgba(29, 154, 108, 0.12);
  color: #1d6f55;
  font-size: 10px;
  font-weight: 680;
}

.tool-timeline {
  display: grid;
  gap: 8px;
  margin: 9px 0 0;
  padding: 0;
  list-style: none;
}

.tool {
  position: relative;
  padding: 8px 9px 8px 13px;
  border: 1px solid rgba(39, 50, 48, 0.1);
  border-radius: 8px;
  background: rgba(255, 255, 255, 0.62);
}

.tool::before {
  position: absolute;
  top: 10px;
  bottom: 10px;
  left: 6px;
  width: 3px;
  border-radius: 999px;
  background: #d8962c;
  content: "";
}

.tool--completed::before {
  background: #1d9a6c;
}

.tool--error::before {
  background: #c64c42;
}

.tool__main {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 8px;
}

.tool__main strong {
  min-width: 0;
  color: #22302d;
  font-size: 12px;
  font-weight: 680;
  line-height: 1.3;
  overflow-wrap: anywhere;
}

.tool__main span {
  flex: 0 0 auto;
  color: #6a7672;
  font-size: 10px;
  font-weight: 680;
  line-height: 1.4;
}

.tool p {
  margin: 4px 0 0;
  color: #63706c;
  font-size: 11px;
  line-height: 1.35;
  overflow-wrap: anywhere;
}

.tool small {
  display: block;
  margin-top: 4px;
  color: #7c8783;
  font-size: 10px;
  line-height: 1.25;
}

.tool__error {
  color: #8a3831 !important;
}

.composer {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 72px;
  gap: 8px;
  padding: 12px;
  border-top: 1px solid rgba(39, 50, 48, 0.1);
}

.composer textarea {
  box-sizing: border-box;
  width: 100%;
  min-width: 0;
  resize: none;
  border: 1px solid rgba(39, 50, 48, 0.16);
  border-radius: 8px;
  padding: 9px 10px;
  background: rgba(255, 255, 255, 0.74);
  color: #1f2d2a;
  font-size: 13px;
  line-height: 1.35;
  outline: none;
}

.composer textarea:focus {
  border-color: rgba(27, 117, 100, 0.48);
}

.composer button {
  border: 0;
  border-radius: 8px;
  background: #2d6f63;
  color: #ffffff;
  font-size: 13px;
  font-weight: 650;
}

.composer button:disabled,
.composer textarea:disabled,
.settings__actions button:disabled {
  opacity: 0.56;
}
</style>
