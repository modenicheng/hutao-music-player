<template>
  <div class="hover-item">
    <slot />
  </div>
</template>

<style lang="css" scoped>
.hover-item {
  /* 需要定位，z-index 才会生效，从而绘制在高亮块之上 */
  position: relative;
  z-index: 1;
}
</style>
