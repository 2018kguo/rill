// Vite 7 and older: the same app with rill's plugin (Vite 8 needs no config).
import { rill } from "@2018kguo/rill/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({ plugins: [react(), rill()] });
