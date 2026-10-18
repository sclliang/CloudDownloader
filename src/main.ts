import { createApp } from "vue";
import App from "./App.vue";
import "./style.css";
import "./theme.css";
import "./night.css";
import { applyTheme } from "./theme";
applyTheme();
createApp(App).mount("#app");
