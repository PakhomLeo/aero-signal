<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import { sourcesApi } from '../api'
import { ApiError, formatApiError } from '../api/client'
import type { PatchSourceRequest, SourceAccountPublic, ValidateSourceResponse } from '../api/types'
import JobProgress from '../components/JobProgress.vue'

const route = useRoute()
const router = useRouter()
const id = () => String(route.params.id)

const source = ref<SourceAccountPublic | null>(null)
const error = ref('')
const success = ref('')
const conflict = ref(false)
const saving = ref(false)
const jobId = ref<string | null>(null)
const verify = ref<ValidateSourceResponse | null>(null)

const form = reactive({
  display_name: '',
  base_level: 'LO0',
  enabled: true,
  connector_override: '',
  interval_override_hours: null as number | null,
  manual_next_heartbeat_at: '',
  auto_promotion_enabled: true,
  working_windows: [] as Array<{ start: string; end: string }>,
})

function toLocalDateTime(value: string | null): string {
  if (!value) return ''
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  const shifted = new Date(date.getTime() - date.getTimezoneOffset() * 60_000)
  return shifted.toISOString().slice(0, 16)
}

function applySource(next: SourceAccountPublic) {
  source.value = next
  form.display_name = next.display_name
  const rules = next.fetch_rules
  if (!rules) {
    form.working_windows = [{ start: '09:00', end: '18:00' }]
    return
  }
  form.base_level = rules.base_level
  form.enabled = rules.enabled
  form.connector_override = rules.connector_override ?? ''
  form.interval_override_hours = rules.interval_override_hours
  form.manual_next_heartbeat_at = toLocalDateTime(rules.manual_next_heartbeat_at)
  form.auto_promotion_enabled = rules.auto_promotion_enabled
  form.working_windows = rules.working_windows.map((window) => ({ ...window }))
}

async function load() {
  error.value = ''
  success.value = ''
  conflict.value = false
  try {
    applySource(await sourcesApi.get(id()))
  } catch (e) {
    error.value = formatApiError(e)
  }
}

function showMutationError(e: unknown) {
  if (e instanceof ApiError && e.status === 409) {
    conflict.value = true
    error.value = '数据已被其他操作更新'
    return
  }
  error.value = `操作失败：${formatApiError(e)}`
}

async function save() {
  if (!source.value) return
  error.value = ''
  success.value = ''
  conflict.value = false
  saving.value = true
  const body: PatchSourceRequest = {
    expected_version: source.value.version,
    display_name: form.display_name.trim(),
    base_level: form.base_level,
    enabled: form.enabled,
    working_windows: form.working_windows.map((window) => ({ ...window })),
    connector_override: form.connector_override.trim() || null,
    interval_override_hours:
      typeof form.interval_override_hours === 'number' && Number.isFinite(form.interval_override_hours)
        ? form.interval_override_hours
        : null,
    manual_next_heartbeat_at: form.manual_next_heartbeat_at
      ? new Date(form.manual_next_heartbeat_at).toISOString()
      : null,
    auto_promotion_enabled: form.auto_promotion_enabled,
  }
  try {
    applySource(await sourcesApi.patch(id(), body))
    success.value = '配置已保存'
  } catch (e) {
    showMutationError(e)
  } finally {
    saving.value = false
  }
}

async function fetchNow(recalculate = false) {
  error.value = ''
  try {
    jobId.value = (await sourcesApi.fetchNow(id(), recalculate)).job_id
  } catch (e) {
    error.value = `抓取任务创建失败：${formatApiError(e)}`
  }
}

async function pause() {
  if (!source.value) return
  error.value = ''
  conflict.value = false
  const status = source.value.status === 'paused' ? 'active' : 'paused'
  try {
    applySource(await sourcesApi.patch(id(), {
      expected_version: source.value.version,
      status,
    }))
  } catch (e) {
    showMutationError(e)
  }
}

async function verifyAgain() {
  error.value = ''
  try {
    verify.value = await sourcesApi.verifyAgain(id())
  } catch (e) {
    error.value = `重新验证失败：${formatApiError(e)}`
  }
}

function addWindow() {
  form.working_windows.push({ start: '09:00', end: '18:00' })
}

onMounted(load)
</script>

<template>
  <section class="panel">
    <header class="panel__head">
      <div>
        <button class="btn ghost small" type="button" @click="router.push('/sources')">← 返回列表</button>
        <h1>{{ source?.display_name || '监测公众号详情' }}</h1>
      </div>
      <div class="row">
        <button class="btn" type="button" @click="fetchNow(false)">立即抓取</button>
        <button class="btn ghost" type="button" @click="fetchNow(true)">抓取并重算心跳</button>
        <button class="btn ghost" type="button" @click="pause">
          {{ source?.status === 'paused' ? '恢复' : '暂停' }}
        </button>
        <button class="btn ghost" type="button" @click="verifyAgain">重新验证三篇文章</button>
      </div>
    </header>

    <p v-if="error" class="error-text">{{ error }}</p>
    <p v-if="success" class="ok-text">{{ success }}</p>
    <button v-if="conflict" class="btn ghost" type="button" @click="load">重新加载最新数据</button>
    <JobProgress v-if="jobId" :job-id="jobId" @done="load" />

    <template v-if="source">
      <div class="detail-grid">
        <div><span class="label">状态</span>{{ source.status }}</div>
        <div><span class="label">首选通道</span>{{ source.preferred_connector || '—' }}</div>
        <div><span class="label">添加时间</span>{{ source.added_at }}</div>
        <div><span class="label">上次抓取</span>{{ source.last_fetch_at || '—' }}</div>
        <div><span class="label">首次心跳（只读）</span>{{ source.fetch_rules?.first_heartbeat_at || '—' }}</div>
        <div><span class="label">计划下次心跳</span>{{ source.fetch_rules?.next_heartbeat_at || '—' }}</div>
        <div class="full">
          <span class="label">强标识</span>
          <ul class="plain">
            <li v-for="(sid, index) in source.strong_ids" :key="index">
              {{ sid.id_type }}: {{ sid.id_value }} ({{ sid.confidence }})
            </li>
          </ul>
        </div>
      </div>

      <h2 class="section-title">抓取配置</h2>
      <form class="form narrow" @submit.prevent="save">
        <label>显示名称<input v-model="form.display_name" required /></label>
        <label>
          抓取等级
          <select v-model="form.base_level">
            <option value="LO0">LO0</option>
            <option value="LO1">LO1</option>
            <option value="L1">L1</option>
            <option value="L2">L2</option>
            <option value="L3">L3</option>
            <option value="L4">L4（不参与自动计划）</option>
          </select>
        </label>
        <label><input v-model="form.enabled" type="checkbox" /> 启用定时抓取</label>
        <label>
          通道覆盖（留空则清除）
          <input v-model="form.connector_override" placeholder="例如 mp_admin" />
        </label>
        <label>
          自定义间隔（小时，留空则清除）
          <input v-model.number="form.interval_override_hours" type="number" min="1" step="1" />
        </label>
        <label>
          手动指定下次心跳（留空则清除）
          <input v-model="form.manual_next_heartbeat_at" type="datetime-local" />
        </label>
        <label><input v-model="form.auto_promotion_enabled" type="checkbox" /> 允许自动提升抓取等级</label>

        <fieldset>
          <legend>工作时间窗口</legend>
          <div v-for="(window, index) in form.working_windows" :key="index" class="row">
            <input v-model="window.start" type="time" required />
            <span>至</span>
            <input v-model="window.end" type="time" required />
            <button class="btn ghost small" type="button" @click="form.working_windows.splice(index, 1)">删除</button>
          </div>
          <button class="btn ghost small" type="button" @click="addWindow">添加时间窗口</button>
        </fieldset>

        <button class="btn primary" type="submit" :disabled="saving">
          {{ saving ? '保存中…' : '保存配置' }}
        </button>
      </form>
    </template>

    <div v-if="source?.status === 'needs_action'" class="needs-action-box">
      该监测公众号需要人工处理。请检查采集账号授权、出口可用性，或重新验证三篇文章。
    </div>

    <div v-if="verify" class="stack" style="margin-top: 1rem">
      <h2 class="section-title">验证结果</h2>
      <p>成功 {{ verify.run.successful_count }} / 候选 {{ verify.run.candidate_count }} · 状态 {{ verify.run.status }}</p>
      <ul>
        <li v-for="(candidate, index) in verify.run.candidates" :key="index">
          #{{ index + 1 }} {{ candidate.title || candidate.canonical_url || '候选文章' }} — {{ candidate.status || '—' }}
          <RouterLink v-if="candidate.article_id" :to="`/articles/${candidate.article_id}`">文章</RouterLink>
        </li>
      </ul>
    </div>
  </section>
</template>
