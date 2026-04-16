<script setup lang="ts">
defineProps<{
  title: string;
  value: string | number;
  description: string;
}>();
</script>

<template>
  <article class="metric-card">
    <p class="metric-card__title">{{ title }}</p>
    <p class="metric-card__value">{{ value }}</p>
    <p class="metric-card__description">{{ description }}</p>
  </article>
</template>
