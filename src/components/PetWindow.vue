<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { Config, LogLevel, Priority } from 'easy-live2d'
import { onMounted, onUnmounted, ref } from 'vue'
import { createLive2DRuntime } from '../live2d/runtime'

const MODEL_URL = '/Resources/Yulia/Yulia.model3.json'
const FOLLOW_THROTTLE_MS = 50

const canvasRef = ref<HTMLCanvasElement>()
const loadError = ref('')
const modelReady = ref(false)

const currentWindow = getCurrentWindow()
let runtime: ReturnType<typeof createLive2DRuntime> | null = null
let resizeObserver: ResizeObserver | null = null
let unlistenMoved: UnlistenFn | null = null
let unlistenResized: UnlistenFn | null = null
let followTimer: number | null = null

Config.MotionGroupIdle = 'Idle'
Config.ViewScale = 1.8
Config.MouseFollow = true
Config.CubismLoggingLevel = LogLevel.LogLevel_Off

function syncChatSoon() {
  if (followTimer !== null) {
    return
  }

  followTimer = window.setTimeout(() => {
    followTimer = null
    void invoke('sync_chat_window_position').catch((error) => {
      console.warn('failed to sync chat window position', error)
    })
  }, FOLLOW_THROTTLE_MS)
}

async function mountYulia() {
  const canvas = canvasRef.value
  if (!canvas) {
    return
  }

  runtime = createLive2DRuntime({
    canvas,
    resizeTo: window,
    resolution: Math.max(window.devicePixelRatio || 1, 1),
  })

  await runtime.init()
  const sprite = await runtime.mountModel({
    modelEntryUrl: MODEL_URL,
    onReady: () => {
      modelReady.value = true
      syncChatSoon()
    },
  })

  if (sprite) {
    await sprite.startRandomMotion({
      group: 'Idle',
      priority: Priority.Idle,
    })
  }

  resizeObserver = new ResizeObserver(() => {
    void runtime?.syncSize()
    syncChatSoon()
  })
  resizeObserver.observe(canvas)
}

onMounted(async () => {
  try {
    await mountYulia()
    unlistenMoved = await currentWindow.onMoved(syncChatSoon)
    unlistenResized = await currentWindow.onResized(() => {
      void runtime?.syncSize()
      syncChatSoon()
    })
    syncChatSoon()
  }
  catch (error) {
    loadError.value = error instanceof Error ? error.message : String(error)
  }
})

onUnmounted(() => {
  if (followTimer !== null) {
    window.clearTimeout(followTimer)
    followTimer = null
  }
  if (unlistenMoved) {
    unlistenMoved()
  }
  if (unlistenResized) {
    unlistenResized()
  }
  resizeObserver?.disconnect()
  runtime?.dispose()
  runtime = null
})
</script>

<template>
  <main
    class="pet"
    data-tauri-drag-region
  >
    <canvas
      ref="canvasRef"
      class="pet__canvas"
      data-tauri-drag-region
    />
    <div
      v-if="loadError"
      class="pet__status"
    >
      {{ loadError }}
    </div>
    <div
      v-else-if="!modelReady"
      class="pet__status"
    >
      Loading Yulia
    </div>
  </main>
</template>

<style scoped>
.pet {
  position: relative;
  width: 100vw;
  height: 100vh;
  overflow: hidden;
  background: transparent;
}

.pet__canvas {
  display: block;
  width: 100%;
  height: 100%;
  background: transparent;
}

.pet__status {
  position: absolute;
  left: 50%;
  bottom: 20px;
  max-width: calc(100% - 32px);
  transform: translateX(-50%);
  padding: 7px 10px;
  border-radius: 6px;
  background: rgba(255, 255, 255, 0.82);
  color: #273434;
  font-size: 12px;
  line-height: 1.35;
  text-align: center;
  pointer-events: none;
  overflow-wrap: anywhere;
}
</style>
