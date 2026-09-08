import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const manifest = JSON.parse(fs.readFileSync(path.join(root, 'contracts/cameo-capabilities-v1.json'), 'utf8'));
const escapeMd = (text) => String(text).replaceAll('|', '\\|').replaceAll('\n', ' ');
const rows = [];
for (const [section, fields] of Object.entries(manifest)) {
  if (!fields || typeof fields !== 'object') continue;
  for (const [name, value] of Object.entries(fields)) {
    if (!value || typeof value !== 'object') continue;
    if (!['stable', 'preview', 'planned', 'unsupported'].includes(value.maturity)
      || typeof value.available !== 'boolean' || typeof value.detail !== 'string') {
      throw new Error(`Invalid capability: ${section}.${name}`);
    }
    if (['planned', 'unsupported'].includes(value.maturity) && value.available) {
      throw new Error(`Unavailable maturity advertised as available: ${section}.${name}`);
    }
    rows.push(`| ${section}.${name} | ${value.maturity} | ${value.available ? 'yes' : 'no'} | ${escapeMd(value.detail)} |`);
  }
}
const document = `# Cameo capabilities\n\nGenerated from contracts/cameo-capabilities-v1.json. Run \`node scripts/render-capabilities.mjs\` to update.\n\nThese labels describe implemented software surfaces, not hardware certification or production release approval. See [release readiness](release-readiness.md) and the [canonical productization plan](../PRODUCTIZATION_PLAN.md) for outstanding gates.\n\nThe same manifest is available from \`cameo capabilities\`, daemon capability discovery, the console Capabilities panel, \`site/capabilities.json\`, and \`/etc/cameo/capabilities.json\` in newly built ISOs.\n\n| Capability | Maturity | Available | Scope |\n|---|---|---|---|\n${rows.join('\n')}\n`;
const siteJson = `${JSON.stringify(manifest, null, 2)}\n`;

const docsTarget = path.join(root, 'docs/capabilities.md');
const jsonTarget = path.join(root, 'site/capabilities.json');
const check = process.argv.includes('--check');

if (check) {
  if (!fs.existsSync(docsTarget) || fs.readFileSync(docsTarget, 'utf8') !== document) {
    throw new Error('Capability documentation is stale; run node scripts/render-capabilities.mjs');
  }
  if (!fs.existsSync(jsonTarget) || fs.readFileSync(jsonTarget, 'utf8') !== siteJson) {
    throw new Error('site/capabilities.json is stale; run node scripts/render-capabilities.mjs');
  }
} else {
  fs.writeFileSync(docsTarget, document);
  fs.writeFileSync(jsonTarget, siteJson);
}
console.log(`Capability manifest: ${rows.length} entries verified`);
