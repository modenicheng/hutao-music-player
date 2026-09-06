<script setup lang="ts">
import AppIcon from "../../components/AppIcon.vue";
import PageHeader from "../../components/PageHeader.vue";
import settingsIcon from "../../assets/icons/settings-rounded.svg?raw";
import volumeIcon from "../../assets/icons/volume-up-rounded.svg?raw";
import personIcon from "../../assets/icons/person-rounded.svg?raw";

/** 设置总览：三张分类卡进入子页；实际设置项都在子页里 */
const categories = [
  {
    title: "常规",
    desc: "主题外观、启动行为与语言偏好",
    icon: settingsIcon,
    to: "/settings/general",
  },
  {
    title: "播放",
    desc: "音质档位、音量与播放行为",
    icon: volumeIcon,
    to: "/settings/playback",
  },
  {
    title: "账号",
    desc: "会员状态与本地资料",
    icon: personIcon,
    to: "/settings/account",
  },
];
</script>

<template>
  <div class="settings-view">
    <PageHeader title="设置">
      <template #meta>
        <span>外观与播放偏好保存在本机</span>
      </template>
    </PageHeader>

    <div class="category-grid">
      <RouterLink
        v-for="category in categories"
        :key="category.to"
        :to="category.to"
        class="category-card"
      >
        <span class="category-icon" aria-hidden="true">
          <AppIcon :src="category.icon" />
        </span>
        <span class="category-copy">
          <span class="category-title">{{ category.title }}</span>
          <span class="category-desc">{{ category.desc }}</span>
        </span>
        <span class="category-arrow" aria-hidden="true"></span>
      </RouterLink>
    </div>
  </div>
</template>

<style scoped>
.settings-view {
  padding: var(--space-6) var(--space-8) var(--space-10);
}

.category-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(13rem, 1fr));
  gap: var(--space-4);
  max-width: 56rem;
}

.category-card {
  display: flex;
  align-items: center;
  gap: var(--space-4);
  padding: var(--space-5);
  background: var(--surface-3);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-sm);
  transition:
    transform var(--duration-normal) var(--ease-standard),
    box-shadow var(--duration-normal) var(--ease-standard);
}

.category-card:hover {
  transform: translateY(-2px);
  box-shadow: var(--shadow-md);
}

.category-icon {
  display: grid;
  place-items: center;
  width: 2.75rem;
  height: 2.75rem;
  flex: 0 0 auto;
  color: var(--foreground);
  background: var(--muted);
  border-radius: var(--radius-md);
}

.category-icon .app-icon {
  width: 1.35rem;
  height: 1.35rem;
}

.category-copy {
  display: grid;
  min-width: 0;
  flex: 1;
}

.category-title {
  font-size: 1rem;
  font-weight: 650;
}

.category-desc {
  margin-top: 0.2rem;
  font-size: 0.8rem;
  color: var(--muted-foreground);
}

/* expand-more 旋转 -90° 指向右侧（与 SectionHeader"更多"同款） */
.category-arrow {
  width: 0.9rem;
  height: 0.9rem;
  flex: 0 0 auto;
  background: var(--muted-foreground);
  -webkit-mask: url("../../assets/icons/expand-more-rounded.svg") no-repeat center / contain;
  mask: url("../../assets/icons/expand-more-rounded.svg") no-repeat center / contain;
  transform: rotate(-90deg);
}
</style>
