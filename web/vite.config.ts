import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      "/sitebookify.v1.SitebookifyService": {
        target: "http://127.0.0.1:8080",
        changeOrigin: true,
      },
      "/artifacts": {
        target: "http://127.0.0.1:8080",
        changeOrigin: true,
      },
      "/jobs": {
        target: "http://127.0.0.1:8080",
        changeOrigin: true,
      },
      "/healthz": {
        target: "http://127.0.0.1:8080",
        changeOrigin: true,
      },
    },
  },
});
