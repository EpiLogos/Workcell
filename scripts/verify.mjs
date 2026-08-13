import { readFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';

const forbidden = ['docker', 'arrakis', 'microvm', 'kubernetes', 'aws', 'azure', 'gcp'];
const schemaPath = new URL('../contracts/workcell/v1/execution-demand.schema.json', import.meta.url);
const schemaText = await readFile(schemaPath, 'utf8');
JSON.parse(schemaText);
const lower = schemaText.toLowerCase();
for (const term of forbidden) {
  if (lower.includes(term)) {
    console.error(`provider vocabulary leaked into ExecutionDemand schema: ${term}`);
    process.exit(1);
  }
}

for (const path of [
  new URL('../contracts/workcell/v1/workcell-plan.schema.json', import.meta.url),
  new URL('../contracts/workcell/v1/materialised-execution-world.schema.json', import.meta.url)
]) {
  JSON.parse(await readFile(path, 'utf8'));
}

const tests = spawnSync(process.execPath, ['--test'], { stdio: 'inherit' });
if (tests.status !== 0) process.exit(tests.status ?? 1);
console.log('workcell verify: schemas parsed; provider-neutral demand boundary clean; tests passed');
