<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from 'vue'
import { jobsApi } from '../api'
import { formatApiError } from '../api/client'
import { labelForErrorCode } from '../api/errorLabels'
import type { JobResponse } from '../api/types'
import { saveStoredJob } from '../utils/jobStorage'

const props = withDefaults(
  defineProps<{
    jobId: string | null
    /** 轮询间隔 ms */
    intervalMs?: number
    autoStart?: boolean
    /** sessionStorage 键后缀，刷新后恢复 job */
    storageKey?: string
  }>(),
  {
    intervalMs: 2000,
    autoStart: true,
  },
)

const emit = defineEmits<{
  done: [job: JobResponse]
  failed: [job: JobResponse]
}>()

const job = ref<JobResponse | null>(null)
const error = ref('')
const polling = ref(false)
let timer: ReturnType<typeof setTimeout> | null = null

const terminal = new Set(['succeeded', 'failed', 'cancelled', 'dead', 'needs_action'])

const isTerminal = computed(() => (job.value ? terminal.has(job.value.status) : false))

const statusLabel = computed(() => {
  if (!job.value) return '等待中'
  const map: Record<string, string> = {
    queued: '排队中',
    pending: '排队中',
    running: '执行中',
    cancelling: '取消中',
    succeeded: '已完成',
    failed: '失败',
    cancelled: '已取消',
    dead: '终止',
    needs_action: '需人工处理',
    retry_wait: '等待重试',
  }
  return map[job.value.status] || job.value.status
})

async function pollOnce() {
  if (!props.jobId) return
  try {
    error.value = ''
    job.value = await jobsApi.get(props.jobId)
    if (isTerminal.value && job.value) {
      stop()
      if (props.storageKey) saveStoredJob(props.storageKey, null)
      if (job.value.status === 'succeeded') emit('done', job.value)
      else emit('failed', job.value)
    }
  } catch (e) {
    error.value = formatApiError(e)
  }
}

function schedule() {
  stopTimer()
  if (!props.jobId || isTerminal.value) return
  polling.value = true
  timer = setTimeout(async () => {
    await pollOnce()
    if (polling.value && !isTerminal.value) schedule()
  }, props.intervalMs)
}

function stopTimer() {
  if (timer) {
    clearTimeout(timer)
    timer = null
  }
}

function stop() {
  polling.value = false
  stopTimer()
}

async function start() {
  if (!props.jobId) return
  stop()
  await pollOnce()
  if (!isTerminal.value) schedule()
}

watch(
  () => props.jobId,
  (id) => {
    job.value = null
    error.value = ''
    if (id && props.storageKey) saveStoredJob(props.storageKey, id)
    if (id && props.autoStart) void start()
    else stop()
  },
  { immediate: true },
)

onUnmounted(stop)

defineExpose({ start, stop, refresh: pollOnce })
</script>

<template>
  <div v-if="jobId" class="job-progress">
    <div class="job-progress__head">
      <span class="job-progress__title">任务进度</span>
      <span class="badge" :data-status="job?.status || 'pending'">{{ statusLabel }}</span>
    </div>
    <p v-if="error" class="error-text">{{ error }}</p>
    <template v-else-if="job">
      <div class="bar">
        <div class="bar__fill" :style="{ width: `${Math.min(100, job.progress_percent)}%` }" />
      </div>
      <p class="meta">
        <span>{{ job.progress_stage || '—' }}</span>
        <span>{{ Math.round(job.progress_percent) }}%</span>
      </p>
      <p v-if="job.progress_message" class="msg">{{ job.progress_message }}</p>
      <p v-if="job.action_code || job.action_message || job.status === 'needs_action'" class="needs-action">
        <strong>需人工处理</strong>
        <span v-if="job.action_code">
          {{ labelForErrorCode(job.action_code) || job.action_code }}
        </span>
        <span v-if="job.action_message">— {{ job.action_message }}</span>
        <span class="hint">
          建议检查
          <a href="/collector-accounts">采集登录账号</a>
          授权与出口，或到
          <a href="/sources">监测公众号</a>
          重新验证。
        </span>
      </p>
      <p v-if="job.last_error_code" class="error-text">
        [{{ job.last_error_code }}] {{ job.last_error_message }}
      </p>
      <p class="hint">job: {{ job.id }} · {{ job.kind }} · 尝试 {{ job.attempt_count }}/{{ job.max_attempts }}</p>
    </template>
    <p v-else class="hint">加载任务状态…</p>
  </div>
</template>

<style scoped>
.job-progress {
  border: 1px solid #d0d7e2;
  background: #f8fafc;
  border-radius: 8px;
  padding: 0.75rem 1rem;
  margin-top: 0.75rem;
}
.job-progress__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
}
.job-progress__title {
  font-weight: 600;
  font-size: 0.9rem;
}
.badge {
  font-size: 0.75rem;
  padding: 0.15rem 0.5rem;
  border-radius: 4px;
  background: #e2e8f0;
}
.badge[data-status='succeeded'] {
  background: #c6f6d5;
  color: #22543d;
}
.badge[data-status='failed'],
.badge[data-status='dead'] {
  background: #fed7d7;
  color: #742a2a;
}
.badge[data-status='needs_action'] {
  background: #feebc8;
  color: #7b341e;
}
.badge[data-status='running'] {
  background: #bee3f8;
  color: #2a4365;
}
.bar {
  height: 6px;
  background: #e2e8f0;
  border-radius: 3px;
  margin: 0.5rem 0;
  overflow: hidden;
}
.bar__fill {
  height: 100%;
  background: #2b6cb0;
  transition: width 0.3s ease;
}
.meta {
  display: flex;
  justify-content: space-between;
  margin: 0;
  font-size: 0.8rem;
  color: #4a5568;
}
.msg,
.hint {
  margin: 0.35rem 0 0;
  font-size: 0.8rem;
  color: #718096;
}
.needs-action {
  margin: 0.5rem 0 0;
  font-size: 0.85rem;
  color: #9c4221;
  background: #fffaf0;
  padding: 0.4rem 0.6rem;
  border-radius: 4px;
}
.error-text {
  color: #c53030;
  font-size: 0.85rem;
  margin: 0.35rem 0 0;
}
</style>
