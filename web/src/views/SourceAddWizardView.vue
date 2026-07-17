<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue'
import { useRouter, RouterLink } from 'vue-router'
import { sourcesApi } from '../api'
import { formatApiError } from '../api/client'
import type { ValidateSourceResponse } from '../api/types'

const router = useRouter()
const step = ref(1)
const url = ref('')
const error = ref('')
const loading = ref(false)
const validateRes = ref<ValidateSourceResponse | null>(null)

const displayName = ref('')
const baseLevel = ref('L1')
const firstHeartbeatAt = ref('')
const windowStart = ref('09:00')
const windowEnd = ref('18:00')
const preferredConnector = ref('')
let pollTimer: ReturnType<typeof setTimeout> | undefined
const validationStorageKey = 'signal.source-validation.active'

async function pollValidation(runId: string) {
  try {
    const run = await sourcesApi.getValidation(runId)
    if (!validateRes.value) return
    validateRes.value = { ...validateRes.value, run }
    if (run.status === 'running') {
      pollTimer = setTimeout(() => void pollValidation(runId), 2000)
    }
  } catch (e) {
    error.value = formatApiError(e)
  }
}

onUnmounted(() => {
  if (pollTimer) clearTimeout(pollTimer)
})

onMounted(async () => {
  const runId = sessionStorage.getItem(validationStorageKey)
  if (!runId) return
  try {
    const run = await sourcesApi.getValidation(runId)
    validateRes.value = {
      run,
      existing_source: null,
      idempotency_key: '',
      already_exists: false,
    }
    url.value = run.input_url
    step.value = 2
    if (run.status === 'running') void pollValidation(run.id)
  } catch {
    sessionStorage.removeItem(validationStorageKey)
  }
})

async function doValidate() {
  error.value = ''
  loading.value = true
  try {
    const res = await sourcesApi.validate({
      url: url.value.trim(),
    })
    validateRes.value = res
    sessionStorage.setItem(validationStorageKey, res.run.id)
    if (res.already_exists && res.existing_source) {
      await router.push(`/sources/${res.existing_source.id}`)
      return
    }
    const parsed = res.run.parsed_source_json as { display_name?: string } | undefined
    if (parsed?.display_name) displayName.value = parsed.display_name
    if (!firstHeartbeatAt.value) {
      const d = new Date()
      d.setMinutes(d.getMinutes() + 30)
      firstHeartbeatAt.value = d.toISOString().slice(0, 16)
    }
    step.value = 2
    if (res.run.status === 'running') void pollValidation(res.run.id)
  } catch (e) {
    error.value = formatApiError(e)
  } finally {
    loading.value = false
  }
}

async function doConfirm() {
  if (!validateRes.value) return
  error.value = ''
  loading.value = true
  try {
    const iso = new Date(firstHeartbeatAt.value).toISOString()
    const created = await sourcesApi.confirm({
      validation_run_id: validateRes.value.run.id,
      display_name: displayName.value || undefined,
      base_level: baseLevel.value,
      working_windows: [{ start: windowStart.value, end: windowEnd.value }],
      first_heartbeat_at: iso,
      preferred_connector: preferredConnector.value || undefined,
    })
    sessionStorage.removeItem(validationStorageKey)
    await router.push(`/sources/${created.id}`)
  } catch (e) {
    error.value = formatApiError(e)
  } finally {
    loading.value = false
  }
}
</script>

<template>
  <section class="panel">
    <header class="panel__head">
      <div>
        <button class="btn ghost small" type="button" @click="router.push('/sources')">← 列表</button>
        <h1>添加监测公众号</h1>
      </div>
    </header>

    <p v-if="error" class="error-text">{{ error }}</p>

    <div v-if="step === 1" class="stack">
      <p class="hint">粘贴任一历史文章链接，系统将解析身份并验证最新可发现的前三篇。</p>
      <label>
        文章链接
        <input v-model="url" placeholder="https://mp.weixin.qq.com/s/..." required />
      </label>
      <button class="btn primary" type="button" :disabled="loading || !url.trim()" @click="doValidate">
        {{ loading ? '解析验证中…' : '解析并验证' }}
      </button>
    </div>

    <div v-else class="stack">
      <h2 class="section-title">验证结果</h2>
      <p v-if="validateRes">
        状态 {{ validateRes.run.status }} · 成功
        {{ validateRes.run.successful_count }} / {{ validateRes.run.candidate_count }}
      </p>
      <p v-if="validateRes?.run.missing_reason" class="hint">{{ validateRes.run.missing_reason }}</p>
      <ul>
        <li v-for="(c, i) in validateRes?.run.candidates || []" :key="i">
          #{{ i + 1 }} {{ c.title || c.canonical_url || '候选' }} —
          {{ c.status || '—' }}
          <span v-if="c.error_code" class="error-text">[{{ c.error_code }}]</span>
          <RouterLink v-if="c.article_id" :to="`/articles/${c.article_id}`">查看文章</RouterLink>
        </li>
      </ul>

      <div v-if="validateRes?.run.status === 'running'" class="needs-action-box">
        正在后台发现并验证文章。页面会每 2 秒刷新；刷新页面后可通过验证 run 继续查询。
      </div>

      <div
        v-else-if="validateRes && validateRes.run.successful_count < 1"
        class="needs-action-box"
      >
        零篇成功：确认后将进入 needs_action，不能生成日常计划。建议检查采集登录账号后重试。
      </div>

      <template v-if="validateRes?.run.status === 'succeeded'">
      <h2 class="section-title">首次配置</h2>
      <form class="form narrow" @submit.prevent="doConfirm">
        <label>
          显示名称
          <input v-model="displayName" />
        </label>
        <label>
          等级
          <select v-model="baseLevel">
            <option value="LO0">LO0</option>
            <option value="L1">L1</option>
            <option value="L2">L2</option>
            <option value="L3">L3</option>
          </select>
        </label>
        <label>
          首选 connector（可选）
          <input v-model="preferredConnector" placeholder="mp_admin / weread" />
        </label>
        <label>
          工作窗口起
          <input v-model="windowStart" placeholder="09:00" />
        </label>
        <label>
          工作窗口止
          <input v-model="windowEnd" placeholder="18:00" />
        </label>
        <label>
          首次自动抓取时间
          <input v-model="firstHeartbeatAt" type="datetime-local" required />
        </label>
        <div class="row">
          <button class="btn ghost" type="button" @click="step = 1">上一步</button>
          <button class="btn primary" type="submit" :disabled="loading">确认添加</button>
        </div>
      </form>
      </template>
    </div>
  </section>
</template>
