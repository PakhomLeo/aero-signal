<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { articlesApi } from '../api'
import { formatApiError } from '../api/client'
import type { ArticleDetailDto } from '../api/types'
import JobProgress from '../components/JobProgress.vue'
import { loadStoredJob, saveStoredJob } from '../utils/jobStorage'

const route = useRoute()
const router = useRouter()
const id = () => String(route.params.id)
const jobStorageKey = computed(() => `article.${id()}.refetch`)

const detail = ref<ArticleDetailDto | null>(null)
const editMd = ref('')
const changeReason = ref('')
const error = ref('')
const ok = ref('')
const jobId = ref<string | null>(loadStoredJob(jobStorageKey.value))
const jobIds = ref<string[]>([])
const editing = ref(false)

watch(jobId, (v) => saveStoredJob(jobStorageKey.value, v))

const article = computed(() => detail.value?.article)

async function load() {
  error.value = ''
  try {
    detail.value = await articlesApi.get(id())
    editMd.value = detail.value.article.content_md
  } catch (e) {
    error.value = formatApiError(e)
  }
}

async function save() {
  error.value = ''
  ok.value = ''
  try {
    detail.value = await articlesApi.patchContent(
      id(),
      editMd.value,
      changeReason.value || undefined,
      detail.value?.article.current_revision,
    )
    editing.value = false
    ok.value = '已保存新版本'
  } catch (e) {
    error.value = formatApiError(e)
  }
}

async function reocr() {
  error.value = ''
  try {
    const res = await articlesApi.reocr(id())
    jobIds.value = res.job_ids.map(String)
    if (jobIds.value[0]) jobId.value = jobIds.value[0]
  } catch (e) {
    error.value = formatApiError(e)
  }
}

async function refetch() {
  error.value = ''
  try {
    const res = await articlesApi.refetch(id())
    jobId.value = res.job_id
  } catch (e) {
    error.value = formatApiError(e)
  }
}

async function restore(rev: number) {
  error.value = ''
  try {
    detail.value = await articlesApi.restore(id(), rev)
    editMd.value = detail.value.article.content_md
    ok.value = `已恢复到版本 ${rev}`
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
        <button class="btn ghost small" type="button" @click="router.back()">← 返回</button>
        <h1>{{ article?.original_title || '文章详情' }}</h1>
      </div>
      <div class="row">
        <button class="btn" type="button" @click="editing = !editing">
          {{ editing ? '取消编辑' : '编辑 Markdown' }}
        </button>
        <button class="btn ghost" type="button" @click="reocr">重新 OCR</button>
        <button class="btn ghost" type="button" @click="refetch">按链接重抓</button>
      </div>
    </header>

    <p v-if="error" class="error-text">{{ error }}</p>
    <p v-if="ok" class="ok-text">{{ ok }}</p>
    <JobProgress
      v-if="jobId"
      :job-id="jobId"
      :storage-key="jobStorageKey"
      @done="load"
    />

    <div v-if="article" class="detail-grid">
      <div class="full">
        <span class="label">原文链接</span>
        <a :href="article.original_url" target="_blank" rel="noopener noreferrer">
          {{ article.original_url }}
        </a>
      </div>
      <div><span class="label">状态</span>{{ article.status }}</div>
      <div><span class="label">抓取时间</span>{{ article.fetched_at }}</div>
      <div><span class="label">当前版本</span>{{ article.current_revision }}</div>
      <div><span class="label">作者</span>{{ article.author || '—' }}</div>
    </div>

    <h2 class="section-title">Markdown</h2>
    <div v-if="editing" class="stack">
      <textarea v-model="editMd" rows="18" class="md-editor" />
      <label>
        变更说明
        <input v-model="changeReason" placeholder="可选" />
      </label>
      <button class="btn primary" type="button" @click="save">保存为新版本</button>
    </div>
    <pre v-else class="md-preview">{{ article?.content_md }}</pre>

    <h2 class="section-title">版本历史</h2>
    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th>#</th>
            <th>编辑者</th>
            <th>原因</th>
            <th>时间</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="r in detail?.revisions || []" :key="r.id">
            <td>{{ r.revision }}</td>
            <td>{{ r.editor_kind }}</td>
            <td>{{ r.change_reason }}</td>
            <td>{{ r.created_at }}</td>
            <td>
              <button
                v-if="r.revision !== article?.current_revision"
                class="btn small ghost"
                type="button"
                @click="restore(r.revision)"
              >
                恢复
              </button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <p class="hint">OCR 文本已合并进 Markdown；重新 OCR 会入队任务并产生新版本。</p>
  </section>
</template>

<style scoped>
.md-editor,
.md-preview {
  width: 100%;
  font-family: ui-monospace, Consolas, monospace;
  font-size: 0.85rem;
  line-height: 1.45;
  white-space: pre-wrap;
  word-break: break-word;
  background: #fff;
  border: 1px solid #d0d7e2;
  border-radius: 6px;
  padding: 0.75rem;
  margin: 0;
}
.md-editor {
  resize: vertical;
}
</style>
