import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

const proxyTarget = process.env.VITE_RGINX_CONSOLE_PROXY_TARGET ?? "http://127.0.0.1:8080";

export default defineConfig({
  plugins: [vue()],
  server: {
    port: 5173,
    proxy: {
      "/api": proxyTarget,
      "/healthz": proxyTarget,
    },
  },
});
