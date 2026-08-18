<script setup lang="ts">
import type { Component } from "vue";

type ButtonVariant =
  | "default"
  | "secondary"
  | "outline"
  | "ghost"
  | "destructive"
  | "link";
type ButtonSize = "default" | "sm" | "lg" | "icon";

withDefaults(
  defineProps<{
    as?: string | Component;
    variant?: ButtonVariant;
    size?: ButtonSize;
    disabled?: boolean;
    type?: "button" | "submit" | "reset";
  }>(),
  {
    as: "button",
    variant: "default",
    size: "default",
    disabled: false,
    type: "button",
  },
);
</script>

<template>
  <component
    :is="as"
    class="button"
    :class="[`button-${variant}`, `button-${size}`]"
    :type="as === 'button' ? type : undefined"
    :disabled="as === 'button' ? disabled : undefined"
    :aria-disabled="disabled || undefined"
    v-bind="$attrs"
  >
    <slot />
  </component>
</template>

<style scoped>
.button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 0.5rem;
  min-height: 2.5rem;
  padding: 0.5rem 0.75rem;
  color: var(--foreground);
  border: 1px solid transparent;
  border-radius: var(--radius-md);
  cursor: pointer;
  user-select: none;
}

.button:hover:not(:disabled):not([aria-disabled="true"]) {
  background: var(--surface-3);
}

.button:disabled,
.button[aria-disabled="true"] {
  cursor: not-allowed;
  opacity: 0.5;
}

.button-default {
  color: var(--primary-foreground);
  background: var(--primary);
}

.button-default:hover:not(:disabled):not([aria-disabled="true"]) {
  background: var(--primary-hover);
}

.button-secondary {
  color: var(--secondary-foreground);
  background: var(--secondary);
}

.button-outline {
  border-color: var(--border);
}

.button-ghost {
  color: var(--muted-foreground);
}

.button-link {
  color: var(--primary);
  text-decoration: underline;
  text-underline-offset: 0.25rem;
}

.button-destructive {
  color: var(--destructive-foreground);
  background: var(--destructive);
}

.button-sm {
  min-height: 2rem;
  padding: 0.25rem 0.5rem;
}

.button-lg {
  min-height: 3rem;
  padding: 0.75rem 1rem;
}

.button-icon {
  width: 2.5rem;
  padding: 0.5rem;
}
</style>
