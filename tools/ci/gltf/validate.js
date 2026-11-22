// Validates every .glb under the given directories with the Khronos glTF
// validator and fails on any error or warning.
//
//   cd tools/ci/gltf && npm ci --ignore-scripts && npm run check

const fs = require('fs');
const path = require('path');
const validator = require('gltf-validator');

function glbFiles(dir) {
  const out = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...glbFiles(p));
    else if (entry.name.endsWith('.glb')) out.push(p);
  }
  return out.sort();
}

(async () => {
  const files = process.argv.slice(2).flatMap(glbFiles);
  if (files.length === 0) {
    console.error('no .glb files found');
    process.exit(1);
  }
  let bad = 0;
  for (const file of files) {
    const report = await validator.validateBytes(new Uint8Array(fs.readFileSync(file)));
    const { numErrors, numWarnings } = report.issues;
    const status = numErrors + numWarnings === 0 ? 'ok' : 'FAIL';
    console.log(`${status}  ${path.relative(process.cwd(), file)}  errors=${numErrors} warnings=${numWarnings}`);
    for (const m of report.issues.messages) {
      console.log(`      ${m.severity} ${m.code} ${m.pointer || ''} ${m.message}`);
    }
    if (status !== 'ok') bad += 1;
  }
  console.log(`${files.length - bad}/${files.length} glb files valid`);
  process.exit(bad === 0 ? 0 : 1);
})();
