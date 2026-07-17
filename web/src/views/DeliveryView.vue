<script setup lang="ts">
import { onMounted, ref, watch } from 'vue'
import { deliveryApi, sourcesApi } from '../api'
import { formatApiError } from '../api/client'
import type {
  ApiTokenPublic,
  DeliveryGroupPublic,
  SourceAccountPublic,
} from '../api/types'

const groups = ref<DeliveryGroupPublic[]>([])
const sources = ref<SourceAccountPublic[]>([])
const selectedId = ref<string | null>(null)
const tokens = ref<ApiTokenPublic[]>([])
const error = ref('')
const ok = ref('')

const newName = ref('')
const newDesc = ref('')
const selectedSources = ref<string[]>([])

const tokenName = ref('')
const tokenScopes = ref(['articles:read', 'sources:read', 'service_health:read'])
const plaintextOnce = ref<string | null>(null)

async function loadGroups() {
  error.value = ''
  try {
    groups.value = await deliveryApi.listGroups()
    sources.value = await sourcesApi.list().catch(() => [])
    if (!selectedId.value && groups.value[0]) {
      selectedId.value = groups.value[0].id
    }
  } catch (e) {
    error.value = formatApiError(e)
  }
}

async function loadTokens() {
  if (!selectedId.value) {
    tokens.value = []
    return
  }
  try {
    tokens.value = await deliveryApi.listTokens(selectedId.value)
  } catch (e) {
    error.value = formatApiError(e)
  }
}

async function createGroup() {
  error.value = ''
  ok.value = ''
  try {
    const g = await deliveryApi.createGroup({
      name: newName.value.trim(),
      description: newDesc.value || undefined,
      source_account_ids: selectedSources.value,
    })
    newName.value = ''
    newDesc.value = ''
    selectedSources.value = []
    await loadGroups()
    selectedId.value = g.id
    ok.value = '分组已创建'
  } catch (e) {
    error.value = formatApiError(e)
  }
}

async function issueToken() {
  if (!selectedId.value) return
  error.value = ''
  plaintextOnce.value = null
  try {
    const res = await deliveryApi.issueToken(selectedId.value, {
      name: tokenName.value.trim() || 'default',
      scopes: tokenScopes.value,
    })
    plaintextOnce.value = res.plaintext
    tokenName.value = ''
    await loadTokens()
  } catch (e) {
    error.value = formatApiError(e)
  }
}

async function revoke(tokenId: string) {
  if (!selectedId.value) return
  try {
    await deliveryApi.revokeToken(selectedId.value, tokenId)
    await loadTokens()
  } catch (e) {
    error.value = formatApiError(e)
  }
}

async function copyPlain() {
  if (!plaintextOnce.value) return
  try {
    await navigator.clipboard.writeText(plaintextOnce.value)
    ok.value = '已复制明文 Token（仅此一次可见）'
  } catch {
    ok.value = '请手动复制下方明文'
  }
}

watch(selectedId, () => {
  plaintextOnce.value = null
  void loadTokens()
})

onMounted(async () => {
  await loadGroups()
  await loadTokens()
})
</script>

<template>
  <section class="panel">
    <header class="panel__head">
      <h1>分组与 API</h1>
    </header>

    <p v-if="error" class="error-text">{{ error }}</p>
    <p v-if="ok" class="ok-text">{{ ok }}</p>

    <div class="split">
      <div>
        <h2 class="section-title">分组</h2>
        <ul class="list">
          <li
            v-for="g in groups"
            :key="g.id"
            :class="{ active: g.id === selectedId }"
            @click="selectedId = g.id"
          >
            <strong>{{ g.name }}</strong>
            <span class="hint">{{ g.status }} · {{ g.source_account_ids.length }} 个公众号</span>
          </li>
          <li v-if="!groups.length" class="hint">暂无分组</li>
        </ul>

        <h3 class="section-title">新建分组</h3>
        <form class="form" @submit.prevent="createGroup">
          <label>
            名称
            <input v-model="newName" required />
          </label>
          <label>
            描述
            <input v-model="newDesc" />
          </label>
          <fieldset>
            <legend>监测公众号</legend>
            <label v-for="s in sources" :key="s.id" class="check">
              <input v-model="selectedSources" type="checkbox" :value="s.id" />
              {{ s.display_name }}
            </label>
            <p v-if="!sources.length" class="hint">暂无可选公众号（列表可能为空）</p>
          </fieldset>
          <button class="btn primary" type="submit">创建</button>
        </form>
      </div>

      <div>
        <h2 class="section-title">Token</h2>
        <p class="hint">明文 Token 仅在签发时展示一次，之后只显示前缀。</p>

        <div v-if="plaintextOnce" class="plaintext-box">
          <strong>请立即复制（仅此一次）</strong>
          <code>{{ plaintextOnce }}</code>
          <button class="btn small" type="button" @click="copyPlain">复制</button>
          <button class="btn small ghost" type="button" @click="plaintextOnce = null">关闭</button>
        </div>

        <form class="form" @submit.prevent="issueToken">
          <label>
            Token 名称
            <input v-model="tokenName" required />
          </label>
          <fieldset>
            <legend>Scopes</legend>
            <label class="check">
              <input v-model="tokenScopes" type="checkbox" value="articles:read" />
              articles:read
            </label>
            <label class="check">
              <input v-model="tokenScopes" type="checkbox" value="sources:read" />
              sources:read
            </label>
            <label class="check">
              <input v-model="tokenScopes" type="checkbox" value="service_health:read" />
              service_health:read
            </label>
          </fieldset>
          <button class="btn primary" type="submit" :disabled="!selectedId">签发 Token</button>
        </form>

        <div class="table-wrap" style="margin-top: 1rem">
          <table>
            <thead>
              <tr>
                <th>名称</th>
                <th>前缀</th>
                <th>状态</th>
                <th>最后使用</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="t in tokens" :key="t.id">
                <td>{{ t.name }}</td>
                <td><code>{{ t.token_prefix }}…</code></td>
                <td>{{ t.status }}</td>
                <td>{{ t.last_used_at || '—' }}</td>
                <td>
                  <button
                    v-if="t.status === 'active'"
                    class="btn small ghost"
                    type="button"
                    @click="revoke(t.id)"
                  >
                    撤销
                  </button>
                </td>
              </tr>
              <tr v-if="selectedId && !tokens.length">
                <td colspan="5" class="hint">该分组尚无 Token</td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.split {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 1.5rem;
}
@media (max-width: 900px) {
  .split {
    grid-template-columns: 1fr;
  }
}
.list {
  list-style: none;
  padding: 0;
  margin: 0 0 1rem;
}
.list li {
  padding: 0.6rem 0.75rem;
  border: 1px solid #d0d7e2;
  border-radius: 6px;
  margin-bottom: 0.4rem;
  cursor: pointer;
  display: flex;
  flex-direction: column;
  gap: 0.15rem;
  background: #fff;
}
.list li.active {
  border-color: #2b6cb0;
  background: #ebf8ff;
}
.check {
  display: flex;
  align-items: center;
  gap: 0.4rem;
  font-weight: 400;
}
.plaintext-box {
  background: #fffaf0;
  border: 1px solid #ed8936;
  border-radius: 8px;
  padding: 0.75rem;
  margin-bottom: 1rem;
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}
.plaintext-box code {
  word-break: break-all;
  font-size: 0.85rem;
}
</style>
