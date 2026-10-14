/// <reference types="vite/client" />
import { describe, expect, it } from "vitest";
import indexHtml from "../index.html?raw";
import mainTsx from "./main.tsx?raw";

// The server's CSP is `default-src 'self'`, so any font or stylesheet fetched
// from another host is blocked and the UI silently loses its typeface.
describe("fonts are self-hosted", () => {
  it("index.html loads nothing from another origin", () => {
    expect(indexHtml).not.toMatch(/https?:\/\//);
  });

  it("main.tsx imports the bundled Plex faces", () => {
    expect(mainTsx).toContain("@fontsource/ibm-plex-sans");
    expect(mainTsx).toContain("@fontsource/ibm-plex-mono");
  });
});
