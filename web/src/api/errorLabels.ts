/** 常见 API 错误码 → 中文说明（索引见 docs/api/错误码表.md） */
export const ERROR_LABELS: Record<string, string> = {
  'admin.not_bootstrapped': '管理员尚未初始化',
  'admin.already_bootstrapped': '系统已初始化，不能重复 bootstrap',
  'admin.weak_password': '密码不符合安全策略',
  'admin.invalid_username': '用户名无效',
  'admin.must_change_password': '请先修改初始密码',
  'admin.invalid_schedule': '调度设置无效',
  'collector.qr_expired': '二维码已过期，请重新发起登录',
  'collector.qr_not_found': '二维码会话不存在或已失效',
  'collector.qr_failed': '扫码登录失败',
  'collector.needs_relogin': '采集账号需重新扫码登录',
  'collector.advance_not_allowed': '当前连接器不支持手动推进二维码',
  'collector.not_found': '采集登录账号不存在',
  'connector.unsupported': '连接器未启用或未验收',
  'source.initial_validation_failed': '首次验证未通过（成功篇数不足）',
  'source.not_found': '监测公众号不存在',
  'source.validation_expired': '验证已过期，请重新发起',
  'egress.network_unreachable': '出口网络不可达',
  'egress.not_found': '出口配置不存在',
  'egress.inactive': '出口已停用',
  'crawler.blocked_by_protection': '平台防护拦截，需人工处理',
  'ocr.failed': 'OCR 识别失败',
  'health.snapshot_unavailable': '尚无健康快照',
  'health.preflight_failed': '日预检失败',
  'token.scope_denied': 'API Token 权限不足',
  'token.invalid': 'API Token 无效',
  'token.expired': 'API Token 已过期',
  'token.revoked': 'API Token 已撤销',
  'job.not_found': '任务不存在',
  'job.invalid_id': '任务 ID 无效',
  'article.not_found': '文章不存在',
  'daily_plan.build_failed': '日计划生成失败',
}

export function labelForErrorCode(code: string): string | undefined {
  return ERROR_LABELS[code]
}
