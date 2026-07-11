#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");
const sharp = require("sharp");

const root = path.resolve(__dirname, "..");
const logoPath = path.join(root, "assets", "logo.svg");
const svgPath = path.join(root, "assets", "icon.svg");
const pngPath = path.join(root, "assets", "icon.png");
const composerAssets = path.join(root, "assets", "AppIcon.icon", "Assets");
const logo = fs.readFileSync(logoPath, "utf8");
const markPaths = [...logo.matchAll(/<path\b[^>]*\/>/g)].map((match) => match[0]);
if (markPaths.length !== 5) {
  throw new Error(`expected 5 paths in ${logoPath}, found ${markPaths.length}`);
}

fs.mkdirSync(composerAssets, { recursive: true });
const writeLayer = (filename, title, paths) => {
  fs.writeFileSync(
    path.join(composerAssets, filename),
    `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1254 1254" role="img" aria-label="${title}">
  ${paths.join("\n  ")}
</svg>
`,
  );
};
writeLayer("01-disks.svg", "DiskDeck disk stack", markPaths.slice(0, 3));
writeLayer("02-reclaim.svg", "DiskDeck reclaimed-space wedge", [markPaths[3]]);
writeLayer("03-spindle.svg", "DiskDeck spindle", [markPaths[4]]);

const fallbackPaths = markPaths.map((markPath) =>
  markPath.replaceAll("#102A56", "#EAF6FF"),
);
const svg = `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" role="img" aria-labelledby="title desc">
  <title id="title">DiskDeck app icon</title>
  <desc id="desc">The DiskDeck stacked-disk mark on a universal blue fallback tile.</desc>
  <rect x="64" y="64" width="896" height="896" rx="205" fill="#1D5673" stroke="#12384D" stroke-width="10"/>
  <g transform="translate(19 17) scale(.82)">
    ${fallbackPaths.join("\n    ")}
  </g>
</svg>
`;
fs.writeFileSync(svgPath, svg);
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
