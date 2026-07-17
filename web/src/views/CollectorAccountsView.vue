<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { collectorApi, settingsApi } from '../api'
import { formatApiError } from '../api/client'
import type { CollectorAccountPublic, EgressProfilePublic } from '../api/types'
import QrLoginModal from '../components/QrLoginModal.vue'

const accounts = ref<CollectorAccountPublic[]>([])
const egress = ref<EgressProfilePublic[]>([])
const error = ref('')
const loading = ref(false)

const qrOpen = ref(false)
const connector = ref('mp_admin')
const fixedEgressId = ref('')

async function load() {
  loading.value = true
  error.value = ''
  try {
    const [list, profiles] = await Promise.all([
      collectorApi.list(),
      settingsApi.listEgress().catch(() => [] as EgressProfilePublic[]),
    ])
    accounts.value = list
    egress.value = profiles.filter(
      (p) => p.kind === 'local_direct' || p.kind === 'fixed' || p.status === 'active',
    )
    if (!fixedEgressId.value && egress.value.length) {
      const local = egress.value.find((p) => p.kind === 'local_direct')
      fixedEgressId.value = (local || egress.value[0]).id
    }
  } catch (e) {
    error.value = formatApiError(e)
    accounts.value = []
  } finally {
    loading.value = false
  }
}

function openQr() {
  if (!fixedEgressId.value) {
    error.value = '请先选择固定出口（或本机出口）'
    return
  }
  qrOpen.value = true
}

async function probe(id: string) {
  error.value = ''
  try {
    await collectorApi.probeNow(id)
    await load()
  } catch (e) {
    error.value = formatApiError(e)
  }
}

async function toggle(acc: CollectorAccountPublic) {
  error.value = ''
  const next = acc.status === 'disabled' ? 'active' : 'disabled'
  try {
    await collectorApi.patch(acc.id, { status: next })
    await load()
  } catch (e) {
    error.value = formatApiError(e)
  }
}

async function reauth(id: string) {
  error.value = ''
  try {
    await collectorApi.reauthorize(id, {
      fixed_egress_id: fixedEgressId.value || undefined,
    })
    openQr()
    await load()
  } catch (e) {
    error.value = formatApiError(e)
  }
}

function onQrSuccess() {
  qrOpen.value = false
  void load()
}

onMounted(load)
</script>

<template>
  <section class="panel">
    <header class="panel__head">
      <div>
        <h1>采集登录账号</h1>
        <p class="hint">供 connector 使用的扫码会话，不是监测公众号。</p>
      </div>
    </header>

    <div class="toolbar">
      <label>
        Connector
        <select v-model="connector">
          <option value="mp_admin">公众号管理 (mp_admin)</option>
          <option value="weread">微信读书 (weread)</option>
          <option value="fake">测试假账号 (fake)</option>
        </select>
      </label>
      <label>
        固定出口
        <select v-model="fixedEgressId">
          <option disabled value="">请选择</option>
          <option v-for="p in egress" :key="p.id" :value="p.id">
            {{ p.name }} ({{ p.kind }})
          </option>
        </select>
      </label>
      <button class="btn primary" type="button" @click="openQr">扫码登录</button>
      <button class="btn ghost" type="button" :disabled="loading" @click="load">刷新</button>
    </div>

    <p v-if="error" class="error-text">{{ error }}</p>

    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th>名称</th>
            <th>Connector</th>
            <th>状态</th>
            <th>出口</th>
            <th>最后预检</th>
            <th>成功 / 失败</th>
            <th>操作</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="a in accounts" :key="a.id">
            <td>
              {{ a.display_name }}
              <span v-if="a.requires_relogin" class="tag warn">需重授权</span>
            </td>
            <td>{{ a.connector }}</td>
            <td>{{ a.status }}</td>
            <td>{{ a.fixed_egress_id?.slice(0, 8) || '—' }}</td>
            <td>{{ a.last_probe_at || '—' }}</td>
            <td>
              <small>{{ a.last_success_at || '—' }}</small>
              /
              <small>{{ a.last_failure_at || '—' }}</small>
              <div v-if="a.last_error_code" class="error-text">{{ a.last_error_code }}</div>
            </td>
            <td class="row">
              <button class="btn small" type="button" @click="probe(a.id)">立即检查</button>
              <button class="btn small ghost" type="button" @click="reauth(a.id)">重新扫码</button>
              <button class="btn small ghost" type="button" @click="toggle(a)">
                {{ a.status === 'disabled' ? '启用' : '停用' }}
              </button>
            </td>
          </tr>
          <tr v-if="!loading && !accounts.length">
            <td colspan="7" class="hint">暂无采集登录账号。请扫码创建。</td>
          </tr>
        </tbody>
      </table>
    </div>

    <QrLoginModal
      v-model:open="qrOpen"
      :connector="connector"
      :fixed-egress-id="fixedEgressId"
      @success="onQrSuccess"
    />
  </section>
</template>
