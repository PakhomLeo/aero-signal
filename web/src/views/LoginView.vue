<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import SharedAccountNotice from '../components/SharedAccountNotice.vue'
import { formatApiError } from '../api/client'
import { useAppStore } from '../stores/app'
import { useAuthStore } from '../stores/auth'

const app = useAppStore()
const auth = useAuthStore()
const router = useRouter()

const username = ref('')
const password = ref('')
const error = ref('')
const loading = ref(false)

async function onSubmit() {
  error.value = ''
  loading.value = true
  try {
    await auth.login(username.value.trim(), password.value)
    if (auth.mustChangePassword) {
      await router.replace({ name: 'change-password' })
    } else {
      await router.replace({ name: 'overview' })
    }
  } catch (e) {
    error.value = formatApiError(e)
  } finally {
    loading.value = false
  }
}
</script>

<template>
  <main class="auth-page">
    <div class="auth-card">
      <h1 class="brand">{{ app.productName }}</h1>
      <p class="subtitle">管理后台登录</p>
      <SharedAccountNotice />
      <form class="form" @submit.prevent="onSubmit">
        <label>
          账号
          <input v-model="username" autocomplete="username" required />
        </label>
        <label>
          密码
          <input v-model="password" type="password" autocomplete="current-password" required />
        </label>
        <p v-if="error" class="error-text">{{ error }}</p>
        <button class="btn primary" type="submit" :disabled="loading">
          {{ loading ? '登录中…' : '登录' }}
        </button>
      </form>
      <p class="hint">无注册入口。首次登录后须修改初始密码。</p>
    </div>
  </main>
</template>
