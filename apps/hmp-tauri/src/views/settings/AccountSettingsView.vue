<script setup lang="ts">
import AppIcon from "../../components/AppIcon.vue";
import Button from "../../components/Button.vue";
import PageHeader from "../../components/PageHeader.vue";
import SettingsNav from "./SettingsNav.vue";
import accountIcon from "../../assets/icons/account-circle-rounded.svg?raw";

/** 账号设置：账号体系未接线，这里只展示本地 mock 资料与只读信息行 */
const infoRows = [
  { label: "昵称", value: "Username" },
  { label: "会员状态", value: "普通会员" },
  { label: "数据来源", value: "本地演示数据" },
];
</script>

<template>
  <div class="account-settings-view">
    <PageHeader title="账号设置" back-to="/settings" />
    <SettingsNav />

    <section class="profile-card">
      <span class="profile-avatar" aria-hidden="true">
        <AppIcon :src="accountIcon" />
      </span>
      <div class="profile-copy">
        <span class="profile-name">Username</span>
        <span class="profile-tier">普通会员</span>
      </div>
      <Button variant="outline" size="sm" disabled title="账号体系尚未接入，退出登录即将上线">
        退出登录
      </Button>
    </section>

    <section class="settings-group" aria-label="账号信息">
      <div v-for="row in infoRows" :key="row.label" class="setting-row">
        <span class="setting-label">{{ row.label }}</span>
        <span class="setting-value">{{ row.value }}</span>
      </div>
    </section>

    <p class="footnote">云端账号同步与登录流将在后续版本接入，当前资料仅存于本机。</p>
  </div>
</template>

<style scoped>
.account-settings-view {
  padding: var(--space-6) var(--space-8) var(--space-10);
  max-width: 56rem;
}

/* —— 资料卡 —— */
.profile-card {
  display: flex;
  align-items: center;
  gap: var(--space-4);
  padding: var(--space-5);
  background: var(--surface-3);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-sm);
}

.profile-avatar {
  display: grid;
  place-items: center;
  width: 3.25rem;
  height: 3.25rem;
  flex: 0 0 auto;
  color: var(--primary);
  background: var(--muted);
  border-radius: var(--radius-full);
}

.profile-avatar .app-icon {
  width: 1.75rem;
  height: 1.75rem;
}

.profile-copy {
  display: grid;
  min-width: 0;
  flex: 1;
}

.profile-name {
  font-size: 1.05rem;
  font-weight: 650;
}

.profile-tier {
  margin-top: 0.2rem;
  justify-self: start;
  padding: 0.05rem 0.5rem;
  font-size: 0.72rem;
  color: var(--muted-foreground);
  background: var(--muted);
  border-radius: var(--radius-full);
}

/* —— 信息行 —— */
.settings-group {
  margin-top: var(--space-4);
  padding: var(--space-1) var(--space-5);
  background: var(--surface-3);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-sm);
}

.setting-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-6);
  padding: var(--space-3) var(--space-2);
}

.setting-row + .setting-row {
  border-top: 1px solid var(--border);
}

.setting-label {
  font-size: 0.92rem;
  font-weight: 550;
}

.setting-value {
  font-size: 0.9rem;
  color: var(--muted-foreground);
  text-align: right;
}

.footnote {
  margin-top: var(--space-4);
  font-size: 0.8rem;
  color: var(--muted-foreground);
}
</style>
