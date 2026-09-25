// Native Agency receipt transport. Actuation decides authority; Workcell only
// checks that the native answer is for the exact bytes and selected Agency.
fn admit_run_agency(source: &Path, agency_ref: &str, revision: &str, state_root: &Path) -> Result<Value,WorkcellError> {
    use std::io::Write;
    use std::process::{Command,Stdio};
    let refuse=|message:&str|WorkcellError::InvalidDemand(message.into());
    if revision.trim().is_empty() || !source.is_absolute() || source.canonicalize().ok().as_deref()!=Some(source) {
        return Err(refuse("Agency admission needs an exact canonical source and source revision"));
    }
    let meta=fs::symlink_metadata(source).map_err(|e|WorkcellError::OperationFailed(e.to_string()))?;
    if !meta.is_file() || meta.len()>1_048_576 {return Err(refuse("Agency source must be a regular file no larger than 1 MiB"));}
    let bytes=fs::read(source).map_err(|e|WorkcellError::OperationFailed(e.to_string()))?;
    let request:Value=serde_json::from_slice(&bytes).map_err(|e|WorkcellError::InvalidDemand(e.to_string()))?;
    if request["schema"]!="actuation.agency-actualisation/v1" || request["differentiated_binding"]["agency_ref"]!=agency_ref {
        return Err(refuse("Agency request does not name the selected native Agency"));
    }
    let directory=state_root.join("admission-inputs");fs::create_dir_all(&directory).map_err(|e|WorkcellError::OperationFailed(e.to_string()))?;
    let staged=directory.join(format!("{}-{}.json",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()));
    struct Staged(PathBuf);impl Drop for Staged {fn drop(&mut self){let _=fs::remove_file(&self.0);}}
    let _cleanup=Staged(staged.clone());
    let mut options=fs::OpenOptions::new();options.write(true).create_new(true);
    #[cfg(unix)] {use std::os::unix::fs::OpenOptionsExt;options.mode(0o600);}
    options.open(&staged).and_then(|mut f|f.write_all(&bytes)).map_err(|e|WorkcellError::OperationFailed(e.to_string()))?;
    let program=env::var_os("OI_ACTUATION_BIN").unwrap_or_else(||"actuation".into());
    let mut command=Command::new(program);command.args(["agency","actualise"]).arg(&staged).arg("--json").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let output=epilogos_workcell_runtime::run_bounded_process(command,std::time::Duration::from_secs(15),1_048_576)?;
    if !output.status.success()||output.timed_out||output.output_truncated||!output.output_complete {
        return Err(refuse("Actuation refused or did not complete Agency admission; no run authority was inferred"));
    }
    let receipt:Value=serde_json::from_slice(&output.stdout).map_err(|_|refuse("Actuation returned unreadable Agency admission"))?;
    if receipt["schema"]!="actuation.agency-actualisation/v1"||receipt["status"]!="actualised" {
        return Err(refuse("Actuation did not return an actualised Agency receipt"));
    }
    for key in ["request_ref","requester_ref","governing_binding","differentiated_binding","determination"] {
        if request.get(key).is_none()||request.get(key)!=receipt.get(key) {return Err(refuse("Actuation receipt does not preserve the exact Agency request"));}
    }
    if receipt["bounds_refs"]!=request["determination"]["bounds_refs"]
        ||receipt["metagency"]["grant_ref"]!=request["metagency_grant"]["grant_ref"]
        ||receipt["metagency"]["authority_ref"]!=request["metagency_grant"]["authority_ref"]
        ||receipt["agent_identity"]["agent_ref"]!=request["differentiated_binding"]["agent_ref"]
        ||receipt["provenance"]["source_refs"]!=request["provenance"]["source_refs"]
        ||receipt["effects"]["materialisation"]!="not-performed"||receipt["effects"]["source_mutation"]!="not-performed"
        ||fs::read(source).map_err(|e|WorkcellError::OperationFailed(e.to_string()))?!=bytes {
        return Err(refuse("Agency source or native admission basis changed"));
    }
    let digest=format!("blake3:{}",blake3::hash(&bytes).to_hex());
    Ok(json!({"agency_ref":agency_ref,"agency_rev":revision,"source_ref":source,"source_digest":digest,
        "binding_revision":format!("actuation-receipt/{}",blake3::hash(&output.stdout).to_hex()),"minted_by":null,"admission":receipt}))
}
