<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from 'vue'
import { collectorApi } from '../api'
import { formatApiError } from '../api/client'
import type { CollectorAccountPublic, QrSessionResponse } from '../api/types'

const props = defineProps<{
  open: boolean
  connector: string
  fixedEgressId: string
}>()

const emit = defineEmits<{
  'update:open': [value: boolean]
  success: [account: CollectorAccountPublic]
}>()

const session = ref<QrSessionResponse | null>(null)
const error = ref('')
const loading = ref(false)
let timer: ReturnType<typeof setTimeout> | null = null
let countdownTimer: ReturnType<typeof setInterval> | null = null
let createGeneration = 0
const nowMs = ref(Date.now())

const statusLabel = computed(() => {
  const s = session.value?.status || ''
  const map: Record<string, string> = {
    waiting: '等待扫码',
    qr_waiting: '等待扫码',
    scanned: '已扫码，待确认',
    qr_scanned: '已扫码，待确认',
    finalizing: '确认中…',
    confirmed: '登录成功',
    active: '登录成功',
    expired: '已过期',
    cancelled: '已取消',
    failed: '失败',
  }
  return map[s] || s || '—'
})

const remainSec = computed(() => {
  if (!session.value?.expires_at) return 0
  const exp = Date.parse(session.value.expires_at)
  return Math.max(0, Math.floor((exp - nowMs.value) / 1000))
})

const doneStatuses = new Set(['confirmed', 'active', 'expired', 'cancelled', 'failed'])

const isFake = computed(() => props.connector === 'fake')
const advancing = ref(false)

const canAdvanceScanned = computed(() => {
  const s = session.value?.status
  return isFake.value && !!session.value?.ticket && (s === 'qr_waiting' || s === 'waiting')
})

const canAdvanceConfirmed = computed(() => {
  const s = session.value?.status
  return (
    isFake.value &&
    !!session.value?.ticket &&
    (s === 'qr_waiting' || s === 'waiting' || s === 'qr_scanned' || s === 'scanned')
  )
})

function stopPoll() {
  if (timer) {
    clearTimeout(timer)
    timer = null
  }
}

function stopCountdown() {
  if (countdownTimer) {
    clearInterval(countdownTimer)
    countdownTimer = null
  }
}

function startCountdown() {
  stopCountdown()
  nowMs.value = Date.now()
  countdownTimer = setInterval(() => {
    nowMs.value = Date.now()
  }, 1000)
}

async function createSession() {
  const generation = ++createGeneration
  const previousTicket = session.value?.ticket
  loading.value = true
  error.value = ''
  stopPoll()
  if (previousTicket) {
    try {
      await collectorApi.cancelQr(previousTicket)
    } catch {
      /* the previous ticket may already be terminal or expired */
    }
  }
  try {
    const next = await collectorApi.createQr({
      connector: props.connector,
      fixed_egress_id: props.fixedEgressId,
    })
    if (generation !== createGeneration) {
      try {
        await collectorApi.cancelQr(next.ticket)
      } catch {
        /* ignore a superseded ticket */
      }
      return
    }
    session.value = next
    startCountdown()
    schedulePoll()
  } catch (e) {
    error.value = formatApiError(e)
    session.value = null
  } finally {
    loading.value = false
  }
}

function schedulePoll() {
  stopPoll()
  if (!session.value || doneStatuses.has(session.value.status)) return
  const delay = Math.min(3000, Math.max(2000, session.value.poll_after_ms || 2500))
  timer = setTimeout(async () => {
    await pollOnce()
    schedulePoll()
  }, delay)
}

async function pollOnce() {
  if (!session.value?.ticket) return
  try {
    const next = await collectorApi.pollQr(session.value.ticket)
    // 仅保留 data URL；不展示任何第三方原始 URL
    session.value = {
      ...next,
      qr_data_url: next.qr_data_url?.startsWith('data:')
        ? next.qr_data_url
        : session.value.qr_data_url,
    }
    if (doneStatuses.has(next.status)) {
      stopPoll()
      if ((next.status === 'confirmed' || next.status === 'active') && next.account) {
        emit('success', next.account)
      }
    }
  } catch (e) {
    error.value = formatApiError(e)
  }
}

async function applyAdvanceResult(next: QrSessionResponse) {
  session.value = {
    ...next,
    qr_data_url: next.qr_data_url?.startsWith('data:')
      ? next.qr_data_url
      : session.value?.qr_data_url || next.qr_data_url,
  }
  if (doneStatuses.has(next.status)) {
    stopPoll()
    if ((next.status === 'confirmed' || next.status === 'active') && next.account) {
      emit('success', next.account)
    }
  } else {
    schedulePoll()
  }
}

async function advanceFake(status: 'scanned' | 'confirmed') {
  if (!session.value?.ticket || advancing.value) return
  advancing.value = true
  error.value = ''
  try {
    const next = await collectorApi.advanceQr(session.value.ticket, status)
    await applyAdvanceResult(next)
  } catch (e) {
    error.value = formatApiError(e)
  } finally {
    advancing.value = false
  }
}

async function cancel() {
  stopPoll()
  if (session.value?.ticket) {
    try {
      await collectorApi.cancelQr(session.value.ticket)
    } catch {
      /* ignore */
    }
  }
  close()
}

function close() {
  createGeneration += 1
  stopPoll()
  stopCountdown()
  session.value = null
  error.value = ''
  emit('update:open', false)
}

watch(
  () => props.open,
  (v) => {
    if (v) void createSession()
    else {
      stopPoll()
      stopCountdown()
      session.value = null
    }
  },
)

onUnmounted(() => {
  stopPoll()
  stopCountdown()
})
</script>

<template>
  <div v-if="open" class="modal-backdrop" @click.self="cancel">
    <div class="modal" role="dialog" aria-modal="true" aria-label="扫码登录">
      <header class="modal__head">
        <h2>采集登录 · 扫码</h2>
        <button type="button" class="btn ghost" @click="cancel">关闭</button>
      </header>

      <p class="hint">仅展示服务端下发的二维码图片（data URL）。不会保存 cookie / 采集 token。</p>

      <div v-if="loading && !session" class="center">正在生成二维码…</div>
      <p v-if="error" class="error-text">{{ error }}</p>

      <div v-if="session" class="qr-body">
        <img
          v-if="session.qr_data_url?.startsWith('data:')"
          :src="session.qr_data_url"
          alt="登录二维码"
          class="qr-img"
        />
        <p v-else class="error-text">二维码不可用（非 data URL，已拒绝展示）</p>
        <p class="status">状态：{{ statusLabel }}</p>
        <p class="countdown">剩余 {{ remainSec }}s</p>
        <p v-if="session.message" class="hint">{{ session.message }}</p>
        <div v-if="isFake" class="fake-actions">
          <p class="hint">Fake 联调：模拟扫码确认</p>
          <div class="fake-actions__btns">
            <button
              type="button"
              class="btn small"
              :disabled="!canAdvanceScanned || advancing || loading"
              @click="advanceFake('scanned')"
            >
              模拟已扫码
            </button>
            <button
              type="button"
              class="btn small primary"
              :disabled="!canAdvanceConfirmed || advancing || loading"
              @click="advanceFake('confirmed')"
            >
              模拟确认
            </button>
          </div>
        </div>
      </div>

      <footer class="modal__foot">
        <button type="button" class="btn" :disabled="loading" @click="createSession">刷新二维码</button>
        <button type="button" class="btn ghost" @click="cancel">取消登录</button>
      </footer>
    </div>
  </div>
</template>

<style scoped>
.modal-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(15, 23, 42, 0.45);
  display: grid;
  place-items: center;
  z-index: 50;
  padding: 1rem;
}
.modal {
  width: min(420px, 100%);
  background: #fff;
  border-radius: 10px;
  padding: 1rem 1.25rem 1.25rem;
  box-shadow: 0 12px 40px rgba(0, 0, 0, 0.18);
}
.modal__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.modal__head h2 {
  margin: 0;
  font-size: 1.1rem;
}
.qr-body {
  text-align: center;
  margin: 1rem 0;
}
.qr-img {
  width: 220px;
  height: 220px;
  object-fit: contain;
  border: 1px solid #e2e8f0;
  border-radius: 6px;
}
.status {
  font-weight: 600;
  margin: 0.75rem 0 0.25rem;
}
.countdown {
  margin: 0;
  color: #4a5568;
  font-size: 0.9rem;
}
.modal__foot {
  display: flex;
  gap: 0.5rem;
  justify-content: flex-end;
}
.center {
  text-align: center;
  padding: 2rem;
  color: #718096;
}
.hint {
  font-size: 0.8rem;
  color: #718096;
  margin: 0.5rem 0 0;
}
.error-text {
  color: #c53030;
  font-size: 0.85rem;
}
.fake-actions {
  margin-top: 0.85rem;
  padding-top: 0.75rem;
  border-top: 1px dashed #e2e8f0;
}
.fake-actions__btns {
  display: flex;
  gap: 0.5rem;
  justify-content: center;
  margin-top: 0.5rem;
}
</style>
