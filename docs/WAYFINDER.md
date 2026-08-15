# Workcell Wayfinder

GitHub issue #1 is the executable source of the Workcell development graph.

The governing rule is: **do not close a narrow tracer and silently treat it as closure of a wider Workcell responsibility.** Each ticket must prove its public seam, failure modes, provider replacement and architectural non-collapse relevant to that span.

## Foundation line

The first Rust foundation programme established:

1. #2 — F.01 Rust foundation / complete external contract
2. #3 — F.02 ExecutionDemand
3. #4 — F.03 OperationalOffer / planning
4. #5 — F.04 provider port algebra
5. #6 — F.05 WorkspaceProvider
6. #7 — F.09 BindingGraph / MaterialisedExecutionWorld
7. #9 — F.08 ProjectRuntimeProvider / ServiceProvider
8. #17 — expose / collect material operations
9. #10 — F.06 Docker providers
10. #11 — F.11 reconciliation / lifecycle
11. #12 — F.12 deployment profiles / reference Ubuntu Workcell
12. #13 — F.07 optional Arrakis
13. #14 — cross-repository conformance
14. #15 — F.10 Candidate materialisation integration
15. #16 — remote / multi-Workcell placement seam

Deterministic implementation through that line is represented by draft PR #18. Physical Docker and Arrakis acceptance remain independently open under #10 and #13 until actual provider evidence exists.

## Next product frontier — inhabitable Workcells

The next tranche makes the established Workcell semantics directly usable across the current local/server/agent-hosting field without introducing a second architecture:

16. #19 — native CLI and agent-operable Workcell surface
17. #20 — zero-setup collapsed-local Workcell
18. #21 — optional Workcell Control Service and remote control protocol
19. #22 — service bindings and persistent agent-hosting conformance
20. #23 — client SDK, provider SDK and conformance kit
21. #24 — reference Ubuntu remote Workcell and local/server parity
22. #25 — gateway-management interoperability with Hermes and OpenClaw
23. #26 — Workcell Fabric and Tailscale reference conformance
24. #27 — remote Workcell bootstrap portability with exe.dev

The governing relations are:

```text
ordinary machine
  native CLI -> Workcell core -> host/filesystem providers

remote/server machine
  client/SDK -> material connectivity -> Workcell Control Service -> Workcell core -> providers

persistent agent host / gateway-shaped workload
  higher-layer Agent/Harness/Surface semantics
              ↓ provider-neutral material demand
  Workcell processes + services + bindings + storage + fabric + lifecycle
```

The Workcell Control Service is optional locally and normal for a remotely controlled server. It is not an agent gateway and it is not the physical connectivity fabric.

The canonical Fabric plane is now an explicit implementation frontier. `ExecutionDemand.connectivity` already preserves provider-neutral intent; #26 must make cross-placement relationship feasibility and the resulting material fabric binding inspectable rather than treating connectivity as provider-local token matching.

Tailscale is the first rich reference fabric because its current service/addressability/policy/path model strongly exercises Workcell's logical-service-versus-host and relationship-versus-route distinctions. It does not become Workcell vocabulary.

exe.dev is a separate remote-host/bootstrap comparison. Its SSH management API is useful evidence that host acquisition, Workcell remote-control protocol and runtime fabric are three separable concerns. It does not make SSH the Workcell Control protocol or force a universal host-provider abstraction.

A gateway-shaped workload is a conformance pattern over ordinary Workcell requirements. `Gateway`, `Agent`, `Harness`, `AgentSession`, messaging application and conversation are not Workcell semantic primitives.

Hermes/OpenClaw integrations must be source-pinned against their actual current management surfaces and remain removable adapters. They do not define a universal gateway protocol.

## Execution order

#19 and #20 may start from the existing foundation independently of physical Docker/Arrakis/Tailscale evidence.

#21 follows the native application/CLI seam and defines the transport-neutral remote Workcell control contract.

#26's deterministic relationship/planning work may begin from the current `ExecutionDemand.connectivity`, BindingGraph, ServiceProvider and placement seams. Its live Control-Service-over-Tailscale leg waits only for a stable #21 service boundary and actual physical machines.

#22 composes zero-setup/local, Control-Service and logical connectivity into persistent-service/agent-hosting conformance. #23 packages stable client/provider extension seams and conformance machinery.

#24 proves the same demands on the real reference Ubuntu home server and consumes #26's physical fabric receipt for the private workstation↔server case.

#27 then provides a deliberately different remote-host/bootstrap proof through the source-pinned exe.dev SSH API; it should decide from evidence whether host acquisition deserves a public Workcell port or remains deployment tooling.

#25 uses real ecosystem gateways as interoperability evidence while coordinating Agent/Harness/Surface semantics with AIKit rather than importing them.

Source-inspection and physical-host gates block only the integrations that genuinely require them. They do not block ordinary deterministic Workcell development.

See `docs/CONNECTIVITY-FABRIC.md` for the fabric relationship and reference-test laws.
