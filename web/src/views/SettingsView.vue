<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue'
import { settingsApi } from '../api'
import { formatApiError } from '../api/client'
import type {
  CreateProxyProviderRequest,
  EgressProfilePublic,
  ProxyProviderMode,
  ProxyProviderPublic,
  ScheduleSettings,
} from '../api/types'

const schedule = reactive<ScheduleSettings>({
  timezone: 'Asia/Shanghai',
  preflight_time: '07:30',
  plan_start_time: '08:00',
  working_windows: [{ start: '09:00', end: '18:00' }],
  image_min_side_px: 80,
  image_min_area_px: 14400,
  ocr_min_confidence: 0.7,
})

const egress = ref<EgressProfilePublic[]>([])
const providers = ref<ProxyProviderPublic[]>([])
const error = ref('')
const ok = ref('')
const testMsg = ref('')

const newEgress = reactive({
  name: '',
  kind: 'fixed',
  priority: 100,
  allowed_purposes: 'login,discover',
  endpoint_secret: '',
  status: 'active',
})

const newProvider = reactive<{
  name: string
  mode: ProxyProviderMode
  api_base_url: string
  auth_secret: string
  enabled: boolean
}>({
  name: '',
  mode: 'static_endpoint',
  api_base_url: '',
  auth_secret: '',
  enabled: true,
})

async function load() {
  error.value = ''
  try {
    const [s, e, p] = await Promise.all([
      settingsApi.getSchedule(),
      settingsApi.listEgress(),
      settingsApi.listProviders(),
    ])
    Object.assign(schedule, s)
    if (!schedule.working_windows?.length) {
      schedule.working_windows = [{ start: '09:00', end: '18:00' }]
    }
    egress.value = e
    providers.value = p
  } catch (err) {
    error.value = formatApiError(err)
  }
}

async function saveSchedule() {
  error.value = ''
  ok.value = ''
  try {
    const saved = await settingsApi.patchSchedule({ ...schedule })
    Object.assign(schedule, saved)
    ok.value = '调度设置已保存'
  } catch (e) {
    error.value = formatApiError(e)
  }
}

async function createEgress() {
  error.value = ''
  try {
    await settingsApi.createEgress({
      name: newEgress.name,
      kind: newEgress.kind,
      priority: Number(newEgress.priority),
      allowed_purposes: newEgress.allowed_purposes.split(',').map((s) => s.trim()).filter(Boolean),
      endpoint_secret: newEgress.endpoint_secret || undefined,
      status: newEgress.status,
    })
    newEgress.name = ''
    newEgress.endpoint_secret = ''
    await load()
    ok.value = '出口已创建（密钥不回显）'
  } catch (e) {
    error.value = formatApiError(e)
  }
}

async function testEgress(id: string) {
  testMsg.value = ''
  try {
    const r = await settingsApi.testEgress(id)
    testMsg.value = JSON.stringify(r)
  } catch (e) {
    testMsg.value = formatApiError(e)
  }
}

async function patchEgressStatus(p: EgressProfilePublic) {
  const next = p.status === 'active' ? 'disabled' : 'active'
  try {
    await settingsApi.patchEgress(p.id, { status: next })
    await load()
  } catch (e) {
    error.value = formatApiError(e)
  }
}

async function createProvider() {
  error.value = ''
  try {
    const body: CreateProxyProviderRequest = {
      name: newProvider.name,
      mode: newProvider.mode,
      api_base_url: newProvider.api_base_url || null,
      auth_secret: newProvider.auth_secret || undefined,
      enabled: newProvider.enabled,
    }
    await settingsApi.createProvider(body)
    newProvider.name = ''
    newProvider.auth_secret = ''
    await load()
    ok.value = '供应商已创建（密钥不回显）'
  } catch (e) {
    error.value = formatApiError(e)
  }
}

onMounted(load)
</script>

<template>
  <section class="panel">
    <header class="panel__head">
      <h1>设置</h1>
    </header>

    <p v-if="error" class="error-text">{{ error }}</p>
    <p v-if="ok" class="ok-text">{{ ok }}</p>

    <h2 class="section-title">工作窗口与阈值</h2>
    <form class="form narrow" @submit.prevent="saveSchedule">
      <label>
        时区
        <input v-model="schedule.timezone" />
      </label>
      <label>
        预检时间
        <input v-model="schedule.preflight_time" placeholder="07:30" />
      </label>
      <label>
        计划起始
        <input v-model="schedule.plan_start_time" placeholder="08:00" />
      </label>
      <label>
        窗口起
        <input v-model="schedule.working_windows[0].start" />
      </label>
      <label>
        窗口止
        <input v-model="schedule.working_windows[0].end" />
      </label>
      <label>
        图片最小边 (px)
        <input v-model.number="schedule.image_min_side_px" type="number" />
      </label>
      <label>
        图片最小面积
        <input v-model.number="schedule.image_min_area_px" type="number" />
      </label>
      <label>
        OCR 最低置信度
        <input v-model.number="schedule.ocr_min_confidence" type="number" step="0.01" min="0" max="1" />
      </label>
      <button class="btn primary" type="submit">保存调度设置</button>
    </form>

    <h2 class="section-title">出口</h2>
    <p class="hint">已保存的端点密钥永不回显，仅显示是否已配置。</p>
    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th>名称</th>
            <th>种类</th>
            <th>优先级</th>
            <th>用途</th>
            <th>状态</th>
            <th>密钥</th>
            <th>操作</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="p in egress" :key="p.id">
            <td>{{ p.name }}</td>
            <td>{{ p.kind }}</td>
            <td>{{ p.priority }}</td>
            <td>{{ p.allowed_purposes.join(', ') }}</td>
            <td>{{ p.status }}</td>
            <td>{{ p.has_endpoint_secret ? '已配置' : '无' }}</td>
            <td class="row">
              <button class="btn small" type="button" @click="testEgress(p.id)">测试</button>
              <button
                v-if="p.kind !== 'local_direct'"
                class="btn small ghost"
                type="button"
                @click="patchEgressStatus(p)"
              >
                {{ p.status === 'active' ? '停用' : '启用' }}
              </button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <p v-if="testMsg" class="hint">测试结果：{{ testMsg }}</p>

    <form class="form narrow" style="margin-top: 1rem" @submit.prevent="createEgress">
      <h3>新建出口</h3>
      <label>名称 <input v-model="newEgress.name" required /></label>
      <label>
        种类
        <select v-model="newEgress.kind">
          <option value="fixed">fixed</option>
          <option value="dynamic">dynamic</option>
        </select>
      </label>
      <label>优先级 <input v-model.number="newEgress.priority" type="number" /></label>
      <label>
        用途（逗号分隔）
        <input v-model="newEgress.allowed_purposes" />
      </label>
      <label>
        端点密钥（明文仅提交，不回显）
        <input v-model="newEgress.endpoint_secret" type="password" autocomplete="new-password" />
      </label>
      <button class="btn primary" type="submit">创建出口</button>
    </form>

    <h2 class="section-title">代理供应商</h2>
    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th>名称</th>
            <th>模式</th>
            <th>API</th>
            <th>密钥</th>
            <th>启用</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="p in providers" :key="p.id">
            <td>{{ p.name }}</td>
            <td>{{ p.mode }}</td>
            <td>{{ p.api_base_url || '—' }}</td>
            <td>{{ p.has_auth_secret ? '已配置' : '无' }}</td>
            <td>{{ p.enabled ? '是' : '否' }}</td>
          </tr>
          <tr v-if="!providers.length">
            <td colspan="5" class="hint">暂无供应商</td>
          </tr>
        </tbody>
      </table>
    </div>

    <form class="form narrow" style="margin-top: 1rem" @submit.prevent="createProvider">
      <h3>新建供应商</h3>
      <label>名称 <input v-model="newProvider.name" required /></label>
      <label>
        模式
        <select v-model="newProvider.mode">
          <option value="static_endpoint">static_endpoint</option>
          <option value="gateway">gateway</option>
          <option value="vendor_api">vendor_api</option>
        </select>
      </label>
      <label>API Base URL <input v-model="newProvider.api_base_url" /></label>
      <label>
        Auth 密钥（明文仅提交，不回显）
        <input v-model="newProvider.auth_secret" type="password" autocomplete="new-password" />
      </label>
      <button class="btn primary" type="submit">创建供应商</button>
    </form>
  </section>
</template>
