<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { jobsApi } from '../api'
import { formatApiError } from '../api/client'
import JobProgress from '../components/JobProgress.vue'

const route = useRoute()
const router = useRouter()
const jobId = computed(() => String(route.params.id || ''))
const cancelError = ref('')
const cancelling = ref(false)

const canCancel = computed(() => {
  const s = jobProgress.value?.status
  return s && !['succeeded', 'failed', 'cancelled', 'dead', 'needs_action'].includes(s)
})

const jobProgress = ref<{ status: string } | null>(null)

async function cancelJob() {
  if (!jobId.value) return
  cancelling.value = true
  cancelError.value = ''
  try {
    await jobsApi.cancel(jobId.value)
    await jobProgressRef.value?.refresh()
  } catch (e) {
    cancelError.value = formatApiError(e)
  } finally {
    cancelling.value = false
  }
}

const jobProgressRef = ref<InstanceType<typeof JobProgress> | null>(null)
</script>

<template>
  <section class="panel">
    <header class="panel__head">
      <div>
        <button class="btn ghost small" type="button" @click="router.back()">← 返回</button>
        <h1>任务详情</h1>
      </div>
      <button
        v-if="canCancel"
        class="btn ghost"
        type="button"
        :disabled="cancelling"
        @click="cancelJob"
      >
        取消任务
      </button>
    </header>
    <p v-if="cancelError" class="error-text">{{ cancelError }}</p>
    <JobProgress
      ref="jobProgressRef"
      :job-id="jobId"
      @done="(j) => (jobProgress = j)"
      @failed="(j) => (jobProgress = j)"
    />
  </section>
</template>
