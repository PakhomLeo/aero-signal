<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import SharedAccountNotice from '../components/SharedAccountNotice.vue'
import { formatApiError } from '../api/client'
import { useAuthStore } from '../stores/auth'

const auth = useAuthStore()
const router = useRouter()

const currentPassword = ref('')
const newPassword = ref('')
const confirmPassword = ref('')
const error = ref('')
const loading = ref(false)

const forced = () => auth.mustChangePassword

async function onSubmit() {
  error.value = ''
  if (newPassword.value !== confirmPassword.value) {
    error.value = '两次输入的新密码不一致'
    return
  }
  if (newPassword.value.length < 8) {
    error.value = '新密码至少 8 位'
    return
  }
  loading.value = true
  try {
    await auth.changePassword(currentPassword.value, newPassword.value)
    await router.replace({ name: 'overview' })
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
      <h1>修改密码</h1>
      <p v-if="forced()" class="subtitle">首次登录必须修改初始密码后才能继续。</p>
      <SharedAccountNotice />
      <form class="form" @submit.prevent="onSubmit">
        <label>
          当前密码
          <input v-model="currentPassword" type="password" autocomplete="current-password" required />
        </label>
        <label>
          新密码
          <input v-model="newPassword" type="password" autocomplete="new-password" required />
        </label>
        <label>
          确认新密码
          <input v-model="confirmPassword" type="password" autocomplete="new-password" required />
        </label>
        <p v-if="error" class="error-text">{{ error }}</p>
        <button class="btn primary" type="submit" :disabled="loading">
          {{ loading ? '提交中…' : '保存并继续' }}
        </button>
      </form>
    </div>
  </main>
</template>
