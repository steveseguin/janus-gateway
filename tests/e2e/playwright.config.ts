import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  timeout: 60000,
  retries: 1,
  use: {
    baseURL: "http://localhost:8088",
    headless: true,
    launchOptions: {
      args: [
        "--use-fake-ui-for-media-stream",
        "--use-fake-device-for-media-stream",
        "--allow-file-access-from-files",
        "--autoplay-policy=no-user-gesture-required",
      ],
    },
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
      testIgnore: /meshcast/,
    },
    {
      name: "meshcast",
      use: { ...devices["Desktop Chrome"] },
      testMatch: /meshcast/,
      timeout: 90000,
      retries: 0,
    },
  ],
});
