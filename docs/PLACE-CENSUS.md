# The place census and place request/release

Workcell's harness-instance census sees processes: it reconciles the pid table,
detection receipts and the gateway endpoint into the instance registry. But a
pid is not a room. On a machine where agents actually work, the durable things
are the *places* — tmux sessions and panes, Herdr workspaces and panes — that
outlive any single process and host the work between processes. Until now
Workcell could name every harness process alive and still not know what rooms
existed on the machine, who was running in them, or which ones it had claimed.

The place census is that missing layer: an inventory of persistent process
places and the process generations currently bound to them.

## What the census sees

```bash
workcell places          # JSON by default; --json is accepted and identical
```

The command enumerates the two providers this release knows:

- **tmux** — presence and version come from `tmux -V`; panes come from one
  `tmux list-panes -a` against the default socket with a fixed nine-field,
  tab-separated format (session name, session creation, window index and
  name, pane id, pane pid, current command, dead flag, tty). When no server
  is running, tmux says so on stderr; the census records the provider as
  `present-no-server` with that exact stderr. That is a disclosed degraded
  state, not an error: the provider is installed and the machine genuinely
  has no tmux places right now. Both of tmux's cold wordings are classified
  as this state through the one helper the place-request path also uses —
  `no server running on …` on older releases, and `error connecting to …
  (No such file or directory)` on 3.6+/3.7+ (found live on a rebooted host
  during the commissioned TM02-R re-test, where the census used to report a
  bare `error`). A genuinely unexpected tmux failure is still `error`.
- **herdr** — presence and version from `herdr --version`; workspaces and
  panes from `herdr workspace list` and `herdr pane list`, which emit JSON
  envelopes on herdr 0.8.x. Each pane is read once with `herdr pane
  process-info` for its shell pid. If the CLI is installed but the server
  does not answer, the provider is recorded as `present-server-unreachable`
  with the exact stderr. If the output stops matching the observed JSON
  shape, it is recorded as `present-cli-unparsed` — the census never guesses
  fields a CLI did not prove.

Every pane row is joined against the same `ps` pid table the instance census
uses, so each observation carries the OS process start marker (`ps lstart`)
of the pid it currently hosts. That marker scopes a pid to one process
generation, exactly as in the instance registry: a recycled pid is a new
process, and the census can tell.

Each pane is classified by its process command stem:

- `harness` — the stem matches the known harness alias table (claude, codex,
  zcode, hermes, …), and the record names the harness slug;
- `self` — the stem is a Workcell/Factory binary, i.e. a place this suite
  launched;
- `other` — everything else. This is the point of the census: panes running
  processes Workcell did not launch are seen and reported. They are never
  killed, hidden, or treated as errors. The machine's rooms belong to its
  operator; Workcell only insists on knowing what is in them;
- `unobserved` — the pane has no pid, or its pid is not in the pid table (a
  dead pane, or a provider reporting stale state). The pane stays visible.

Every classified pane also carries `classification_evidence`, naming WHICH
evidence decided:

- `ps-comm` — the pid-table `ps` command stem decided. Every tmux pane, and
  any pane whose provider reading was not consulted or did not match;
- `provider-foreground` — the pid-table comm did not match a harness alias,
  but the herdr provider's `pane process-info` named a foreground process
  whose stem is a known harness. Found live on TM02-R: a herdr pane running
  the `pi` harness classified `other` because its pid-table comm named the
  shell; the provider's own reading now classifies it, and the record says
  so.

The fallback applies only to herdr panes (tmux panes keep `ps-comm`), only
after `ps-comm` missed, and only against the harness alias table — never the
self stems. If neither evidence source names a known harness, the pane stays
`other`. The field is additive in `workcell.place-census/v1` and is `null`
for `unobserved` panes nothing classified.

A pane whose id or session name reappears bound to a different process
generation is a **place reuse finding**: named with both pids, never merged
into a continuity claim. A recycled pane id is a new place, the same way a
recycled pid is a new process.

The census is strictly read-only. No tmux or herdr command on this path can
create, kill, or modify anything — listing never starts a server, and the
provider commands are fixed argv with no interpolation.

## What the census refuses to claim

A place is material evidence and an addressable location. It is never a
caller identity. Nothing in a place census proves who a session "belongs to"
beyond which process generation currently runs in it, and a session name or
pane id proves nothing across generations. Identity claims belong in
persisted context; the census only witnesses what is materially present.

Start markers are host-local scheduling facts. They correlate observations on
one machine; they are not global identity, not a semantic session proof, and
not work continuity.

## Requesting and releasing a place

```bash
workcell place request --provider auto|herdr|tmux --name <slug>
workcell place release --place-ref <ref> --pid <n> --start-marker "<ps lstart>"
```

A request claims a new place and returns a **place grant**
(`workcell.place-grant/v1`): the `place_ref`, provider, session/workspace
name, the observed pane id, and the `(pid, process_start_marker)` pair that
scopes the grant to one process generation.

- The name is validated `[a-z0-9-]{1,64}` before anything runs. The request
  is one bounded effect — a single `tmux new-session -d -s <name>` or a
  single `herdr workspace create --label <name>` — built as argv, never
  through a shell, followed by read-only observation of what was created.
- `auto` tries herdr first when the CLI is present and its server answers,
  falls back to tmux, and otherwise refuses naming what was tried. Every
  refusal is a typed document (`already-exists`, `stale-binding`,
  `no-provider`, `provider-cannot-create`, `place-mismatch`, …) with
  evidence, because error paths must explain.
- A tmux server that is merely **not yet running** is a cold bootstrap, not
  an error: the name-free probe accepts both of tmux's wordings for the
  cold case (`no server running on …` on older releases; `error connecting
  to … (No such file or directory)` on 3.6+ — found live on a rebooted
  host during the commissioned TM02-R re-test), and `new-session -d` then
  starts the server as tmux itself defines cold bootstrap. A genuinely
  unexpected tmux failure still stops the request as `provider-error`.
  Open policy question, recorded deliberately: the herdr leg answers
  `no-provider` when its server is down — request never auto-starts a
  provider service. Whether a place *request* may start a found provider
  (as tmux's own CLI effectively does) is an owner decision, not something
  this release decides silently.
- If a place of that name already exists, the request is **refused** with
  `already-exists` evidence. Adopting an existing place is a separate,
  explicit operation, never a silent side effect of asking for a new one —
  the same separation the instance registry draws between declare and adopt.
- The herdr path only does what the proven CLI surface supports. `workspace
  create`/`close` exist on herdr 0.8.x and are used; if a command's output
  stops yielding the identifiers Workcell needs (a workspace id, a pane pid),
  the refusal is typed `provider-cannot-create` with the raw output as
  evidence. Workcell does not fabricate grants for capabilities it cannot
  observe.

Release proves before it kills. The caller must supply the place ref **and**
the `(pid, start-marker)` from the grant. The live pid table must still show
that exact generation, the named session/workspace must still exist, and the
pid must still be one of that place's pane processes. Only then does
Workcell run `tmux kill-session -t <session>` (or `herdr workspace close
<workspace_id>`). A pid that is gone, or that now carries a different start
marker, is a stale binding: refused with both markers in evidence. A pid that
is alive but no longer a pane process of the named place is a
`place-mismatch`: the place moved on, and it is not ours to kill.

### When the proof cannot be made: `--provider-close`

```bash
workcell place release --place-ref <ref> --pid <n> --start-marker "<ps lstart>" --provider-close
```

The generation contract has a blind spot the commissioned TM02-R re-test
found live: a herdr workspace can outlive its pane processes. The pane's pid
churns, the proof becomes unmakeable, the release correctly refuses — and
the room itself is left standing, with nothing in the contract to end it. The
operator's only recourse was a bare `herdr workspace close` by hand.

`--provider-close` is that recourse, brought inside the contract:

- **It is valid only on a failed generation proof.** The release runs the
  same proof first; the flag changes the outcome only when the proof refused
  as `stale-binding`, `place-gone` or `place-mismatch`. Any other refusal
  stands with or without the flag.
- **A passed proof refuses the flag** (`provider-close-refused`). A live,
  provable generation is released through the normal contract, never closed
  over it. A live pid is never killed outside the generation proof.
- **The provider must still name the place.** A tmux session is verified
  with `tmux has-session` immediately before `tmux kill-session -t
  <session>`; a herdr workspace is verified present in `herdr workspace
  list` immediately before `herdr workspace close <workspace_id>`. A place
  that has vanished is a `place-gone` refusal — there is nothing left to
  close.
- **The close is receipted.** On success the command emits a
  `workcell.place-provider-close/v1` document naming the close as
  provider-native (`"close": "provider-native"`), carrying the failed
  generation proof as its `justification` (the refusal kind, message and
  evidence — granted and live start markers included), and the provider's
  own output as `evidence`, with the close's UTC time.

Without the flag, release behaviour is unchanged, byte for byte.

The write-boundary discipline (`workcell-write-boundary inspect/run`) is a
kernel-enforced Landlock confinement for launched workload processes in the
control plane; it does not apply to a one-shot CLI invoking tmux or herdr
directly. The bound on place request/release is the contract above: validated
names, one fixed argv effect, read-only read-back, and a generation proof
before any destructive step.

## Schema versions and tests

- `workcell.place-census/v1` — the census document (`workcell places`). The
  `classification_evidence` pane field is an additive revision of this
  schema: new field, disclosed here, existing fields untouched.
- `workcell.place-grant/v1` — the place grant and its release proof.
- `workcell.place-provider-close/v1` — the receipt for a
  `--provider-close` release.

Both are versioned constants in `workcell-runtime` and asserted in tests,
like every other schema in this repository. The tmux row parser, the herdr
envelope parsers, classification, place-reuse findings and the release
decision are all covered by fixture tests that do not require a tmux server;
live tmux request/release tests exist behind `TMUX_TEST_LIVE` and are skipped
unless the environment variable is set.

Provider status vocabulary, for callers: `ok`, `absent`,
`present-no-server` (tmux), `present-server-unreachable` (herdr),
`present-cli-unparsed` (either), `error`. Every state is either a completed
enumeration or a named, evidenced degradation — never a silent empty set.
