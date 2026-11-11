import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

// In development the console calls the server through this dev server, so
// the browser sees one origin (09 §9): the session cookie is first-party and
// `Sec-Fetch-Site` is `same-origin`. `changeOrigin` stays false: the server
// checks `Origin` against the `Host` it sees. `CAVEA_SERVER` is the server's
// address, `http://127.0.0.1:8080` (its default `CAVEA_LISTEN`) if unset.
const server = process.env["CAVEA_SERVER"] ?? "http://127.0.0.1:8080";
const proxy = { target: server, changeOrigin: false };

export default defineConfig({
  plugins: [solid(), tailwindcss()],
  server: {
    proxy: {
      "/console/api/": proxy,
      "/v1/": proxy,
    },
  },
});
