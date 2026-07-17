import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { authApi } from '../api'
import {
  clearSession,
  loadSession,
  saveSession,
  type SessionSnapshot,
} from '../api/client'
import type { AdministratorPublic } from '../api/types'

export const useAuthStore = defineStore('auth', () => {
  const token = ref<string | null>(null)
  const expiresAt = ref<string | null>(null)
  const username = ref<string | null>(null)
  const mustChangePassword = ref(false)
  const me = ref<AdministratorPublic | null>(null)
  const bootstrapped = ref(false)

  const isAuthenticated = computed(() => Boolean(token.value))

  function applySession(s: SessionSnapshot) {
    token.value = s.token
    expiresAt.value = s.expiresAt
    username.value = s.username
    mustChangePassword.value = s.mustChangePassword
    saveSession(s)
  }

  function hydrate() {
    const s = loadSession()
    if (s) {
      token.value = s.token
      expiresAt.value = s.expiresAt
      username.value = s.username
      mustChangePassword.value = s.mustChangePassword
    }
    bootstrapped.value = true
  }

  async function login(user: string, password: string) {
    const res = await authApi.login(user, password)
    applySession({
      token: res.token,
      expiresAt: res.expires_at,
      username: res.admin.username,
      mustChangePassword: res.admin.must_change_password,
    })
    me.value = res.admin
    return res
  }

  async function fetchMe() {
    const profile = await authApi.getMe()
    me.value = profile
    username.value = profile.username
    mustChangePassword.value = profile.must_change_password
    if (token.value && expiresAt.value) {
      applySession({
        token: token.value,
        expiresAt: expiresAt.value,
        username: profile.username,
        mustChangePassword: profile.must_change_password,
      })
    }
    return profile
  }

  async function changePassword(currentPassword: string, newPassword: string) {
    const profile = await authApi.patchMe({
      current_password: currentPassword,
      new_password: newPassword,
    })
    me.value = profile
    mustChangePassword.value = profile.must_change_password
    if (token.value && expiresAt.value) {
      applySession({
        token: token.value,
        expiresAt: expiresAt.value,
        username: profile.username,
        mustChangePassword: profile.must_change_password,
      })
    }
    return profile
  }

  async function logout() {
    try {
      if (token.value) await authApi.logout()
    } catch {
      /* 忽略网络错误，仍清理本地会话 */
    }
    token.value = null
    expiresAt.value = null
    username.value = null
    mustChangePassword.value = false
    me.value = null
    clearSession()
  }

  return {
    token,
    expiresAt,
    username,
    mustChangePassword,
    me,
    bootstrapped,
    isAuthenticated,
    hydrate,
    login,
    fetchMe,
    changePassword,
    logout,
  }
})
