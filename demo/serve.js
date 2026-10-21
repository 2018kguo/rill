// Serves the demo at http://localhost:4173 (PORT overrides).
const root = new URL(".", import.meta.url).pathname;
const port = Number(process.env.PORT ?? 4173);
Bun.serve({
  port,
  async fetch(request) {
    const path = new URL(request.url).pathname;
    const file = Bun.file(root + (path === "/" ? "index.html" : path.slice(1)));
    return (await file.exists()) ? new Response(file) : new Response("Not found", { status: 404 });
  },
});
console.log(`rill demo on http://localhost:${port}`);
