# Model-serving materialisation conformance

**Owning ticket:** #29 (`W25 — Model-serving materialisation conformance`)  
**SDK boundary:** #23 / `epilogos-workcell-sdk`  
**Status:** deterministic lifecycle implementation plus opt-in physical-provider gates

This programme treats Ollama, llama.cpp and vLLM as pressure on ordinary Workcell material contracts. It does not add model, agent, harness, session or actuation ontology to Workcell.

The caller continues to own model/variant identity. Workcell owns only the material facts required to make execution possible: process/service lifecycle, artifact/storage placement, resource/accelerator capacity, endpoint/reachability, placement, provider health, provenance and reconciliation.

## Source pins

The fixtures in `crates/workcell-runtime/tests/model_serving_conformance.rs` are pinned to the upstream heads inspected when this implementation was made:

| Provider | Upstream revision | Material behaviour used |
|---|---|---|
| Ollama | `d67ad83426633195089509347ffd4fe795120198` | `ollama serve` as the managed service process; `OLLAMA_HOST` as its bind address; CLI/API model acquisition/control remains separate from inference reachability. |
| llama.cpp | `4df29be4f4c3673f428170fda944a5b19f743bb8` | `llama-cli` as a direct one-shot process and `llama-server` as a long-running HTTP service. |
| vLLM | `6b0b850a8b1764a66d7ffbb023c0b0e0bbdb900b` | `vllm serve <provider-model-id>` as a long-running service; accelerator capacity remains an ordinary Workcell resource requirement. |

Provider-native model identifiers, filesystem paths, arguments, bind addresses, process ids and revisions are recorded as provider/material properties or provenance. They never replace caller-owned semantic refs.

## New generic material seam

`ManagedHostServiceProvider` implements the existing public `ServiceProvider` port for an ordinary long-running host process. It is intentionally not model-specific.

A `ManagedHostService` supplies:

- a caller-facing logical service ref;
- a material endpoint;
- executable, arguments, environment and optional working directory;
- provider-native metadata/provenance;
- an optional TCP readiness probe.

The provider:

1. advertises the service only when the executable is materially available;
2. starts the process without shell interpolation;
3. waits for declared TCP readiness before returning a healthy binding;
4. records process id, endpoint, executable and metadata as material facts;
5. distinguishes a live but unreachable process from a healthy reachable service;
6. reports process disappearance as unavailable health;
7. releases by terminating and reaping the process;
8. refuses suspend/snapshot because a generic host process cannot honestly promise those lifecycle operations;
9. kills outstanding managed children when the provider itself disappears.

The same provider is suitable for any ordinary host service and is therefore a reusable Workcell-owned seam exposed by model-serving implementation evidence rather than a provider-brand abstraction.

## Inference access versus control

The service binding proves only that an endpoint exists and is reachable.

It does **not** grant or imply model-control authority.

Provider-native control remains an explicit operation through existing Workcell process execution where the caller is authorised to request it. For example, Ollama model acquisition/unload/inspection can be represented by explicit `ollama pull`, `ollama stop` and `ollama ps` process operations. llama.cpp direct execution is likewise an ordinary `HostProcessExecutionProvider` operation. The endpoint and the control operation are separate material capabilities and may be exposed under different policy.

## Deterministic conformance

Standard repository verification exercises:

- real managed process start;
- TCP readiness and endpoint observation;
- PID/material provenance;
- release and process reaping;
- process disappearance -> unavailable observation;
- replacement/rematerialisation with stable logical identity but changed provider/material identity;
- executable/provider disappearance represented as unavailable rather than fake health;
- llama.cpp direct-process shape alongside server form;
- vLLM resource planning failing without accelerator capacity;
- the same vLLM demand becoming satisfiable when a separate remote execution offer provides the required accelerator capacity;
- upstream provider revisions retained as material provenance.

The vLLM planning fixture deliberately separates the service offer from the accelerator offer. This preserves the existing Workcell grammar for later remote, multi-GPU and distributed placement: the service does not become a scheduler or model ontology.

## Physical gates

The default CI suite never claims that Ollama, llama.cpp models, or vLLM/GPU hardware are present. Physical tests are opt-in and fail normally when explicitly enabled but unavailable.

### Ollama service

```bash
WORKCELL_OLLAMA_LIVE=1 \
WORKCELL_OLLAMA_BIN=ollama \
WORKCELL_OLLAMA_PORT=21434 \
./scripts/verify.sh
```

This starts `ollama serve`, proves readiness/observation, then releases the service. Model acquisition is intentionally not implicit in this gate.

### llama.cpp direct CLI + server

```bash
WORKCELL_LLAMA_CPP_LIVE=1 \
WORKCELL_LLAMA_CPP_CLI=llama-cli \
WORKCELL_LLAMA_CPP_SERVER=llama-server \
WORKCELL_LLAMA_CPP_MODEL=/absolute/path/to/model.gguf \
WORKCELL_LLAMA_CPP_PORT=28080 \
./scripts/verify.sh
```

The test first executes `llama-cli` through the ordinary host-process execution port, then starts the same caller-supplied model file through the managed `llama-server` service form.

### vLLM GPU-backed service

```bash
WORKCELL_VLLM_LIVE=1 \
WORKCELL_VLLM_BIN=vllm \
WORKCELL_VLLM_MODEL=<provider-native-model-id> \
WORKCELL_VLLM_PORT=28000 \
./scripts/verify.sh
```

This gate proves process/service/readiness lifecycle on a real vLLM-capable environment. It does not fabricate accelerator evidence: the environment running it must actually satisfy the model and GPU requirements.

## Still physical / external

The repository can deterministically prove lifecycle shape, loss/degradation, logical/material identity separation, endpoint reachability mechanics and accelerator planning. It cannot truthfully claim, from generic hosted CI alone:

- that a particular model has been acquired into an Ollama installation;
- that a real GGUF model loads successfully on the selected llama.cpp build/hardware;
- that a chosen vLLM model fits and runs on an actual accelerator topology;
- that a remote/reference Workcell and its Fabric path are reachable;
- multi-host or multi-GPU performance/placement behaviour.

Those remain explicit physical gates. Their future evidence extends the same `ExecutionDemand` + provider/service/resource/fabric contracts.

## Deliberately absent abstractions

This implementation introduces none of the following:

- `LocalModelProvider`;
- `ModelServer`;
- `ModelRelation`;
- `Harness`;
- `AgentSession`;
- `SessionSpace`;
- Actuation semantics;
- a provider daemon or dynamic plugin ABI.

`epilogos-workcell-sdk` remains a thin public façade over the existing Workcell contracts. No SDK expansion was necessary for this provider implementation.
