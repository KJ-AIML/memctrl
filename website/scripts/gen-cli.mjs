// Generates the CLI reference (src/content/docs/reference/cli/) from `memctrl --help`, so the
// documented flags are the binary's own. Usage: node scripts/gen-cli.mjs /path/to/memctrl
import { execFileSync } from 'node:child_process';
import { mkdirSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const bin = process.argv[2] ?? 'memctrl';
const out = new URL('../src/content/docs/reference/cli/', import.meta.url).pathname;
const GLOBAL = new Set(['--db', '--rc', '-h, --help', '-V, --version']);

// Options whose help text is empty in the binary: what they mean, per command.
const FILL = {
  'add --layer': '`project` (no default expiry), `user` (90 days) or `session` (7 days)',
  'add --confidence': 'From 0 to 1; weighs the memory in the brief and in search',
  'add --claim-type': '`assertion`, `observation`, `decision`, `hypothesis` or `derived_lesson`',
  'add --lifecycle-state': '`accepted`, `candidate`, `superseded`, `rejected` or `archived`',
  'add --verification-state': '`unverified`, `supported`, `disputed` or `refuted`',
  'add --tags': 'A tag (repeatable). Tags starting with `guard:` or `import:` are reserved',
  'add --expires-at': 'RFC 3339 time after which the memory stops being retrieved',
  'semantic tune --file': 'An eval suite to tune on instead of the built-in one',
  'semantic tune --top-k': 'How many results count as a hit',
  'add <CONTENT>': 'The memory, one fact per memory',
  'query <QUERY_TEXT>': 'What to look for: names, paths, commands and error text work best',
  'query --layer': 'Only this layer',
  'query --limit': 'At most this many results',
  'list --layer': 'Only this layer',
  'list --limit': 'At most this many memories, newest first',
  'candidates --layer': 'Only this layer',
  'candidates --limit': 'At most this many candidates, newest first',
  'recall --limit': 'At most this many memories added to the request',
  'stale --layer': 'Only this layer',
  'tree --layer': 'Only this layer',
  'semantic --layer': 'Only this layer',
  'semantic index --layer': 'Only embed memories in this layer',
  'eval --top-k': 'How many results count as a hit',
  'clear --layer': 'Only delete this layer (default: every layer)',
  'clear --yes': 'Required: confirms that memories are deleted',
  'show <MEMORY_ID>': 'The memory id, or the first 8 characters the brief shows',
  'history <MEMORY_ID>': 'The memory id, or the first 8 characters the brief shows',
  'accept <MEMORY_ID>': 'The memory id, or its first 8 characters',
  'pin <MEMORY_ID>': 'The memory id, or its first 8 characters',
  'unpin <MEMORY_ID>': 'The memory id, or its first 8 characters',
  'reject <MEMORY_ID>': 'The memory id, or its first 8 characters',
  'reject --reason': 'Why, kept with the rejection',
  'dispute <MEMORY_ID>': 'The memory id, or its first 8 characters',
  'refute <MEMORY_ID>': 'The memory id, or its first 8 characters',
  'refute --reason': 'Why the memory is wrong',
  'refute --refuting-id': 'A memory that shows it is wrong, linked as evidence',
  'forget <MEMORY_ID>': 'The memory id, or its first 8 characters',
  'reinforce <MEMORY_ID>': 'The memory id, or its first 8 characters',
  'refresh <MEMORY_ID>': 'The memory id, or its first 8 characters',
  'cite <MEMORY_ID>': 'The memory id, or its first 8 characters',
  'cite <PATH[:LINE[-LINE]]>...': 'Code the memory describes: a file, a line or a line range, relative to the project root',
  'supersede <OLD_ID>': 'The memory being replaced',
  'supersede <NEW_ID>': 'The memory that replaces it',
  'import <FILE>': 'A memctrl export, usually `.memctrl/memory.md`',
  'import --dry-run': 'Report what would change without changing anything',
  'init --force': 'Start a separate project here even inside another memctrl project, or replace a malformed `.memoryrc`',
  'install --tool': 'Required: `claude`, `codex`, `cursor`, `axga`, `pi` or `kimi`',
  'install --project': 'Install into this project instead of your home folder',
  'decay --dry-run': 'Only list low-confidence memories; change nothing',
  'decay --threshold': 'List memories whose confidence is below this',
  'decay --min-hours': 'Skip decay if it ran less than this many hours ago; scheduled runs apply the elapsed time',
  'doctor --low-confidence-threshold': 'Report memories whose confidence is below this',
  'doctor --json': 'Accepted for compatibility; the report is always JSON',
  'audit --limit': 'At most this many records',
  'timeline --limit': 'At most this many events',
  'provenance [QUERY]': 'Only records for exactly this query',
  'provenance --full': 'Accepted for compatibility; records are always complete',
  'provenance --limit': 'At most this many records',
  'spans --name': 'Only spans whose operation name contains this',
  'spans --demo': 'Record one demo span first',
  'spans --limit': 'At most this many spans',
  'otel-export --output': 'File to write',
  'otel-export --otlp': 'Write OTLP JSON instead of memctrl spans',
  'otel-export --limit': 'At most this many spans',
  'reflect --force': 'Reflect now, even when the maintenance heuristics would wait',
  'trigger <EVENT>': 'The trigger name from `[triggers]` in `.memoryrc`, such as `on_commit`',
  'trigger --context': 'A JSON object passed to the trigger',
  'review heli --workspace': 'The Heli workspace to read',
  'review heli --completed': 'How many recently completed tasks to read',
  'review heli --persist': 'Save the lessons as candidates (default: only report them)',
  'review heli --dry-run': 'Report only (the default)',
  'review heli --repo': 'Only tasks for this repository',
  'agent put <THREAD_ID>': 'The thread to save state for',
  'agent put <STATE>': 'A JSON object',
  'agent get <THREAD_ID>': 'The thread to load',
  'agent list --limit': 'At most this many checkpoints',
  'agent node <STATE>': 'A JSON object of agent state',
  'agent node --persist': 'Save the extracted memories',
};

// The overview groups commands by what you use them for.
const GROUPS = [
  ['Set up', ['init', 'install', 'serve']],
  ['Everyday use', ['add', 'query', 'context', 'recall', 'list', 'show']],
  ['Review and correct', ['candidates', 'accept', 'pin', 'unpin', 'reject', 'dispute', 'refute', 'supersede', 'forget', 'clear', 'history']],
  ['Code citations', ['cite', 'stale', 'refresh']],
  ['Team sharing', ['export', 'sync', 'import', 'check']],
  ['Search by meaning', ['semantic']],
  ['Measure and maintain', ['eval', 'doctor', 'stats', 'maintenance', 'decay', 'reinforce']],
  ['Structure and automation', ['tree', 'trigger', 'audit', 'reflect', 'done', 'review', 'agent']],
  ['Observability', ['provenance', 'heatmap', 'timeline', 'spans', 'otel-export', 'otel-stats']],
];

const help = (args) => execFileSync(bin, [...args, '--help'], { encoding: 'utf8' });

/** Splits clap help into its about line, usage, and sections of items. */
function parse(text) {
  const lines = text.replace(/\r/g, '').split('\n');
  const about = [];
  let i = 0;
  while (i < lines.length && !lines[i].startsWith('Usage:')) {
    if (lines[i].trim()) about.push(lines[i].trim());
    i++;
  }
  const usage = (lines[i] ?? '').replace(/^Usage:\s*/, '');
  const sections = {};
  let current = null;
  let item = null;
  for (const line of lines.slice(i + 1)) {
    const header = line.match(/^([A-Z][A-Za-z ]+):\s*$/);
    if (header) {
      current = sections[header[1]] = [];
      item = null;
      continue;
    }
    if (!current || !line.trim()) continue;
    const indent = line.match(/^ */)[0].length;
    if (indent <= 6) {
      const [name, ...rest] = line.trim().split(/\s{2,}/);
      item = { name, text: rest.join(' ') };
      current.push(item);
    } else if (item) {
      item.text = `${item.text} ${line.trim()}`.trim();
    }
  }
  return { about: about.join(' '), usage, sections };
}

// URLs in help text are examples (often localhost), so show them as code, not links.
const escape = (s) =>
  s
    .replace(/\|/g, '\\|')
    .replace(/(?<!`)(https?:\/\/[^\s,;)]+)/g, '`$1`')
    .replace(/<(?![^`]*`)/g, '&lt;')
    .replace(/>(?![^`]*`)/g, '&gt;');
const code = (s) => '`' + s.replace(/`/g, '') + '`';

/** One options table: name, what it does, default and environment variable. */
function table(command, items) {
  const rows = items
    .filter((it) => !GLOBAL.has(it.name.replace(/ <.*$/, '')) && !GLOBAL.has(it.name))
    .map((it) => {
      let text = it.text;
      const def = text.match(/\[default: ([^\]]*)\]/)?.[1];
      const env = text.match(/\[env: ([A-Z_]+)=?[^\]]*\]/)?.[1];
      const values = text.match(/\[possible values: ([^\]]*)\]/)?.[1];
      text = text.replace(/\s*\[(default|env|possible values|aliases): [^\]]*\]/g, '').trim();
      const flag = it.name.replace(/^-\w, /, '').split(' ')[0];
      if (!text) text = FILL[`${command} ${flag}`] ?? '';
      if (values) text = `${text}${text ? ' ' : ''}One of: ${values.split(', ').map(code).join(', ')}.`;
      return `| ${code(it.name)} | ${escape(text) || '—'} | ${def !== undefined ? code(def || '""') : ''} | ${env ? code(env) : ''} |`;
    });
  if (!rows.length) return '';
  return ['| Option | Description | Default | Environment |', '| --- | --- | --- | --- |', ...rows].join('\n');
}

function args(command, items) {
  return ['| Argument | Description |', '| --- | --- |', ...items.map((it) => `| ${code(it.name)} | ${escape(it.text || FILL[`${command} ${it.name}`] || '') || '—'} |`)].join('\n');
}

function section(path, depth) {
  const parsed = parse(help(path));
  const command = path.join(' ');
  const h = '#'.repeat(depth);
  const parts = [];
  if (depth > 2) parts.push(`${h} ${code('memctrl ' + command)}`, '', escape(parsed.about), '');
  parts.push('```text', parsed.usage, '```', '');
  if (parsed.sections.Arguments?.length) parts.push(args(command, parsed.sections.Arguments), '');
  const opts = table(command, parsed.sections.Options ?? []);
  if (opts) parts.push(opts, '');
  const subs = (parsed.sections.Commands ?? []).filter((c) => c.name !== 'help');
  if (subs.length) {
    parts.push('## Subcommands', '');
    parts.push('| Subcommand | What it does |', '| --- | --- |', ...subs.map((s) => `| [${code(s.name)}](#memctrl-${[...path, s.name].join('-')}) | ${escape(s.text)} |`), '');
    for (const s of subs) parts.push(section([...path, s.name], 3).body);
  }
  return { about: parsed.about, body: parts.join('\n') };
}

const top = parse(help([]));
const commands = (top.sections.Commands ?? []).map((c) => c.name).filter((c) => c !== 'help');
const grouped = new Set(GROUPS.flatMap(([, list]) => list));
const missing = commands.filter((c) => !grouped.has(c));
if (missing.length) throw new Error(`commands with no group: ${missing.join(', ')}`);

mkdirSync(out, { recursive: true });
for (const file of readdirSync(out)) if (file.endsWith('.md')) rmSync(join(out, file));
const version = execFileSync(bin, ['--version'], { encoding: 'utf8' }).trim();
const abouts = {};
for (const command of commands) {
  const { about, body } = section([command], 2);
  abouts[command] = about;
  const title = `memctrl ${command}`;
  writeFileSync(
    join(out, `${command}.md`),
    [
      '---',
      `title: ${JSON.stringify(title)}`,
      `description: ${JSON.stringify(about)}`,
      'sidebar:',
      `  label: ${JSON.stringify(command)}`,
      'editUrl: false',
      '---',
      '',
      `${escape(about)}`,
      '',
      body,
      `Every command also takes the [global options](/memctrl/reference/cli/#global-options) \`--db\` and \`--rc\`.`,
      '',
      `<sub>Generated from \`${version}\` by \`website/scripts/gen-cli.mjs\`.</sub>`,
      '',
    ].join('\n'),
  );
}
const groups = GROUPS.map(([name, list]) =>
  [`## ${name}`, '', '| Command | What it does |', '| --- | --- |', ...list.map((c) => `| [${code('memctrl ' + c)}](/memctrl/reference/cli/${c}/) | ${escape(abouts[c])} |`), ''].join('\n'),
).join('\n');
writeFileSync(
  join(out, 'index.md'),
  [
    '---',
    'title: CLI reference',
    `description: ${JSON.stringify(`Every memctrl command and option, generated from ${version}.`)}`,
    'sidebar:',
    '  label: Overview',
    '  order: 0',
    'editUrl: false',
    '---',
    '',
    `memctrl is one binary. Every command prints JSON on stdout, except the Markdown session brief and \`--format\` variants, and reports errors on stderr with a nonzero exit status. Run \`memctrl <command> --help\` for the same information in your terminal.`,
    '',
    groups,
    '## Global options',
    '',
    'Every command takes these two options. Without them, memctrl walks up from the current folder to the repository root to find `.memoryrc`, and uses `.memctrl/memories.db` in the project folder.',
    '',
    '| Option | Description | Environment |',
    '| --- | --- | --- |',
    '| `--db <DB>` | SQLite database (otherwise `db_path` in `.memoryrc`, else `.memctrl/memories.db` in the project folder) | `MEMCTRL_DB_PATH` |',
    '| `--rc <RC>` | TOML rules file (default: the nearest `.memoryrc` in this folder or a parent, up to the repository root) | |',
    '| `-h, --help` | Print help | |',
    '| `-V, --version` | Print version | |',
    '',
    `<sub>Generated from \`${version}\` by \`website/scripts/gen-cli.mjs\`.</sub>`,
    '',
  ].join('\n'),
);
console.log(`wrote ${commands.length} command pages and an overview to ${out}`);
