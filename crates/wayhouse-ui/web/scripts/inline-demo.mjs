// Folds dist-demo/ into one self-contained dist-demo/wayhouse-demo.html (script, styles and fonts inline).
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const dir = "dist-demo";
let html = readFileSync(join(dir, "demo.html"), "utf8");
const file = (ref) => readFileSync(join(dir, ref.replace(/^\.\//, "")));

html = html.replace(/<link rel="stylesheet"[^>]*href="([^"]+)"[^>]*>/, (_, href) => {
  const css = file(href).toString().replace(/url\(([^)]+\.woff2)\)\s*format\("woff2"\),\s*url\([^)]+\.woff\)\s*format\("woff"\)/g, (_m, u) => {
    const ref = u.replace(/['"]/g, "").replace(/^\.\.?\//, "");
    return `url(data:font/woff2;base64,${file(ref.startsWith("assets") ? ref : `assets/${ref}`).toString("base64")}) format("woff2")`;
  });
  return `<style>${css}</style>`;
});
html = html.replace(/<script type="module"[^>]*src="([^"]+)"[^>]*><\/script>/, (_, src) =>
  `<script type="module">${file(src).toString().replace(/<\/script/gi, "<\\/script")}</script>`);
html = html.replace(/<link rel="modulepreload"[^>]*>/g, "");
writeFileSync(join(dir, "wayhouse-demo.html"), html);
console.log(`wrote ${join(dir, "wayhouse-demo.html")} (${(html.length / 1e6).toFixed(2)} MB)`);
