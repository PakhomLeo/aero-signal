import { defineStore } from 'pinia'
import { ref } from 'vue'

/** 应用级状态。 */
export const useAppStore = defineStore('app', () => {
  const productName = ref('Signal')
  return { productName }
})
