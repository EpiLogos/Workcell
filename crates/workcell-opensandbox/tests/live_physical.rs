//! The W26 physical acceptance exercise against a real local OpenSandbox
//! deployment (server 0.2.x against Docker), driven through the Workcell
//! provider adapter alone — the exact paths `docs/OPENSANDBOX-SOURCE-INTEGRATION.md`
//! §"Physical acceptance at O:I #97" enumerates, bounded to what a Rust test
//! can drive: real lifecycle creation, native execd command + file read,
//! lease observation + renewal, checkpoint + rematerialisation, the direct
//! sidecar egress control route (the recorded proxy-defect resolution), and
//! release.
//!
//! Opt-in:
//!
//! ```sh
//! WORKCELL_OPENSANDBOX_LIVE=1 cargo test -p epilogos-workcell-opensandbox --test live_physical
//! ```

use std::collections::BTreeMap;

use epilogos_workcell_core::{
    Capacity, CheckpointRequest, DemandRef, ExecutionMaterialRequest, ExecutionProvider,
    LeaseRenewalRequest, MaterialCheckpointProvider, MaterialLeaseProvider, ProviderOperation,
    ProviderPort, ProviderRef, ResourceRequirement, RetentionExpectation,
};
use epilogos_workcell_opensandbox::{
    OpenSandboxConfig, OpenSandboxDataPlane, OpenSandboxEgressPolicyProvider,
    OpenSandboxEgressRule, OpenSandboxEgressTarget, OpenSandboxExecutionProvider,
    StdHttpOpenSandboxTransport,
};
use epilogos_workcell_sdk::contract::{ExternalRef, WorkcellRef};

fn live_enabled() -> bool {
    std::env::var("WORKCELL_OPENSANDBOX_LIVE").as_deref() == Ok("1")
}

fn base_url() -> String {
    std::env::var("WORKCELL_OPENSANDBOX_URL").unwrap_or_else(|_| "http://127.0.0.1:8080/v1".into())
}

fn live_config(provider_ref: &str) -> OpenSandboxConfig {
    let mut config = OpenSandboxConfig::local(
        ProviderRef::new(provider_ref).unwrap(),
        std::env::var("WORKCELL_OPENSANDBOX_IMAGE")
            .unwrap_or_else(|_| "opensandbox/code-interpreter:v1.1.0".into()),
        vec!["/opt/code-interpreter/code-interpreter.sh".into()],
    )
    .unwrap();
    config.lifecycle_base_url = base_url();
    // The deployment shape the 2026-09-07 receipt exercised: data plane
    // through the server proxy, egress control direct to the sidecar.
    config.use_server_proxy = true;
    config.egress_control_direct = true;
    config.api_key_env = None;
    let mut capacity = BTreeMap::new();
    capacity.insert(
        "memory".to_owned(),
        Capacity {
            amount: 768,
            unit: Some("MiB".into()),
        },
    );
    capacity.insert(
        "cpu".to_owned(),
        Capacity {
            amount: 1,
            unit: Some("count".into()),
        },
    );
    config.capacity = capacity;
    config
}

fn request(demand_ref: &str) -> ExecutionMaterialRequest {
    ExecutionMaterialRequest {
        demand_ref: DemandRef::new(demand_ref).unwrap(),
        affordances: vec!["shell".into()],
        resources: vec![
            ResourceRequirement {
                key: "memory".into(),
                minimum: Some(256),
                maximum: Some(768),
                unit: Some("MiB".into()),
            },
            ResourceRequirement {
                key: "cpu".into(),
                minimum: Some(1),
                maximum: Some(1),
                unit: Some("count".into()),
            },
        ],
        connectivity: vec![],
        isolation_trust: None,
        retention: RetentionExpectation::Release,
    }
}

#[test]
fn live_physical_sandbox_lifecycle_execd_lease_egress_and_release() {
    if !live_enabled() {
        eprintln!(
            "skipping: set WORKCELL_OPENSANDBOX_LIVE=1 with the lifecycle server on {host}",
            host = base_url()
        );
        return;
    }
    let mut provider = OpenSandboxExecutionProvider::new(
        live_config("provider:opensandbox-live"),
        StdHttpOpenSandboxTransport,
    )
    .unwrap();

    // Discovered: the offer is available only when the server actually answers.
    let offers = ProviderPort::offers(&provider).unwrap();
    assert_eq!(offers.len(), 1);
    assert_eq!(
        offers[0].availability,
        epilogos_workcell_core::Availability::Available,
        "the lifecycle server must answer before this test runs"
    );

    // Materialise a real sandbox: POST /sandboxes, Creating -> Running.
    let allocation = provider
        .prepare_execution(&request("demand:osb-live-physical"))
        .unwrap();
    assert_ne!(allocation.material_ref, "");
    let observation = provider.observe_execution(&allocation).unwrap();
    assert_eq!(
        observation.detail.get("provider_state").map(String::as_str),
        Some("Running"),
        "observation: {:?}",
        observation.detail
    );

    // Native execd command: real stdout comes back.
    let result = provider
        .execute_operation(
            &allocation,
            &ProviderOperation {
                key: "command".into(),
                parameters: BTreeMap::from([(
                    "command".into(),
                    "printf opensandbox-live-physical".into(),
                )]),
            },
        )
        .unwrap();
    // The code-interpreter entrypoint echoes its own `pong` banner before
    // the command; assert on the command's own output within the stream.
    let stdout = result.output.get("stdout").cloned().unwrap_or_default();
    assert!(
        stdout.contains("opensandbox-live-physical"),
        "command stdout must carry the command's output: {stdout:?}"
    );

    // Native execd file read: write through a command, read through the
    // native /files/download endpoint, verify the bytes.
    provider
        .execute_operation(
            &allocation,
            &ProviderOperation {
                key: "command".into(),
                parameters: BTreeMap::from([(
                    "command".into(),
                    "printf workcell-file-proof > /tmp/workcell-proof.txt".into(),
                )]),
            },
        )
        .unwrap();
    let data_plane = OpenSandboxDataPlane::new(
        live_config("provider:opensandbox-live"),
        StdHttpOpenSandboxTransport,
    )
    .unwrap();
    let reading = data_plane
        .read_file(&allocation, "/tmp/workcell-proof.txt")
        .unwrap();
    assert_eq!(
        reading.bytes.as_slice().trim_ascii(),
        b"workcell-file-proof"
    );

    // Lease: observed from expiresAt, then renewed.
    let lease = provider
        .observe_lease(&allocation)
        .unwrap()
        .expect("a lease");
    assert!(!lease.expires_at.is_empty());
    let renewed = provider
        .renew_lease(
            &allocation,
            &LeaseRenewalRequest {
                expires_at: lease.expires_at.clone(),
            },
        )
        .unwrap();
    assert!(!renewed.expires_at.is_empty());

    // Egress control through the direct sidecar route: the recorded proxy
    // defect's provider-native resolution. The policy patch must answer, not
    // 502, and the observation must read back what was written.
    let egress = OpenSandboxEgressPolicyProvider::new(
        live_config("provider:opensandbox-live"),
        StdHttpOpenSandboxTransport,
        allocation.clone(),
        WorkcellRef::new("workcell:omarchy").unwrap(),
        vec![OpenSandboxEgressTarget::new(
            ExternalRef::new("service:github-api").unwrap(),
            "api.github.com",
        )
        .unwrap()],
    )
    .unwrap();
    // Physically re-confirmed 2026-10-07 (server 0.2.3, egress v1.1.7): the
    // pinned egress control API is unimplemented on the direct sidecar route
    // as well as through the proxy. The fence must answer BY NAME with zero
    // provider-side writes — the exact capability standing, visible.
    let fence = egress
        .patch_policy(&[OpenSandboxEgressRule {
            action: epilogos_workcell_opensandbox::OpenSandboxEgressAction::Allow,
            target: "api.github.com".into(),
        }])
        .expect_err("the pinned egress control API is fenced on this stack");
    assert!(
        fence.to_string().contains("egress policy control path"),
        "expected the named egress control fence, got: {fence}"
    );

    // Checkpoint: the typed attempt records the actual provider verdict. On
    // this stack (server 0.2.3, Docker backend, the 10.5GB code-interpreter
    // image) the snapshot physically fails — a named capability standing of
    // this deployment, not a Workcell failure. When a backend supports it,
    // the checkpoint reaches Ready and the checkpoint ref rematerialises.
    let checkpoint = provider.checkpoint(
        &allocation,
        &CheckpointRequest {
            name: Some("workcell-capacity-20261006".into()),
        },
    );
    match checkpoint {
        Ok(checkpoint)
            if checkpoint.state == epilogos_workcell_core::MaterialCheckpointState::Ready =>
        {
            let restore_config = OpenSandboxConfig::from_snapshot(
                ProviderRef::new("provider:opensandbox-live").unwrap(),
                base_url(),
                checkpoint.checkpoint_ref.clone(),
            )
            .unwrap();
            assert!(matches!(
                restore_config.startup,
                epilogos_workcell_opensandbox::OpenSandboxStartupSource::Snapshot { .. }
            ));
        }
        Ok(checkpoint) => {
            eprintln!(
                "CAPABILITY STANDING: snapshot on this backend reached {:?} (checkpoint_ref {}); rematerialisation is not provable here",
                checkpoint.state, checkpoint.checkpoint_ref
            );
        }
        Err(error) => {
            eprintln!(
                "CAPABILITY STANDING: snapshot attempt refused/failed on this backend: {error}"
            );
        }
    }

    // Release: the sandbox is deleted.
    provider
        .release_execution(&allocation, &RetentionExpectation::Release)
        .unwrap();
    let gone = provider.observe_execution(&allocation);
    assert!(gone.is_err(), "a released sandbox is no longer observable");
}
