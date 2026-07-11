#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");
const sharp = require("sharp");

const root = path.resolve(__dirname, "..");
const svgPath = path.join(root, "assets", "icon.svg");
const pngPath = path.join(root, "assets", "icon.png");
const svg = fs.readFileSync(svgPath, "utf8");
const browserCandidates = [
  process.env.CHROMIUM_EXECUTABLE,
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
  "/Applications/Chromium.app/Contents/MacOS/Chromium",
].filter(Boolean);
const executablePath = browserCandidates.find((candidate) => fs.existsSync(candidate));

(async () => {
  const browser = await chromium.launch({
    headless: true,
    ...(executablePath ? { executablePath } : {}),
  });
  try {
    const page = await browser.newPage({
      viewport: { width: 1024, height: 1024 },
      deviceScaleFactor: 1,
    });
    await page.setContent(
      `<style>html,body{margin:0;width:1024px;height:1024px;background:transparent}svg{display:block;width:1024px;height:1024px}</style>${svg}`,
      { waitUntil: "load" },
    );
    await page.screenshot({
      path: pngPath,
      omitBackground: true,
      animations: "disabled",
    });
  } finally {
    await browser.close();
  }

  await sharp(pngPath).resize(256, 256).png().toFile("/tmp/diskdeck-icon-256.png");
  await sharp(pngPath).resize(96, 96).png().toFile("/tmp/diskdeck-icon-96.png");
  console.log(`rendered ${path.relative(root, pngPath)} plus 256px and 96px QA previews`);
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
