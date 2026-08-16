<script setup lang="ts">
import Sidebar from "./Sidebar.vue";
import TopBar from "./TopBar.vue";
import PlayerBar from "./PlayerBar.vue";
</script>

<template>
  <div class="main-layout">
    <Sidebar />
    <TopBar />
    <main class="main-content">
      <RouterView />
    </main>
    <PlayerBar />
  </div>
</template>
