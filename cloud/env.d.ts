// Makes `env` from "cloudflare:workers" (used by the tests) carry our binding types.
type WisperWorkerEnv = import("./src/types").Env;
type WisperD1Migration = import("cloudflare:test").D1Migration;

declare namespace Cloudflare {
  interface Env extends WisperWorkerEnv {
    TEST_MIGRATIONS: WisperD1Migration[];
  }
}

declare module "*?raw" {
  const source: string;
  export default source;
}
