<script setup lang="ts">
import { getCurrentWindow } from '@tauri-apps/api/window'
import ChatWindow from './components/ChatWindow.vue'
import PetWindow from './components/PetWindow.vue'

const windowLabel = getCurrentWindow().label
</script>

<template>
  <PetWindow v-if="windowLabel === 'pet'" />
  <ChatWindow v-else-if="windowLabel === 'chat'" />
  <PetWindow v-else />
</template>
