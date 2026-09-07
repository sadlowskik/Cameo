import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const manifest = JSON.parse(fs.readFileSync(path.join(root, 'contracts/cameo-capabilities-v1.json'), 'utf8'));
const escapeMd = (text) => String(text).replaceAll('|', '\\|').replaceAll('\n', ' ');
const escapeHtml = (text) => String(text)
  .replaceAll('&', '&amp;')
  .replaceAll('<', '&lt;')
  .replaceAll('>', '&gt;')
  .replaceAll('"', '&quot;');
const rows = [];
const htmlRows = [];
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
    htmlRows.push(`    <tr><td>${escapeHtml(`${section}.${name}`)}</td><td>${escapeHtml(value.maturity)}</td><td class="${value.available ? 'yes' : 'no'}">${value.available ? 'yes' : 'no'}</td><td>${escapeHtml(value.detail)}</td></tr>`);
  }
}
const document = `# Cameo capabilities\n\nGenerated from contracts/cameo-capabilities-v1.json. Run \`node scripts/render-capabilities.mjs\` to update.\n\nThese labels describe implemented software surfaces, not hardware certification or production release approval. See [production audit](production-audit.md) for outstanding gates.\n\nThe same manifest is available from \`cameo capabilities\`, daemon capability discovery, the console Capabilities panel, \`site/capabilities.json\`, and \`/etc/cameo/capabilities.json\` in newly built ISOs.\n\n| Capability | Maturity | Available | Scope |\n|---|---|---|---|\n${rows.join('\n')}\n`;
const siteJson = `${JSON.stringify(manifest, null, 2)}\n`;
const siteTable = `    <table class="caps">
      <thead><tr><th>Capability</th><th>Maturity</th><th>Available</th><th>Scope</th></tr></thead>
      <tbody>
${htmlRows.join('\n')}
      </tbody>
    </table>`;
const begin = '<!-- CAPABILITIES:BEGIN -->';
const end = '<!-- CAPABILITIES:END -->';

function replaceRegion(source, inner) {
  const start = source.indexOf(begin);
  const stop = source.indexOf(end);
  if (start < 0 || stop < 0 || stop < start) {
    throw new Error('site/index.html is missing CAPABILITIES markers');
  }
  return `${source.slice(0, start)}${begin}\n${inner}\n    ${end}${source.slice(stop + end.length)}`;
}

const docsTarget = path.join(root, 'docs/capabilities.md');
const jsonTarget = path.join(root, 'site/capabilities.json');
const siteTarget = path.join(root, 'site/index.html');
const check = process.argv.includes('--check');

if (check) {
  const site = fs.readFileSync(siteTarget, 'utf8');
  const expectedSite = replaceRegion(site, siteTable);
  if (!fs.existsSync(docsTarget) || fs.readFileSync(docsTarget, 'utf8') !== document) {
    throw new Error('Capability documentation is stale; run node scripts/render-capabilities.mjs');
  }
  if (!fs.existsSync(jsonTarget) || fs.readFileSync(jsonTarget, 'utf8') !== siteJson) {
    throw new Error('site/capabilities.json is stale; run node scripts/render-capabilities.mjs');
  }
  if (site !== expectedSite) {
    throw new Error('site/index.html capability table is stale; run node scripts/render-capabilities.mjs');
  }
} else {
  fs.writeFileSync(docsTarget, document);
  fs.writeFileSync(jsonTarget, siteJson);
  fs.writeFileSync(siteTarget, replaceRegion(fs.readFileSync(siteTarget, 'utf8'), siteTable));
}
console.log(`Capability manifest: ${rows.length} entries verified`);
