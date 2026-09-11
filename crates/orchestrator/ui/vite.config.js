import { defineConfig } from "vite";
import preact from "@preact/preset-vite";
import { viteSingleFile } from "vite-plugin-singlefile";

// Builds one self-contained dist/index.html, which the orchestrator embeds and serves at /.
export default defineConfig({
  plugins: [preact(), viteSingleFile()],
  server: { proxy: Object.fromEntries(["/tickets", "/workers", "/metrics"].map(p => [p, "http://localhost:8080"])) },
  test: { environment: "jsdom", include: ["src/**/*.test.{js,jsx}"] },
});
