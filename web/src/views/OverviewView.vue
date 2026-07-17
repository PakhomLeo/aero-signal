<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import { overviewApi } from '../api'
import { ApiError, formatApiError } from '../api/client'
import type { DailyPlanListDto, HealthSnapshotAdminDto } from '../api/types'
import JobProgress from '../components/JobProgress.vue'
import { loadStoredJob, saveStoredJob } from '../utils/jobStorage'

const snapshot = ref<HealthSnapshotAdminDto | null>(null)
const plan = ref<DailyPlanListDto | null>(null)
const error = ref('')
const loading = ref(false)
const checkJobId = ref<string | null>(loadStoredJob('overview.check'))
const preflightJobId = ref<string | null>(loadStoredJob('overview.preflight'))
const rebuildJobId = ref<string | null>(loadStoredJob('overview.rebuild'))

watch(checkJobId, (id) => saveStoredJob('overview.check', id))
watch(preflightJobId, (id) => saveStoredJob('overview.preflight', id))
watch(rebuildJobId, (id) => saveStoredJob('overview.rebuild', id))

const planStats = computed(() => {
  if (!plan.value) return null
  const rows = plan.value.rows || []
  const succeeded = rows.filter((r) => r.status === 'succeeded').length
  const failed = rows.filter((r) => r.status === 'failed' || r.status === 'needs_action').length
  const pending = rows.filter((r) =>
    ['planned', 'running', 'retry_wait'].includes(r.status),
  ).length
  return { succeeded, failed, pending, total: rows.length }
})

async function load() {
  loading.value = true
  error.value = ''
  try {
    const [snapRes, planRes] = await Promise.allSettled([
      overviewApi.latestSnapshot(),
      overviewApi.dailyPlan(),
    ])
    if (snapRes.status === 'fulfilled') {
      snapshot.value = snapRes.value
    } else {
      const e = snapRes.reason
      if (e instanceof ApiError && e.status === 404) {
        snapshot.value = null
      } else {
        throw e
      }
    }
    if (planRes.status === 'fulfilled') {
      plan.value = planRes.value
    } else {
      throw planRes.reason
    }
  } catch (e) {
    error.value = formatApiError(e)
  } finally {
    loading.value = false
  }
}

/** 仅重新拉取已保存快照，不触发探测。 */
async function refreshSnapshotOnly() {
  error.value = ''
  try {
    snapshot.value = await overviewApi.latestSnapshot()
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) {
      snapshot.value = null
      error.value = '尚无健康快照'
    } else {
      error.value = formatApiError(e)
    }
  }
}

async function checkNow() {
  error.value = ''
  try {
    const res = await overviewApi.checkNow()
    checkJobId.value = res.job_id
  } catch (e) {
    error.value = formatApiError(e)
  }
}

async function runPreflight() {
  error.value = ''
  try {
    const res = await overviewApi.runPreflight()
    preflightJobId.value = res.job_id
  } catch (e) {
    error.value = formatApiError(e)
  }
}

async function rebuild() {
  error.value = ''
  try {
    const res = await overviewApi.rebuildPlan()
    rebuildJobId.value = res.job_id
  } catch (e) {
    error.value = formatApiError(e)
  }
}

onMounted(load)
</script>

<template>
  <section class="panel">
    <header class="panel__head">
      <h1>今日概览</h1>
      <div class="row">
        <button class="btn" type="button" :disabled="loading" @click="refreshSnapshotOnly">
          刷新快照
        </button>
        <button class="btn" type="button" @click="runPreflight">运行日预检</button>
        <button class="btn ghost" type="button" @click="checkNow">立即检查（不覆盖快照）</button>
        <button class="btn ghost" type="button" @click="rebuild">重建今日计划</button>
      </div>
    </header>

    <p class="hint">
      「刷新快照」只读已保存结果。「运行日预检」入队
      <code>daily_preflight</code>
      并写入当日不可变快照；「立即检查」只记探测 run，不覆盖快照。
    </p>
    <p v-if="error" class="error-text">{{ error }}</p>
    <p v-if="loading" class="hint">加载中…</p>

    <JobProgress
      v-if="preflightJobId"
      :job-id="preflightJobId"
      storage-key="overview.preflight"
      @done="load"
    />
    <JobProgress
      v-if="checkJobId"
      :job-id="checkJobId"
      storage-key="overview.check"
      @done="load"
    />
    <JobProgress
      v-if="rebuildJobId"
      :job-id="rebuildJobId"
      storage-key="overview.rebuild"
      @done="load"
    />

    <div v-if="snapshot" class="grid-stats">
      <div class="stat">
        <span class="stat__label">总体状态</span>
        <strong>{{ snapshot.overall_status }}</strong>
      </div>
      <div class="stat">
        <span class="stat__label">快照日</span>
        <strong>{{ snapshot.snapshot_date }}</strong>
      </div>
      <div class="stat">
        <span class="stat__label">今日可抓 / 阻断</span>
        <strong>{{ snapshot.today_eligible_source_count }} / {{ snapshot.today_blocked_source_count }}</strong>
      </div>
      <div class="stat">
        <span class="stat__label">待处理</span>
        <strong>{{ snapshot.pending_action_count }}</strong>
      </div>
      <div class="stat">
        <span class="stat__label">昨日计划成功 / 失败</span>
        <strong>
          {{ snapshot.yesterday_plan_success_count }} / {{ snapshot.yesterday_plan_failure_count }}
        </strong>
      </div>
      <div class="stat">
        <span class="stat__label">昨日文章失败</span>
        <strong>{{ snapshot.yesterday_article_failure_count }}</strong>
      </div>
    </div>
    <p v-if="snapshot && snapshot.pending_action_count > 0" class="hint">
      有待人工处理项（needs_action）。请到
      <RouterLink to="/sources">监测公众号</RouterLink>
      或
      <RouterLink to="/collector-accounts">采集登录账号</RouterLink>
      检查授权、出口与验证状态。
    </p>
    <p v-else-if="!snapshot && !loading" class="hint">尚无健康快照。</p>

    <h2 class="section-title">组件状态</h2>
    <div v-if="snapshot?.components?.length" class="table-wrap">
      <table>
        <thead>
          <tr>
            <th>类型</th>
            <th>状态</th>
            <th>原因</th>
            <th>检查时间</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="(c, i) in snapshot.components" :key="i">
            <td>{{ c.component_type }}</td>
            <td>{{ c.status }}</td>
            <td>{{ c.reason_code || '—' }}</td>
            <td>{{ c.checked_at }}</td>
            <td>
              <RouterLink
                v-if="c.component_type === 'collector' && c.component_id"
                :to="`/collector-accounts`"
              >
                查看采集账号
              </RouterLink>
              <RouterLink
                v-else-if="c.component_type === 'source' && c.component_id"
                :to="`/sources/${c.component_id}`"
              >
                查看公众号
              </RouterLink>
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <h2 class="section-title">今日计划摘要</h2>
    <div v-if="plan" class="grid-stats">
      <div class="stat">
        <span class="stat__label">计划日</span>
        <strong>{{ plan.plan_date }}</strong>
      </div>
      <div class="stat">
        <span class="stat__label">计划 / 已开始</span>
        <strong>{{ plan.planned_count }} / {{ plan.started_count }}</strong>
      </div>
      <div class="stat" v-if="planStats">
        <span class="stat__label">成功 / 失败 / 待处理</span>
        <strong>{{ planStats.succeeded }} / {{ planStats.failed }} / {{ planStats.pending }}</strong>
      </div>
      <div class="stat">
        <span class="stat__label">可执行</span>
        <strong>{{ plan.can_build_executable ? '是' : '否' }}</strong>
      </div>
    </div>
    <p v-if="plan?.block_reason" class="needs-action-box">
      阻断原因：{{ plan.block_reason }}
    </p>
    <div v-if="plan?.rows?.length" class="table-wrap">
      <table>
        <thead>
          <tr>
            <th>公众号 ID</th>
            <th>计划时间</th>
            <th>状态</th>
            <th>原因</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="r in plan.rows" :key="r.id">
            <td>
              <RouterLink :to="`/sources/${r.source_account_id}`">
                {{ r.source_account_id.slice(0, 8) }}…
              </RouterLink>
            </td>
            <td>{{ r.planned_run_at }}</td>
            <td>{{ r.status }}</td>
            <td>{{ r.reason_code }}</td>
            <td>
              <RouterLink v-if="r.fetch_job_id" :to="`/jobs/${r.fetch_job_id}`">job</RouterLink>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </section>
</template>
