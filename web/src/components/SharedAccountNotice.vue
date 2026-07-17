<script setup lang="ts">
/**
 * 共享账号限制提示：唯一管理员会话无法识别具体操作者。
 */
</script>

<template>
  <aside class="shared-account-notice" role="note">
    <strong>共享账号限制</strong>
    <p>
      本系统仅有唯一管理员账号。所有操作者共用同一会话身份，审计只能记录「系统管理员」及时间/来源，
      <em>无法识别具体是谁</em>在操作。请勿将凭据分发给不信任的人。
    </p>
  </aside>
</template>

<style scoped>
.shared-account-notice {
  border: 1px solid #c9a227;
  background: #fff8e1;
  color: #5c4a00;
  padding: 0.75rem 1rem;
  border-radius: 6px;
  font-size: 0.875rem;
}
.shared-account-notice strong {
  display: block;
  margin-bottom: 0.25rem;
}
.shared-account-notice p {
  margin: 0;
  line-height: 1.45;
}
.shared-account-notice em {
  font-style: normal;
  font-weight: 600;
}
</style>
