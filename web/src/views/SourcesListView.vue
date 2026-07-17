<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { sourcesApi } from '../api'
import { formatApiError } from '../api/client'
import type { SourceAccountPublic } from '../api/types'
import JobProgress from '../components/JobProgress.vue'

const list = ref<SourceAccountPublic[]>([])
const error = ref('')
const loading = ref(false)
const jobId = ref<string | null>(null)

async function load() {
  loading.value = true
  error.value = ''
  try {
    list.value = await sourcesApi.list()
  } catch (e) {
    error.value = formatApiError(e)
    list.value = []
  } finally {
    loading.value = false
  }
}

async function fetchNow(id: string) {
  error.value = ''
  try {
    const res = await sourcesApi.fetchNow(id, false)
    jobId.value = res.job_id
  } catch (e) {
    error.value = formatApiError(e)
  }
}

async function pause(s: SourceAccountPublic) {
  error.value = ''
  const next = s.status === 'paused' ? 'active' : 'paused'
  try {
    await sourcesApi.patch(s.id, { expected_version: s.version, status: next })
    await load()
  } catch (e) {
    error.value = formatApiError(e)
  }
}

onMounted(load)
</script>

<template>
  <section class="panel">
    <header class="panel__head">
      <div>
        <h1>监测公众号</h1>
        <p class="hint">目标公众号列表。与「采集登录账号」不同，请勿混用。</p>
      </div>
      <RouterLink class="btn primary" to="/sources/add">添加监测公众号</RouterLink>
    </header>

    <p v-if="error" class="error-text">{{ error }}</p>
    <JobProgress v-if="jobId" :job-id="jobId" @done="load" />

    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th>名称</th>
            <th>等级</th>
            <th>通道</th>
            <th>下次心跳</th>
            <th>上次抓取</th>
            <th>状态</th>
            <th>操作</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="s in list" :key="s.id">
            <td>
              <RouterLink :to="`/sources/${s.id}`">{{ s.display_name }}</RouterLink>
            </td>
            <td>{{ s.fetch_rules?.base_level || '—' }}</td>
            <td>{{ s.preferred_connector || '—' }}</td>
            <td>{{ s.fetch_rules?.next_heartbeat_at || '—' }}</td>
            <td>{{ s.last_fetch_at || '—' }}</td>
            <td>
              {{ s.status }}
              <span v-if="s.status === 'needs_action'" class="tag warn">需处理</span>
            </td>
            <td class="row">
              <button class="btn small" type="button" @click="fetchNow(s.id)">立即抓取</button>
              <button class="btn small ghost" type="button" @click="pause(s)">
                {{ s.status === 'paused' ? '恢复' : '暂停' }}
              </button>
            </td>
          </tr>
          <tr v-if="!loading && !list.length">
            <td colspan="7" class="hint">暂无监测公众号（列表 API 可能为空占位）。可走添加向导。</td>
          </tr>
        </tbody>
      </table>
    </div>
  </section>
</template>
