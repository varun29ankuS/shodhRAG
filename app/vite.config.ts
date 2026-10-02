import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import path from "path";
import fs from "fs";

/**
 * Serve pdf.js runtime data (CMaps for CJK text, standard fonts for PDFs that
 * do not embed them, wasm image decoders, ICC profiles) from the installed
 * pdfjs-dist package under `/pdfjs/<dir>/`, so the document viewer works fully
 * offline and always matches the installed pdf.js version. The dev server
 * serves the files directly; production builds emit them as assets.
 */
const PDFJS_ASSET_DIRS = ["cmaps", "standard_fonts", "wasm", "iccs"];
const PDFJS_MIME: Record<string, string> = {
  ".bcmap": "application/octet-stream",
  ".pfb": "application/octet-stream",
  ".ttf": "font/ttf",
  ".wasm": "application/wasm",
  ".js": "text/javascript",
  ".icc": "application/vnd.iccprofile",
};

function pdfjsAssets(): Plugin {
  const packageRoot = path.resolve(__dirname, "node_modules/pdfjs-dist");
  const assetFiles = (dir: string) =>
    fs
      .readdirSync(path.join(packageRoot, dir), { withFileTypes: true })
      .filter((entry) => entry.isFile() && !entry.name.startsWith("LICENSE"))
      .map((entry) => entry.name);

  return {
    name: "shodh-pdfjs-assets",
    configureServer(server) {
      server.middlewares.use("/pdfjs", (req, res, next) => {
        const pathname = decodeURIComponent((req.url ?? "").split("?")[0]);
        const parts = pathname.split("/").filter(Boolean);
        if (parts.length !== 2 || !PDFJS_ASSET_DIRS.includes(parts[0]) || parts[1].includes("..")) {
          next();
          return;
        }
        const file = path.join(packageRoot, parts[0], parts[1]);
        fs.readFile(file, (error, data) => {
          if (error) {
            next();
            return;
          }
          res.setHeader("Content-Type", PDFJS_MIME[path.extname(file)] ?? "application/octet-stream");
          res.end(data);
        });
      });
    },
    generateBundle() {
      for (const dir of PDFJS_ASSET_DIRS) {
        for (const name of assetFiles(dir)) {
          this.emitFile({
            type: "asset",
            fileName: `pdfjs/${dir}/${name}`,
            source: fs.readFileSync(path.join(packageRoot, dir, name)),
          });
        }
      }
    },
  };
}

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), pdfjsAssets()],

  // Explicitly set root directory
  root: __dirname,

  // Path aliases for cleaner imports
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
      "@/components": path.resolve(__dirname, "./src/components"),
      "@/lib": path.resolve(__dirname, "./src/lib"),
      "@/hooks": path.resolve(__dirname, "./src/hooks"),
      "@/utils": path.resolve(__dirname, "./src/utils"),
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 5173,  // Changed from 1420 to avoid conflict with Claude Code
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 5174,  // Changed HMR port too
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
  // Single entry point
  build: {
    rollupOptions: {
      input: 'index.html'
    }
  }
}));