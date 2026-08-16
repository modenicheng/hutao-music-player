<script setup lang="ts">
import MainLayout from "./layouts/MainLayout.vue";
</script>

<template>
  <!-- <MainLayout /> -->
  <div class="header"></div>
  <div class="center">
    <div class="left"></div>
    <div class="main-layout">
      <MainLayout />
    </div>
  </div>
  <div class="float-right"></div>
  <div class="playbar"></div>
</template>

<style>
#app {
  display: flex !important;
  flex-direction: column;
}
</style>

<style scoped>
@import "./styles/index.css";

.header {
  height: 5rem;
  background-color: var(--primary);
}

.center {
  display: flex;
  flex-direction: row;
  flex: 1;
}

.left {
  width: 17%;
  min-width: 14rem;
  max-width: 18rem;
  background-color: var(--walnut-300);
}

.playbar {
  height: 5rem;
  background-color: var(--walnut-200);
}

.main-layout {
  background-color: var(--walnut-100);
  flex: 1;
}
</style>
