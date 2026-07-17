import { createRouter, createWebHistory } from 'vue-router'
import { useAuthStore } from '../stores/auth'

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    {
      path: '/login',
      name: 'login',
      component: () => import('../views/LoginView.vue'),
      meta: { public: true },
    },
    {
      path: '/change-password',
      name: 'change-password',
      component: () => import('../views/ChangePasswordView.vue'),
      meta: { requireAuth: true, allowMustChange: true },
    },
    {
      path: '/',
      component: () => import('../layouts/AdminLayout.vue'),
      meta: { requireAuth: true },
      children: [
        { path: '', redirect: { name: 'overview' } },
        {
          path: 'overview',
          name: 'overview',
          component: () => import('../views/OverviewView.vue'),
        },
        {
          path: 'collector-accounts',
          name: 'collector-accounts',
          component: () => import('../views/CollectorAccountsView.vue'),
        },
        {
          path: 'sources',
          name: 'sources',
          component: () => import('../views/SourcesListView.vue'),
        },
        {
          path: 'sources/add',
          name: 'sources-add',
          component: () => import('../views/SourceAddWizardView.vue'),
        },
        {
          path: 'sources/:id',
          name: 'source-detail',
          component: () => import('../views/SourceDetailView.vue'),
        },
        {
          path: 'articles/:id',
          name: 'article-detail',
          component: () => import('../views/ArticleDetailView.vue'),
        },
        {
          path: 'delivery',
          name: 'delivery',
          component: () => import('../views/DeliveryView.vue'),
        },
        {
          path: 'settings',
          name: 'settings',
          component: () => import('../views/SettingsView.vue'),
        },
        {
          path: 'account',
          name: 'account',
          component: () => import('../views/AccountView.vue'),
        },
        {
          path: 'jobs/:id',
          name: 'job-detail',
          component: () => import('../views/JobView.vue'),
        },
      ],
    },
    { path: '/:pathMatch(.*)*', redirect: { name: 'overview' } },
  ],
})

router.beforeEach(async (to) => {
  const auth = useAuthStore()
  if (!auth.bootstrapped) auth.hydrate()

  if (to.meta.public) {
    if (auth.isAuthenticated && to.name === 'login') {
      return auth.mustChangePassword
        ? { name: 'change-password' }
        : { name: 'overview' }
    }
    return true
  }

  if (!auth.isAuthenticated) {
    return { name: 'login', query: { redirect: to.fullPath } }
  }

  if (auth.mustChangePassword && !to.meta.allowMustChange) {
    return { name: 'change-password' }
  }

  return true
})
