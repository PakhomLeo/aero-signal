<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { authApi } from '../api'
import { formatApiError } from '../api/client'
import SharedAccountNotice from '../components/SharedAccountNotice.vue'
import { useAuthStore } from '../stores/auth'

const auth = useAuthStore()
const router = useRouter()

const username = ref('')
const currentPassword = ref('')
const newPassword = ref('')
const error = ref('')
const ok = ref('')
const loading = ref(false)

onMounted(async () => {
  try {
    const me = await auth.fetchMe()
    username.value = me.username
  } catch (e) {
    error.value = formatApiError(e)
  }
})

async function saveProfile() {
  error.value = ''
  ok.value = ''
  loading.value = true
  try {
    const body: { username?: string; current_password?: string; new_password?: string } = {}
    if (username.value && username.value !== auth.username) body.username = username.value
    if (newPassword.value) {
      body.current_password = currentPassword.value
      body.new_password = newPassword.value
    }
    if (!body.username && !body.new_password) {
      error.value = '没有需要保存的变更'
      return
    }
    await authApi.patchMe(body)
    await auth.fetchMe()
    currentPassword.value = ''
    newPassword.value = ''
    ok.value = '已保存'
  } catch (e) {
    error.value = formatApiError(e)
  } finally {
    loading.value = false
  }
}

async function goChangePassword() {
  await router.push({ name: 'change-password' })
}
</script>

<template>
  <section class="panel">
    <header class="panel__head">
      <h1>账户</h1>
    </header>
    <SharedAccountNotice />
    <div class="stack" style="margin-top: 1rem">
      <p class="meta">
        会话到期：{{ auth.expiresAt || '—' }}
        <span v-if="auth.mustChangePassword"> · 须改密</span>
      </p>
      <form class="form narrow" @submit.prevent="saveProfile">
        <label>
          账号名
          <input v-model="username" required />
        </label>
        <label>
          当前密码（改密时必填）
          <input v-model="currentPassword" type="password" autocomplete="current-password" />
        </label>
        <label>
          新密码（可选）
          <input v-model="newPassword" type="password" autocomplete="new-password" />
        </label>
        <p v-if="error" class="error-text">{{ error }}</p>
        <p v-if="ok" class="ok-text">{{ ok }}</p>
        <div class="row">
          <button class="btn primary" type="submit" :disabled="loading">保存</button>
          <button class="btn ghost" type="button" @click="goChangePassword">强制改密页</button>
        </div>
      </form>
    </div>
  </section>
</template>
