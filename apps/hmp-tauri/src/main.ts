import { createApp } from "vue";
import App from "./App.vue";
import "./styles/index.css";
import { router } from "./router";
import { initTheme } from "./lib/themeStore.ts";

initTheme();
createApp(App).use(router).mount("#app");
