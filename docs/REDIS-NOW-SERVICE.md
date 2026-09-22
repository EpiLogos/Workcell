# Redis NOW material service

**Owner:** Workcell. **Consumer:** AIKit Jev/Redis NOW integration (EpiLogos/ai-kit#388; O:I #65/#220).
**Reference implementation basis:** Redis OSS 8.10 series; Workcell manages material lifecycle only.

Redis is the hot operational body for prepared participant context and the live NOW neighbourhood. It is not the durable owner of Central source identity, BKMR/Wiki knowledge, Factory Return, or Day/NOW semantics.

## Material policy

The built-in profile is deliberately loopback-only. It uses a target-owned Redis process so a one-shot Workcell invocation can re-observe material started earlier. The reference policy requires:

- AOF persistence with `appendfsync everysec`;
- operator-selected RDB snapshots as a second recovery surface;
- finite `maxmemory` (minimum 64 MiB);
- `maxmemory-policy noeviction`, so Redis pressure cannot discard coordination state as though it were a cache;
- explicit TTL on AIKit's rebuildable prepared payloads;
- protected mode and loopback binding;
- an owner-scoped data directory that is backed up before adoption or destructive replacement.

The helper `redis_now_config_policy(...)` renders this policy and `redis_now_service(...)` supplies the target-owned status/readiness/start/stop material description. Neither operation flushes an existing database.

A Redis service reachable beyond loopback is **not** created by this profile. Such a deployment must use the generic Workcell service declaration with an operator-owned authenticated/encrypted network boundary, and the AIKit consumer must support that same transport. Do not expose the reference profile on `0.0.0.0`.

## Existing native configuration path

Workcell already owns the declared-services setting and persists it as `<state-root>/services.json`. Use the normal configuration transport; do not hand-edit a running Workcell's state without first inspecting it.

A local Redis NOW service can be represented by the existing declaration protocol as:

```json
[
  {
    "logical_ref": "service:redis-now/personal-workcell",
    "endpoint": "redis://127.0.0.1:6381",
    "lifetime": "target-owned",
    "acquisition": "ensure-running",
    "status": {
      "program": "redis-cli",
      "args": ["--raw", "-h", "127.0.0.1", "-p", "6381", "PING"]
    },
    "readiness": {
      "program": "redis-cli",
      "args": ["--raw", "-h", "127.0.0.1", "-p", "6381", "PING"],
      "timeout_ms": 5000,
      "interval_ms": 50
    },
    "start": {
      "program": "redis-server",
      "args": ["/ABSOLUTE/WORKCELL-OWNED/redis-now/redis.conf", "--daemonize", "yes"]
    },
    "stop": {
      "program": "redis-cli",
      "args": ["--raw", "-h", "127.0.0.1", "-p", "6381", "SHUTDOWN"]
    },
    "metadata": {
      "target": "redis",
      "target_minimum_series": "8.10",
      "configuration_owner": "workcell",
      "semantic_state_owner": "central+aikit+factory",
      "persistence_policy": "aof+operator-rdb",
      "eviction_policy": "noeviction",
      "binding": "loopback"
    }
  }
]
```

Resolve the absolute paths and port from the receiving Workcell instead of copying the placeholders.

Validate and plan through the owner-native setting before applying:

```sh
workcell --state-root "$WORKCELL_STATE_ROOT" config validate --json \
  --setting workcell.declared-services \
  --value-file redis-now-services.json

workcell --state-root "$WORKCELL_STATE_ROOT" config plan --json \
  --setting workcell.declared-services \
  --value-file redis-now-services.json > redis-now-plan.json

workcell --state-root "$WORKCELL_STATE_ROOT" config apply --json \
  --plan-file redis-now-plan.json \
  --changeset redis-now-adoption
```

The owner validates that the declared programs exist on the receiving machine and writes `services.json` atomically. Existing unrelated declared services must be included in the planned value; this setting represents the whole declared-services list.

## Redis configuration

For a 256 MiB reference allocation the generated policy is equivalent to:

```conf
bind 127.0.0.1
protected-mode yes
port 6381
dir /ABSOLUTE/WORKCELL-OWNED/redis-now/data
appendonly yes
appendfsync everysec
save 900 1
save 300 10
maxmemory 268435456
maxmemory-policy noeviction
stop-writes-on-bgsave-error yes
```

Create the data directory with permissions appropriate to the Redis service account. If the selected service already exists, inspect its Redis version, bind/auth settings, persistence mode, data directory, memory policy and existing keys before adoption. Do not overwrite its configuration or data merely because the logical service is named Redis NOW.

## Status, recovery, backup and cleanup

`workcell discover --json` shows the declared material offer. Preparation/reconciliation through the ordinary Workcell demand path starts the target-owned service only when `ensure-running` is selected and the service is actually required. Later invocations re-observe it through `redis-cli PING`.

For restart recovery, preserve the data directory and AOF/RDB files, start the same declared material service, then let AIKit re-read its versioned prepared/coordination records and reconcile any uncertain reservation or delivery state. A process restart does not mint new NOW, participant, source, Run or Return identity.

Before configuration replacement or owned teardown, take an operator-approved filesystem/Redis backup appropriate to the deployment and confirm durable Central/Factory continuation has retained consequential Return. Cleanup may stop a service Workcell actually started and may remove an owner-dedicated data directory only as a separately authorised filesystem operation. **Never use `FLUSHDB` or `FLUSHALL` as setup, test cleanup, reset, or teardown.** Never delete or rewrite a pre-existing Redis database merely because a test used the same host.

The first #388/#65 joined proof uses a disposable Workcell state root and disposable Redis data directory. Installed-world adoption follows the local integration handoff after the cloud cut is accepted.
