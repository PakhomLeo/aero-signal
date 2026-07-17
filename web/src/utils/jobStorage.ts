const PREFIX = 'signal.job.'

export function loadStoredJob(key: string): string | null {
  try {
    return sessionStorage.getItem(`${PREFIX}${key}`)
  } catch {
    return null
  }
}

export function saveStoredJob(key: string, jobId: string | null): void {
  try {
    const full = `${PREFIX}${key}`
    if (!jobId) sessionStorage.removeItem(full)
    else sessionStorage.setItem(full, jobId)
  } catch {
    /* ignore */
  }
}

export function usePersistedJob(key: string) {
  return {
    restore(): string | null {
      return loadStoredJob(key)
    },
    persist(jobId: string | null) {
      saveStoredJob(key, jobId)
    },
    clear() {
      saveStoredJob(key, null)
    },
  }
}
