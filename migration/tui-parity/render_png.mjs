// Render a parity report.html to PNG with the preinstalled Chromium.
// Usage: NODE_PATH=$(npm root -g) node render_png.mjs report.html report.png
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const [, , input, output] = process.argv;
const executablePath = process.env.PLAYWRIGHT_CHROMIUM || undefined;
const browser = await chromium.launch(executablePath ? { executablePath } : {});
const page = await browser.newPage({ viewport: { width: 1800, height: 1000 } });
await page.goto(pathToFileURL(input).href);
await page.screenshot({ path: output, fullPage: true });
await browser.close();
console.log(output);
