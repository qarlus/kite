import { defineConfig, devices } from "@playwright/test";

// Chromium ≈ WebView2 (Windows), WebKit ≈ WKWebView (macOS).
export default defineConfig({
  testDir: "tests/e2e",
  fullyParallel: true,
  reporter: process.env.CI ? "github" : "list",
  use: { baseURL: "http://localhost:4173" },
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
    { name: "webkit", use: { ...devices["Desktop Safari"] } },
  ],
  webServer: { command: "pnpm vite preview --port 4173 --strictPort", port: 4173, reuseExistingServer: !process.env.CI },
});
