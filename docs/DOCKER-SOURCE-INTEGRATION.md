# Docker provider source integration

**Ticket:** Workcell #10 / W09 / F.06  
**Factory evidence gate:** `EpiLogos/agent-system-design#24` / SI-009  
**Integration mode:** Docker CLI + Compose plugin adapter behind Workcell Rust provider ports

## Boundary

Docker is a material provider, not a Workcell semantic primitive. `ExecutionDemand` continues to express affordances, logical connectivity, resources, persistence, isolation/trust and runtime modes without container IDs, Compose project names, bridge names, volume IDs or physical ports.

The adapter lives in `epilogos-workcell-docker`. Removing that crate or making the Docker executable unavailable removes its current `OperationalOffer`; it does not change the public semantic-client reference types or the provider-neutral demand schema.

## Inspected upstream seam

The implementation pins the inspected source baseline while probing the actually installed runtime at operation time:

| Component | Inspected pin | Runtime seam |
|---|---:|---|
| Docker Engine | `29.6.2` | `docker version`, `docker container create/start/exec/inspect/restart/stop/rm`, `docker network connect` |
| Docker Compose | `5.1.4` | `docker compose version/up/ps/port/restart/stop/down` |

The Docker CLI negotiates the Engine API with the daemon. Workcell therefore does not hard-code an Engine API version into semantic demand or provider contracts.

Primary upstream references:

- Docker Engine 29 release notes: https://docs.docker.com/engine/release-notes/29/
- Docker CLI `version`: https://docs.docker.com/reference/cli/docker/version/
- Docker CLI `container create`: https://docs.docker.com/reference/cli/docker/container/create/
- Docker CLI `container inspect`: https://docs.docker.com/reference/cli/docker/container/inspect/
- Docker CLI `network connect`: https://docs.docker.com/reference/cli/docker/network/connect/
- Docker Compose command reference: https://docs.docker.com/reference/cli/docker/compose/
- Docker Compose `ps`: https://docs.docker.com/reference/cli/docker/compose/ps/
- Docker Compose `port`: https://docs.docker.com/reference/cli/docker/compose/port/
- Docker Compose releases: https://github.com/docker/compose/releases

The Docker CLI, Moby Engine and Docker Compose upstream repositories are Apache-2.0 licensed. This adapter invokes installed Docker tooling; it does not vendor those codebases.

## Host requirements

The production adapter requires:

1. a `docker` CLI executable visible to the Workcell process;
2. a reachable Docker Engine daemon for `ExecutionProvider`;
3. the Docker Compose plugin for `ProjectRuntimeProvider`;
4. permissions for the Workcell process to perform the requested container/network/runtime operations;
5. images and Compose project inputs resolvable according to the host's ordinary Docker configuration.

Absence is explicit. Discovery returns no Docker-backed offer when the Engine or required Compose plugin cannot be probed. A later prepare operation fails with `WorkcellError::Unavailable` if a previously available Docker installation disappears.

## Execution provider mapping

The Docker execution adapter maps only the provider portion of an `ExecutionMaterialRequest`:

- generic `shell` affordance -> `docker container exec`;
- memory/CPU requirements -> Docker resource flags;
- logical connection names -> adapter-owned physical Docker network names;
- isolation/trust requirements -> declared offer capability;
- lifecycle -> create/start/inspect/restart/stop/remove.

Provider-specific IDs and names are returned only in `ProviderAllocation`, observation and provenance fields. They never become caller-owned semantic subject identity.

Partial prepare is cleaned up: if start or later network attachment fails after container creation, the adapter attempts forced container removal before returning the operation failure.

## Project runtime mapping

A `DockerRuntimeMode` is provider configuration for one provider-neutral runtime mode. It binds that mode to a Compose project directory/files, logical connections and exposure targets.

`ProjectRuntimeProvider` uses a stable provider-local Compose project name and:

- `up -d --remove-orphans` to materialise;
- `ps --all --quiet` plus `ps --status running --quiet` to observe;
- `restart` for deterministic restart;
- `stop` for `SuspendIfSupported`;
- `down --remove-orphans` for release.

For explicitly short-lived persistence (`ephemeral`, `task-or-run`, `candidate`), release also passes `--volumes`. Project/workcell/factory/external persistence leaves volumes intact when the runtime is released. Snapshot semantics are not claimed by this Docker provider.

## Recovery after process restart

Provider-local in-memory records are an optimisation, not identity. Execution and Compose runtime allocations retain the provider handle and lifecycle metadata required to reconstruct an operational record after the Workcell process restarts. A newly instantiated provider can therefore inspect or release already materialised Docker resources from persisted `ProviderAllocation` properties/provenance without allocating a replacement or changing the caller's semantic subject identity.

If the physical resource has disappeared, provider observation reports that absence to the reconciliation layer; it does not silently fabricate a resource under the old material identity.

## Exposure

Application/browser exposure is derived from an already materialised runtime. The semantic request remains an `ExposureRequirement`; provider configuration maps it to a service/container port. `docker compose port` discovers the current published binding at invocation time.

Physical host ports therefore remain material state and may change without changing the logical exposure identity.

## Verification

The ordinary repository gate is:

```bash
./scripts/verify.sh
```

Deterministic tests use an injected command runner and cover provider conformance, logical-to-physical network mapping, shell execution, runtime materialisation, observation, restart, exposure, persistence-aware cleanup, provider disappearance, provider replacement and provider-process restart without requiring Docker on the CI host.

A real-provider smoke is opt-in:

```bash
WORKCELL_DOCKER_LIVE=1 ./scripts/verify.sh
```

Optional image override:

```bash
WORKCELL_DOCKER_LIVE=1 WORKCELL_DOCKER_IMAGE=alpine:3.22 ./scripts/verify.sh
```

The live tests exercise real Engine prepare/observe/shell/restart/release and a real temporary Compose project prepare/observe/restart/release. Factory SI-009 should record the actual host versions and successful smoke evidence before its source-inspection gate is closed.
