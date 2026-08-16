<template>
  <footer class="player-bar">
    <div class="progress-bar"></div>
    <div class="status-card"></div>
  </footer>
</template>

<style lang="css" scoped>
.player-bar {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  min-height: var(--player-bar-height);
  color: var(--foreground);
}

.progress-bar {
  width: 100%;
  height: var(--control-height-xs);
  background: var(--track);
  border-radius: var(--radius-full);
}

.status-card {
  width: 100%;
  flex: 1;
  min-height: 0;
  background: var(--surface-2);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
}
</style>
