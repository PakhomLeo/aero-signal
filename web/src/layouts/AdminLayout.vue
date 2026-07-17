<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import SharedAccountNotice from '../components/SharedAccountNotice.vue'
import { useAppStore } from '../stores/app'
import { useAuthStore } from '../stores/auth'

const app = useAppStore()
const auth = useAuthStore()
const route = useRoute()
const router = useRouter()

const nav = [
  { to: '/overview', label: '今日概览' },
  { to: '/collector-accounts', label: '采集登录账号' },
  { to: '/sources', label: '监测公众号' },
  { to: '/delivery', label: '分组与 API' },
  { to: '/settings', label: '设置' },
  { to: '/account', label: '账户' },
]

const activePath = computed(() => route.path)

async function onLogout() {
  await auth.logout()
  await router.push({ name: 'login' })
}
</script>

<template>
  <div class="shell">
    <aside class="side">
      <div class="brand-block">
        <RouterLink class="brand" to="/overview">{{ app.productName }}</RouterLink>
        <p class="brand-sub">管理后台</p>
      </div>
      <nav class="nav">
        <RouterLink
          v-for="item in nav"
          :key="item.to"
          :to="item.to"
          class="nav__link"
          :class="{ active: activePath.startsWith(item.to) }"
        >
          {{ item.label }}
        </RouterLink>
      </nav>
      <div class="side-foot">
        <p class="user">{{ auth.username || '管理员' }}</p>
        <button type="button" class="btn ghost small" @click="onLogout">退出</button>
      </div>
    </aside>
    <div class="main">
      <div class="main-top">
        <SharedAccountNotice />
      </div>
      <div class="main-body">
        <RouterView />
      </div>
    </div>
  </div>
</template>

<style scoped>
.shell {
  display: grid;
  grid-template-columns: 220px 1fr;
  min-height: 100vh;
}
.side {
  background: #1a2332;
  color: #e8eef7;
  display: flex;
  flex-direction: column;
  padding: 1.25rem 1rem;
}
.brand-block {
  margin-bottom: 1.5rem;
}
.brand {
  color: #fff;
  text-decoration: none;
  font-size: 1.5rem;
  font-weight: 700;
  letter-spacing: 0.04em;
}
.brand-sub {
  margin: 0.2rem 0 0;
  font-size: 0.8rem;
  color: #9aa8bc;
}
.nav {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
  flex: 1;
}
.nav__link {
  color: #c5d0e0;
  text-decoration: none;
  padding: 0.5rem 0.65rem;
  border-radius: 6px;
  font-size: 0.9rem;
}
.nav__link:hover {
  background: rgba(255, 255, 255, 0.06);
}
.nav__link.active {
  background: rgba(66, 153, 225, 0.25);
  color: #fff;
}
.side-foot {
  border-top: 1px solid rgba(255, 255, 255, 0.1);
  padding-top: 0.75rem;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
}
.user {
  margin: 0;
  font-size: 0.8rem;
  color: #9aa8bc;
  overflow: hidden;
  text-overflow: ellipsis;
}
.main {
  display: flex;
  flex-direction: column;
  min-width: 0;
  background: #f3f5f8;
}
.main-top {
  padding: 1rem 1.25rem 0;
}
.main-body {
  padding: 1rem 1.25rem 2rem;
  flex: 1;
}
@media (max-width: 800px) {
  .shell {
    grid-template-columns: 1fr;
  }
  .side {
    flex-direction: row;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.75rem;
  }
  .nav {
    flex-direction: row;
    flex-wrap: wrap;
    flex: 1 1 100%;
  }
  .side-foot {
    border: 0;
    padding: 0;
    margin-left: auto;
  }
}
</style>
