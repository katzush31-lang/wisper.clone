import path from "node:path";
import { cloudflareTest, readD1Migrations } from "@cloudflare/vitest-pool-workers";
import { defineConfig } from "vitest/config";

export default defineConfig(async () => {
  const migrations = await readD1Migrations(path.join(import.meta.dirname, "migrations"));
  return {
    plugins: [
      cloudflareTest({
        wrangler: { configPath: "./wrangler.jsonc" },
        miniflare: {
          bindings: {
            TEST_MIGRATIONS: migrations,
            // Throwaway values for tests only.
            MASTER_KEY: "dGVzdC1tYXN0ZXIta2V5LTMyLWJ5dGVzLWxvbmctISE=",
            TOKEN_SECRET: "dGVzdC10b2tlbi1zZWNyZXQtMzItYnl0ZXMtbG9uZy0hISE=",
            GOOGLE_CLIENT_SECRET: "test-google-client-secret",
            GOOGLE_CLIENT_ID: "test-client-id.apps.googleusercontent.com",
          },
        },
      }),
    ],
    test: {
      setupFiles: ["./test/setup.ts"],
      fileParallelism: false,
    },
  };
});
