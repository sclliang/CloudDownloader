<script setup lang="ts">
import { computed, ref } from "vue";
import { Sparkles } from "@lucide/vue";

const resonators = [
  {
    id: "shorekeeper",
    name: "守岸人",
    english: "THE SHOREKEEPER",
    region: "黑海岸",
    ability: "叙响织构",
  },
  {
    id: "changli",
    name: "长离",
    english: "CHANGLI",
    region: "今州",
    ability: "丹煌离火",
  },
] as const;
const preferenceKey = "clouddownloader.resonator";
function savedResonator() {
  try {
    return (
      resonators.find((r) => r.id === localStorage.getItem(preferenceKey)) ||
      resonators[0]
    );
  } catch {
    return resonators[0];
  }
}
const current = ref<(typeof resonators)[number]>(savedResonator());
const imageFailed = ref(false);
const portrait = computed(
  () => `/themes/wuthering-waves/${current.value.id}.png`,
);
function select(resonator: (typeof resonators)[number]) {
  current.value = resonator;
  imageFailed.value = false;
  try {
    localStorage.setItem(preferenceKey, resonator.id);
  } catch {
    // The theme remains usable when local preferences cannot be written.
  }
}
</script>

<template>
  <aside
    class="resonator-stage"
    :data-resonator="current.id"
    aria-label="鸣潮共鸣者主题"
  >
    <div class="resonator-heading">
      <Sparkles :size="15" /><span>鸣潮</span><small>WUTHERING WAVES</small>
    </div>
    <div class="resonator-orbit" aria-hidden="true"></div>
    <div class="resonator-watermark" aria-hidden="true">共鸣</div>
    <img
      v-if="!imageFailed"
      :key="current.id"
      class="resonator-portrait"
      :src="portrait"
      :alt="`${current.name}共鸣者立绘`"
      draggable="false"
      @error="imageFailed = true"
    />
    <div v-else class="resonator-fallback">
      <Sparkles :size="48" /><span>立绘暂时无法显示</span>
    </div>
    <div class="resonator-caption" aria-live="polite">
      <span class="resonator-region"
        >{{ current.region }} <i></i> {{ current.ability }}</span
      >
      <h2>{{ current.name }}</h2>
      <p>{{ current.english }}</p>
    </div>
    <div class="resonator-controls" role="group" aria-label="切换共鸣者立绘">
      <span>共鸣者</span>
      <button
        v-for="resonator in resonators"
        :key="resonator.id"
        :aria-pressed="current.id === resonator.id"
        :aria-label="`切换${resonator.name}立绘`"
        @click="select(resonator)"
      >
        {{ resonator.name }}
      </button>
    </div>
  </aside>
</template>
