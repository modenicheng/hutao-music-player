<script setup lang="ts">
const tracks = [
  { title: 'Demo Track', artist: 'HMP Prototype' },
  { title: 'Another Track', artist: 'Local Mock Data' },
]
</script>

<template>
  <main>
    <aside>
      <h1>HMP</h1>
      <p>Frontend prototype</p>
    </aside>
    <section>
      <h2>Queue</h2>
      <div v-for="track in tracks" :key="track.title">
        {{ track.title }} - {{ track.artist }}
      </div>
    </section>
  </main>
</template>
