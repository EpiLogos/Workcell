// Native Agency receipt transport. Actuation decides authority; Workcell only
// checks that the native answer is for the exact bytes and selected Agency.
// admission-inputs is bounded caller-owned diagnostic material, never a receipt
// registry, continuation authority, command queue, or permission to resubmit.
mod admission_inputs {
    use std::{fs, io::{self, Write}, path::{Path, PathBuf}};
    use serde_json::{json, Value};
    use epilogos_workcell_runtime::{BoundedProcessFailure, BoundedProcessOutput};
    const REQUEST_LIMIT: usize = 1_048_576;
    #[cfg(unix)]
    const STREAM_LIMIT: usize = 1_048_576;
    #[cfg(unix)]
    const REPORT_LIMIT: usize = 16_384;
    #[cfg(unix)]
    const INVOCATION_LIMIT: usize = 64;
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    pub(super) struct Inputs {
        #[cfg(unix)] directory: PathBuf,
        request: PathBuf,
        #[cfg(unix)] directory_fd: fs::File,
        #[cfg(unix)] identity: (u64,u64),
        #[cfg(unix)] request_digest: String,
        #[cfg(unix)] request_len: usize,
        #[cfg(unix)] request_file: Option<fs::File>,
        #[cfg(unix)] request_bytes: Vec<u8>,
        #[cfg(unix)] created: Vec<(String,bool)>,
    }
    #[derive(Debug)]
    pub(super) struct StagingFailure { cause:io::Error, facts:Value }
    impl From<io::Error> for StagingFailure {
        fn from(cause:io::Error)->Self {Self {cause,facts:json!({"published":[],"creation_facts":"unavailable before invocation admission","command_invoked":false})}}
    }
    impl std::fmt::Display for StagingFailure {
        fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {
            write!(f,"native input staging refused: kind={:?}, raw_os_error={:?}; {}; command not invoked",
                self.cause.kind(),self.cause.raw_os_error(),self.facts)
        }
    }
    impl std::error::Error for StagingFailure {
        fn source(&self)->Option<&(dyn std::error::Error+'static)> {Some(&self.cause)}
    }
    pub(super) struct Retention {
        pub facts: Value,
        pub cause: Option<io::Error>,
    }
    impl Retention {
        pub fn safe_summary(&self) -> String {
            // Only published paths and bounded nonsecret structural facts.
            format!("admission diagnostics {}; retention cause kind={:?}, raw_os_error={:?}; no authority or automatic retry",
                self.facts,self.cause.as_ref().map(io::Error::kind),
                self.cause.as_ref().and_then(io::Error::raw_os_error))
        }
    }
    fn invalid(message: &'static str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData,message) }

    #[cfg(unix)]
    mod private {
        use super::*;
        use std::os::{fd::{AsRawFd,FromRawFd}, unix::fs::{MetadataExt,OpenOptionsExt}};
        use std::ffi::CString;
        pub fn name(value: &str) -> io::Result<CString> {
            CString::new(value).map_err(|_| invalid("invalid owned admission filename"))
        }
        pub fn open_directory(path: &Path) -> io::Result<fs::File> {
            fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY|libc::O_NOFOLLOW|libc::O_CLOEXEC).open(path)
        }
        pub fn child_directory(parent: &fs::File, child: &str, create: bool) -> io::Result<fs::File> {
            let name=name(child)?;
            if create && unsafe {libc::mkdirat(parent.as_raw_fd(),name.as_ptr(),0o700)} != 0 {
                return Err(io::Error::last_os_error());
            }
            let fd=unsafe {libc::openat(parent.as_raw_fd(),name.as_ptr(),libc::O_RDONLY|libc::O_DIRECTORY|libc::O_NOFOLLOW|libc::O_CLOEXEC)};
            if fd<0 {return Err(io::Error::last_os_error());}
            Ok(unsafe {fs::File::from_raw_fd(fd)})
        }
        #[cfg(target_os="linux")]
        fn empty_acl(file: &fs::File, directory: bool) -> io::Result<()> {
            for attribute in if directory {&["system.posix_acl_access","system.posix_acl_default"][..]} else {&["system.posix_acl_access"][..]} {
                let name=name(attribute)?;
                let size=unsafe {libc::fgetxattr(file.as_raw_fd(),name.as_ptr(),std::ptr::null_mut(),0)};
                if size>=0 {return Err(invalid("private admission object has a native POSIX ACL"));}
                let error=io::Error::last_os_error();
                if error.raw_os_error()!=Some(libc::ENODATA) {return Err(error);}
            }
            Ok(())
        }
        #[cfg(target_os="macos")]
        fn empty_acl(file: &fs::File, _directory: bool) -> io::Result<()> {
            extern "C" {
                fn acl_get_fd_np(fd: libc::c_int, kind: libc::c_int) -> *mut libc::c_void;
                fn acl_free(value: *mut libc::c_void) -> libc::c_int;
            }
            // Darwin ACL_TYPE_EXTENDED=0x100; only actual ENOENT proves absence.
            let acl=unsafe {acl_get_fd_np(file.as_raw_fd(),0x100)};
            if acl.is_null() {
                let error=io::Error::last_os_error();
                return if error.raw_os_error()==Some(libc::ENOENT) {Ok(())} else {Err(error)};
            }
            // A present ACL object is refused; never apply Linux's entry
            // return convention to Darwin (0 means an actual entry there).
            if unsafe {acl_free(acl)}!=0 {return Err(io::Error::last_os_error());}
            Err(invalid("private admission object has a native extended ACL"))
        }
        #[cfg(not(any(target_os="linux",target_os="macos")))]
        fn empty_acl(_file: &fs::File,_directory: bool) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::Unsupported,"native private admission ACL inspection unavailable"))
        }
        pub fn privacy(file: &fs::File, directory: bool) -> io::Result<()> {
            let metadata=file.metadata()?;
            let required=if directory {0o700} else {0o600};
            if metadata.uid()!=unsafe {libc::geteuid()} || metadata.mode()&0o7777!=required
                || (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
                return Err(invalid("admission object is not an owned private native object"));
            }
            empty_acl(file,directory)
        }
        pub fn identity(file: &fs::File) -> io::Result<(u64,u64)> {
            let metadata=file.metadata()?; Ok((metadata.dev(),metadata.ino()))
        }
        pub fn named_directory(path: &Path, expected:(u64,u64)) -> io::Result<()> {
            let current=open_directory(path)?;
            if identity(&current)?!=expected {return Err(invalid("admission directory relation changed"));}
            privacy(&current,true)
        }
        pub fn create_file(parent:&fs::File, child:&str) -> io::Result<fs::File> {
            let name=name(child)?;
            let fd=unsafe {libc::openat(parent.as_raw_fd(),name.as_ptr(),libc::O_WRONLY|libc::O_CREAT|libc::O_EXCL|libc::O_NOFOLLOW|libc::O_CLOEXEC,0o600)};
            if fd<0 {return Err(io::Error::last_os_error());}
            let file=unsafe {fs::File::from_raw_fd(fd)};
            Ok(file)
        }
        pub fn read_exact_private(parent:&fs::File,child:&str,expected:&[u8])->io::Result<()> {
            use std::io::Read;
            let child_name=name(child)?;
            let fd=unsafe {libc::openat(parent.as_raw_fd(),child_name.as_ptr(),libc::O_RDONLY|libc::O_NONBLOCK|libc::O_NOFOLLOW|libc::O_CLOEXEC)};
            if fd<0 {return Err(io::Error::last_os_error());}
            let file=unsafe {fs::File::from_raw_fd(fd)};privacy(&file,false)?;
            let mut current=Vec::new();(&file).take((REQUEST_LIMIT+1) as u64).read_to_end(&mut current)?;
            if current!=expected {return Err(invalid("retained admission request bytes changed; no overwrite attempted"));}
            verify_named_file(parent,child,&file)
        }
        pub fn verify_named_file(parent:&fs::File,child:&str,held:&fs::File)->io::Result<()> {
            let child=name(child)?;
            let fd=unsafe {libc::openat(parent.as_raw_fd(),child.as_ptr(),libc::O_RDONLY|libc::O_NONBLOCK|libc::O_NOFOLLOW|libc::O_CLOEXEC)};
            if fd<0 {return Err(io::Error::last_os_error());}
            let current=unsafe {fs::File::from_raw_fd(fd)};
            if identity(&current)?!=identity(held)? {return Err(invalid("admission file relation changed"));}
            privacy(&current,false)
        }
        pub fn publish(parent:&fs::File,stage:&str,final_name:&str,held:&fs::File) -> io::Result<()> {
            verify_named_file(parent,stage,held)?;
            let stage=name(stage)?; let final_name=name(final_name)?;
            if unsafe {libc::linkat(parent.as_raw_fd(),stage.as_ptr(),parent.as_raw_fd(),final_name.as_ptr(),0)}!=0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        pub fn entry_observation(parent:&fs::File,child:&str)->Value {
            let child=match name(child) {Ok(child)=>child,Err(error)=>return json!({"presence_observed":null,"native_kind":format!("{:?}",error.kind())})};
            let mut metadata=std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe {libc::fstatat(parent.as_raw_fd(),child.as_ptr(),metadata.as_mut_ptr(),libc::AT_SYMLINK_NOFOLLOW)}==0 {
                let metadata=unsafe {metadata.assume_init()};
                json!({"presence_observed":true,"byte_len":metadata.st_size})
            } else {
                let error=io::Error::last_os_error();
                json!({"presence_observed":if error.raw_os_error()==Some(libc::ENOENT) {Some(false)} else {None},
                    "native_kind":format!("{:?}",error.kind()),"raw_os_error":error.raw_os_error()})
            }
        }
        pub fn remove(parent:&fs::File,child:&str) -> io::Result<()> {
            let child=name(child)?;
            if unsafe {libc::unlinkat(parent.as_raw_fd(),child.as_ptr(),0)}!=0 {return Err(io::Error::last_os_error());}
            parent.sync_all()
        }
        pub fn lock(root:&fs::File) -> io::Result<fs::File> {
            let name=name(".allocation-lock")?;
            let fd=unsafe {libc::openat(root.as_raw_fd(),name.as_ptr(),libc::O_RDWR|libc::O_CREAT|libc::O_NOFOLLOW|libc::O_CLOEXEC,0o600)};
            if fd<0 {return Err(io::Error::last_os_error());}
            let lock=unsafe {fs::File::from_raw_fd(fd)};
            privacy(&lock,false)?;
            if unsafe {libc::flock(lock.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)}!=0 {return Err(io::Error::last_os_error());}
            Ok(lock)
        }
    }

    impl Inputs {
        pub fn create(state_root:&Path, bytes:&[u8]) -> std::result::Result<Self,StagingFailure> {
            if bytes.len()>REQUEST_LIMIT {return Err(invalid("admission request exceeds 1 MiB").into());}
            #[cfg(unix)] {
                use std::os::{fd::AsRawFd,unix::fs::MetadataExt};
                // Keep the existing caller-selected state-root path contract,
                // including a fresh root or a platform's lexical path alias.
                fs::create_dir_all(state_root)?;
                let physical_state=state_root.canonicalize()?;
                let state_root=physical_state.as_path();
                let state=private::open_directory(state_root)?;
                let root_path=state_root.join("admission-inputs");
                let root=match private::child_directory(&state,"admission-inputs",true) {
                    Ok(root)=>root,
                    Err(error) if error.kind()==io::ErrorKind::AlreadyExists=>private::child_directory(&state,"admission-inputs",false)?,
                    Err(error)=>return Err(error.into()),
                };
                // The previous owner used a public traversal mode. Restrict the
                // same owned relation, never relax ACL or adopt a foreign owner.
                if root.metadata()?.uid()!=unsafe {libc::geteuid()} {return Err(invalid("foreign admission directory owner").into());}
                if unsafe {libc::fchmod(root.as_raw_fd(),0o700)}!=0 {return Err(io::Error::last_os_error().into());}
                private::privacy(&root,true)?;
                let root_identity=private::identity(&root)?;
                let _lock=private::lock(&root)?;
                private::named_directory(&root_path,root_identity)?;
                let mut entries=0usize;
                for entry in fs::read_dir(&root_path)? {
                    if entry?.file_name()!=".allocation-lock" {entries+=1;}
                    if entries>=INVOCATION_LIMIT {return Err(invalid("admission diagnostic capacity reached; explicit owner cleanup required").into());}
                }
                let name=format!("{}-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos(),SEQUENCE.fetch_add(1,std::sync::atomic::Ordering::Relaxed));
                let directory_fd=private::child_directory(&root,&name,true)?;
                private::privacy(&directory_fd,true)?;
                root.sync_all()?; state.sync_all()?;
                let directory=root_path.join(name);
                let identity=private::identity(&directory_fd)?;
                private::named_directory(&root_path,root_identity)?;
                private::named_directory(&directory,identity)?;
                let request=directory.join("request.json");
                let mut inputs=Self {directory,request,directory_fd,identity,
                    request_digest:format!("blake3:{}",blake3::hash(bytes).to_hex()),request_len:bytes.len(),
                    request_file:None,request_bytes:bytes.to_vec(),created:Vec::new()};
                if let Err(cause)=inputs.write_private("request.json",bytes,REQUEST_LIMIT) {
                    return Err(StagingFailure {cause,facts:inputs.retention_facts()});
                }
                Ok(inputs)
            }
            #[cfg(not(unix))] {
                // Preserve existing native staging/capture on Windows. This
                // owner has no established Windows private ACL faculty, so new
                // durable private captured-output evidence is withheld there.
                let directory=state_root.join("admission-inputs"); fs::create_dir_all(&directory)?;
                let request=directory.join(format!("{}-{}-{}.json",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos(),SEQUENCE.fetch_add(1,std::sync::atomic::Ordering::Relaxed)));
                let mut file=fs::OpenOptions::new().write(true).create_new(true).open(&request)?;
                if let Err(cause)=file.write_all(bytes).and_then(|_|file.sync_all()) {
                    return Err(StagingFailure {cause,facts:json!({"request_creation_observed":true,
                        "request_complete_or_durable":false,"private_acl_qualification":false,"command_invoked":false})});
                }
                Ok(Self {request})
            }
        }
        pub fn request(&self)->&Path {&self.request}
        #[cfg(unix)]
        fn write_private(&mut self,name:&str,bytes:&[u8],limit:usize)->io::Result<()> {
            if bytes.len()>limit {return Err(invalid("admission diagnostic byte budget exceeded"));}
            private::named_directory(&self.directory,self.identity)?;
            private::privacy(&self.directory_fd,true)?;
            let stage=format!(".{name}.writing");
            let mut file=private::create_file(&self.directory_fd,&stage)?;
            self.created.push((stage.clone(),false));
            // Keep actual stage creation even if inherited ACL admission fails.
            private::privacy(&file,false)?;
            file.write_all(bytes)?; file.sync_all()?;
            private::privacy(&file,false)?;
            private::named_directory(&self.directory,self.identity)?;
            private::publish(&self.directory_fd,&stage,name,&file)?;
            self.created.push((name.to_owned(),false));
            self.directory_fd.sync_all()?;
            private::remove(&self.directory_fd,&stage)?;
            self.created.retain(|(created,_)|created!=&stage);
            private::verify_named_file(&self.directory_fd,name,&file)?;
            private::named_directory(&self.directory,self.identity)?;
            if let Some((_,durable))=self.created.iter_mut().find(|(created,_)|created==name) {*durable=true;}
            if name=="request.json" {self.request_file=Some(file);}
            Ok(())
        }
        pub fn retain_capture(&mut self,failure:&BoundedProcessFailure)->Retention {
            self.retain(failure.observation(),failure.stdout(),failure.stderr())
        }
        pub fn retain_output(&mut self,output:&BoundedProcessOutput,reason:&str)->Retention {
            self.retain(json!({"schema":"workcell.admission-output-observation/v1",
                "reason":reason,"executed":true,"status_observed":true,"exit_code":output.status.code(),
                "timed_out":output.timed_out,"output_complete":output.output_complete,
                "output_truncated":output.output_truncated,"automatic_retry":false,
                "admission_authority":false}),&output.stdout,&output.stderr)
        }
        fn retain(&mut self,observation:Value,stdout:&[u8],stderr:&[u8])->Retention {
            #[cfg(unix)] {
                let result=(|| {
                    self.write_private("stdout.bin",stdout,STREAM_LIMIT)?;
                    self.write_private("stderr.bin",stderr,STREAM_LIMIT)?;
                    let report=serde_json::to_vec(&json!({"schema":"workcell.admission-capture-diagnostic/v1",
                        "request":{"file":"request.json","byte_len":self.request_len,"digest":self.request_digest},
                        "stdout":{"file":"stdout.bin","byte_len":stdout.len(),"digest":format!("blake3:{}",blake3::hash(stdout).to_hex())},
                        "stderr":{"file":"stderr.bin","byte_len":stderr.len(),"digest":format!("blake3:{}",blake3::hash(stderr).to_hex())},
                        "observation":observation,"admission_authority":false,"automatic_retry":false}))
                        .map_err(io::Error::other)?;
                    self.write_private("capture-failure.json",&report,REPORT_LIMIT)
                })();
                let mut facts=self.retention_facts();
                facts["retention_complete"]=json!(result.is_ok());
                facts["native_capture_observation"]=observation;
                Retention {facts,cause:result.err()}
            }
            #[cfg(not(unix))] {
                let _=(stdout,stderr);
                // Keep the previous platform's actual cleanup behaviour rather
                // than publish unqualified private durable evidence.
                let cleanup=fs::remove_file(&self.request);
                Retention {facts:json!({"native_capture_observation":observation,
                    "private_output_evidence":"withheld: native privacy faculty unavailable",
                    "private_request_evidence":"withheld; original Source remains caller-owned",
                    "legacy_staging_cleanup_confirmed":cleanup.is_ok(),"admission_authority":false}),cause:cleanup.err()}
            }
        }
        #[cfg(unix)]
        fn retention_facts(&self)->Value {
            let named=private::named_directory(&self.directory,self.identity).is_ok();
            let published=self.created.iter().filter(|(_,durable)|*durable).map(|(name,_)|
                json!({"name":name,"path":if named {Some(self.directory.join(name))} else {None},"durable_publication_observed":true})).collect::<Vec<_>>();
            let remaining=self.created.iter().filter(|(_,durable)|!*durable).map(|(name,_)|
                json!({"name":name,"creation_observed":true,"durable_publication_observed":false,
                    "current_entry":private::entry_observation(&self.directory_fd,name)})).collect::<Vec<_>>();
            json!({"published":published,"remaining_created_files":remaining,
                "directory_relation_verified":named,"request_digest":self.request_digest,
                "request_current_identity_verified":named&&self.request_file.as_ref().is_some_and(|file|
                    private::verify_named_file(&self.directory_fd,"request.json",file).is_ok()),
                "request_byte_len":self.request_len,"admission_authority":false,"automatic_retry":false})
        }
        #[cfg(unix)]
        pub fn restore_request_after_cleanup_failure(&mut self,bytes:&[u8])->io::Result<()> {
            #[cfg(unix)] {
                if bytes!=self.request_bytes {return Err(invalid("request restoration differs from original bounded bytes; no overwrite attempted"));}
                private::named_directory(&self.directory,self.identity)?;
                let present=private::entry_observation(&self.directory_fd,"request.json");
                match present["presence_observed"].as_bool() {
                    Some(false)=>self.write_private("request.json",bytes,REQUEST_LIMIT),
                    Some(true)=>private::read_exact_private(&self.directory_fd,"request.json",bytes),
                    None=>Err(invalid("native request presence is uncertain; no replacement attempted")),
                }
            }
        }
        pub fn remove_after_success(&mut self)->io::Result<()> {
            #[cfg(unix)] {
                private::named_directory(&self.directory,self.identity)?;
                let original=self.request_file.as_ref().ok_or_else(||invalid("original published request identity unavailable; cleanup refused"))?;
                private::verify_named_file(&self.directory_fd,"request.json",original)?;
                private::read_exact_private(&self.directory_fd,"request.json",&self.request_bytes)?;
                private::verify_named_file(&self.directory_fd,"request.json",original)?;
                // A hostile same-UID namespace writer after the final check is
                // outside this finite observation; no atomic cross-owner claim.
                private::remove(&self.directory_fd,"request.json")?;
                self.created.retain(|(name,_)|name!="request.json");
                // No output diagnostics were written on accepted admission.
                fs::remove_dir(&self.directory)?;
                private::open_directory(self.directory.parent().unwrap())?.sync_all()
            }
            #[cfg(not(unix))] {fs::remove_file(&self.request)}
        }
    }
    #[cfg(all(test,any(target_os="linux",target_os="macos")))]
    mod tests {
        use super::*;
        use std::{process::{Command,Stdio},time::Duration};
        use std::os::{fd::AsRawFd,unix::fs::{MetadataExt,OpenOptionsExt,PermissionsExt}};
        fn fixture(label:&str)->PathBuf {
            let base=PathBuf::from(std::env::var_os("WORKCELL_TEST_ARTIFACT_ROOT")
                .expect("real admission evidence gate requires an allocated artifact root"));
            assert!(base.is_absolute()&&base.is_dir());
            let base=base.canonicalize().unwrap();
            let root=base.join(format!("admission-{label}-{}-{}",std::process::id(),
                SEQUENCE.fetch_add(1,std::sync::atomic::Ordering::Relaxed)));
            fs::create_dir(&root).unwrap();fs::set_permissions(&root,fs::Permissions::from_mode(0o700)).unwrap();root
        }
        fn native_failure(bytes:usize)->BoundedProcessFailure {
            let mut command=Command::new("python3");
            command.args(["-S","-c",&format!("import os,time;os.write(1,b'o'*{bytes});os.write(2,b'e'*{bytes});time.sleep(20)")])
                .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
            let failure=epilogos_workcell_runtime::capture_bounded_process(command,Duration::from_secs(2),STREAM_LIMIT).unwrap_err();
            assert!(failure.timed_out()&&failure.status().is_some());
            assert_eq!(failure.stdout().len(),bytes);assert_eq!(failure.stderr().len(),bytes);
            failure
        }
        fn isolated(name:&str)->bool {
            if std::env::var("WORKCELL_ADMISSION_ISOLATED").as_deref()==Ok(name) {return true;}
            let mut command=Command::new(std::env::current_exe().unwrap());
            command.args(["--exact",name,"--nocapture","--test-threads=1"])
                .env("WORKCELL_ADMISSION_ISOLATED",name).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
            let output=epilogos_workcell_runtime::run_bounded_process(command,Duration::from_secs(12),65536).unwrap();
            assert!(output.status.success()&&!output.timed_out&&output.output_complete&&!output.output_truncated
                && String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed; 0 ignored;"),
                "real isolated admission gate failed: stdout={} stderr={}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));false
        }
        #[test]
        fn actual_capture_diagnostics_retain_exact_input_bytes_and_private_native_files() {
            let root=fixture("retained-capture");let request=b"exact diagnostic source bytes\x00\xff";
            let mut inputs=Inputs::create(&root,request).unwrap();
            let failure=native_failure(16384);
            let retained=inputs.retain_capture(&failure);
            assert!(retained.cause.is_none());assert_eq!(retained.facts["retention_complete"],true);
            assert_eq!(fs::read(inputs.request()).unwrap(),request);
            // Real native pathname adversity, not a returned-provider fixture.
            // Keep the held regular source while an actual same-byte replacement
            // is published at its name; the native identity must not be adopted.
            let source=root.join("authored-source.json");
            fs::write(&source,b"actual authored source").unwrap();
            let source=source.canonicalize().unwrap();
            let snapshot=super::super::RunAdmissionSource::read(&source).unwrap();
            fs::rename(&source,root.join("retained-authored-source.json")).unwrap();
            fs::write(&source,b"actual authored source").unwrap();
            assert!(snapshot.verify_current(&source).is_err());
            let bounded=super::super::RunAdmissionSource::read(&source).unwrap();
            fs::OpenOptions::new().write(true).open(&source).unwrap()
                .set_len(super::super::RunAdmissionSource::LIMIT+1).unwrap();
            assert!(bounded.verify_current(&source).is_err());
            assert!(super::super::RunAdmissionSource::read(&source).is_err());
            fs::remove_file(&source).unwrap();
            let native_name=private::name(source.to_str().unwrap()).unwrap();
            assert_eq!(unsafe {libc::mkfifo(native_name.as_ptr(),0o600)},0);
            // O_NONBLOCK permits the real owner to refuse this FIFO as nonregular
            // without any writer, rather than wait before the regular-file check.
            assert!(super::super::RunAdmissionSource::read(&source).is_err());
            fs::remove_file(&source).unwrap();
            let fifo_name=private::name("unpublished-fifo").unwrap();
            assert_eq!(unsafe {libc::mkfifoat(inputs.directory_fd.as_raw_fd(),fifo_name.as_ptr(),0o600)},0);
            assert!(private::read_exact_private(&inputs.directory_fd,"unpublished-fifo",b"").is_err());
            let held=private::open_directory(&inputs.directory).unwrap();
            assert!(private::verify_named_file(&inputs.directory_fd,"unpublished-fifo",&held).is_err());
            private::remove(&inputs.directory_fd,"unpublished-fifo").unwrap();
            let report:Value=serde_json::from_slice(&fs::read(inputs.directory.join("capture-failure.json")).unwrap()).unwrap();
            assert_eq!(report["observation"],failure.observation());
            assert_eq!(report["request"]["digest"],format!("blake3:{}",blake3::hash(request).to_hex()));
            assert_eq!(report["stdout"]["digest"],format!("blake3:{}",blake3::hash(failure.stdout()).to_hex()));
            assert_eq!(report["stderr"]["digest"],format!("blake3:{}",blake3::hash(failure.stderr()).to_hex()));
            assert_eq!(report["admission_authority"],false);
            for name in ["request.json","stdout.bin","stderr.bin","capture-failure.json"] {
                let file=fs::File::open(inputs.directory.join(name)).unwrap();
                private::privacy(&file,false).unwrap();assert_eq!(file.metadata().unwrap().mode()&0o7777,0o600);
            }
            assert_eq!(fs::read(inputs.directory.join("stdout.bin")).unwrap(),failure.stdout());
            assert_eq!(fs::read(inputs.directory.join("stderr.bin")).unwrap(),failure.stderr());
            // Fresh caller read is diagnostic only; no Actuation command/Return
            // has been manufactured or retried by this file-owner qualification.
            let fresh=private::open_directory(&inputs.directory).unwrap();private::privacy(&fresh,true).unwrap();
            fs::write(root.join("native-result.json"),serde_json::to_vec(&retained.facts).unwrap()).unwrap();
            // A real same-name replacement must survive cleanup. The original
            // published FD stays held, and no Actuation call is re-admitted.
            let original=inputs.directory.join("retained-original-request.json");
            fs::rename(inputs.request(),&original).unwrap();
            let replacement=b"intervening native request bytes must remain";
            let mut named=fs::OpenOptions::new().write(true).create_new(true).mode(0o600)
                .open(inputs.request()).unwrap();
            named.write_all(replacement).unwrap();named.sync_all().unwrap();
            let cleanup=inputs.remove_after_success().unwrap_err();
            assert_eq!(cleanup.to_string(),"admission file relation changed");
            assert_eq!(fs::read(inputs.request()).unwrap(),replacement);
            assert_eq!(fs::read(&original).unwrap(),request);
            assert!(!inputs.retention_facts()["request_current_identity_verified"].as_bool().unwrap());
            assert!(inputs.restore_request_after_cleanup_failure(request).is_err());
            assert_eq!(fs::read(inputs.request()).unwrap(),replacement);
            // The same inode with mutated bytes also refuses before unlink.
            let mut changed=Inputs::create(&root,request).unwrap();
            fs::write(changed.request(),replacement).unwrap();
            let cleanup=changed.remove_after_success().unwrap_err();
            assert_eq!(cleanup.to_string(),"retained admission request bytes changed; no overwrite attempted");
            assert_eq!(fs::read(changed.request()).unwrap(),replacement);
        }
        #[test]
        fn actual_inherited_acl_refuses_before_first_request_byte() {
            let root=fixture("inherited-acl");let directory=root.join("admission-inputs");fs::create_dir(&directory).unwrap();
            fs::set_permissions(&directory,fs::Permissions::from_mode(0o700)).unwrap();
            #[cfg(target_os="linux")]
            let mut command={let mut c=Command::new("/usr/bin/setfacl");c.args(["-m","d:u:65534:r-x"]).arg(&directory);c};
            #[cfg(target_os="macos")]
            let mut command={let mut c=Command::new("/bin/chmod");c.arg("+a").arg("everyone allow read,readattr,readextattr,readsecurity,file_inherit,directory_inherit").arg(&directory);c};
            command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
            let output=epilogos_workcell_runtime::run_bounded_process(command,Duration::from_secs(2),4096).unwrap();
            fs::write(root.join("native-acl.stdout"),&output.stdout).unwrap();fs::write(root.join("native-acl.stderr"),&output.stderr).unwrap();
            assert!(output.status.success()&&!output.timed_out&&output.output_complete&&!output.output_truncated,
                "actual ACL grant could not be established; not a passing privacy gate");
            let held=private::open_directory(&directory).unwrap();
            assert!(private::privacy(&held,true).is_err(),"real grant must be observed by native held-FD oracle");
            let error=match Inputs::create(&root,b"private request must not be written") {Ok(_)=>panic!("native inherited grant was accepted"),Err(error)=>error};
            assert!(std::error::Error::source(&error).is_some());
            #[cfg(target_os="linux")]
            assert_eq!(error.cause.to_string(),"private admission object has a native POSIX ACL");
            #[cfg(target_os="macos")]
            assert_eq!(error.cause.to_string(),"private admission object has a native extended ACL");
            assert!(fs::read_dir(&directory).unwrap().next().is_none(),"privacy admission must refuse before request or lock payload creation");
            fs::write(root.join("native-refusal.json"),serde_json::to_vec(&json!({"refusal":error.to_string(),"command_invoked":false})).unwrap()).unwrap();
        }
        #[test]
        fn actual_file_size_failure_keeps_primary_capture_and_truthful_partial_stage() {
            if !isolated("combined::local_cli::admission_inputs::tests::actual_file_size_failure_keeps_primary_capture_and_truthful_partial_stage") {return;}
            let root=fixture("partial-stage");let request=b"exact staged input remains after native publication failure";
            let mut inputs=Inputs::create(&root,request).unwrap();let failure=native_failure(16384);
            let mut original=std::mem::MaybeUninit::<libc::rlimit>::uninit();
            assert_eq!(unsafe {libc::getrlimit(libc::RLIMIT_FSIZE,original.as_mut_ptr())},0);
            let original=unsafe {original.assume_init()};let bounded=libc::rlimit {rlim_cur:4096,rlim_max:original.rlim_max};
            let old_signal=unsafe {libc::signal(libc::SIGXFSZ,libc::SIG_IGN)};
            assert_eq!(unsafe {libc::setrlimit(libc::RLIMIT_FSIZE,&bounded)},0);
            let retained=inputs.retain_capture(&failure);
            let restore=unsafe {libc::setrlimit(libc::RLIMIT_FSIZE,&original)};
            unsafe {libc::signal(libc::SIGXFSZ,old_signal)};
            assert_eq!(restore,0);
            let cause=retained.cause.as_ref().expect("real FSIZE publication must fail");
            assert_eq!(cause.raw_os_error(),Some(libc::EFBIG));
            assert_eq!(retained.facts["retention_complete"],false);
            assert_eq!(retained.facts["native_capture_observation"],failure.observation());
            assert_eq!(fs::read(inputs.request()).unwrap(),request);
            let partial=inputs.directory.join(".stdout.bin.writing");
            let held=fs::File::open(&partial).unwrap();private::privacy(&held,false).unwrap();
            assert!(held.metadata().unwrap().len()>0&&held.metadata().unwrap().len()<16384);
            assert!(!inputs.directory.join("stdout.bin").exists());
            assert!(!inputs.directory.join("capture-failure.json").exists());
            assert!(retained.facts["remaining_created_files"].as_array().unwrap().iter().any(|v|
                v["name"]==".stdout.bin.writing"&&v["current_entry"]["presence_observed"]==true&&v["durable_publication_observed"]==false));
            fs::write(root.join("native-result.json"),serde_json::to_vec(&retained.facts).unwrap()).unwrap();
        }
        #[test]
        fn actual_capacity_refuses_new_command_input_without_overwriting_prior_files() {
            let root=fixture("capacity");let mut inputs=Vec::new();
            for number in 0..INVOCATION_LIMIT {
                let value=format!("actual independent input {number}");inputs.push(Inputs::create(&root,value.as_bytes()).unwrap());
            }
            let error=match Inputs::create(&root,b"one too many") {Ok(_)=>panic!("bounded native input capacity ignored"),Err(error)=>error};
            assert!(error.to_string().contains("native input staging refused"));
            assert_eq!(fs::read_dir(root.join("admission-inputs")).unwrap().count(),INVOCATION_LIMIT+1);
            for (number,input) in inputs.iter().enumerate() {
                assert_eq!(fs::read(input.request()).unwrap(),format!("actual independent input {number}").as_bytes());
            }
            fs::write(root.join("native-result.json"),serde_json::to_vec(&json!({"refusal":error.to_string(),"old_inputs_preserved":true,"command_invoked":false})).unwrap()).unwrap();
        }
    }

}

// A finite observation of the authored input, held across the external call.
// The bounded bytes and native object are retained; no mutable pathname reread
// is allowed to allocate unbounded memory or silently adopt a replacement.
struct RunAdmissionSource {
    file: fs::File,
    metadata: fs::Metadata,
    bytes: Vec<u8>,
}
impl RunAdmissionSource {
    const LIMIT: u64 = 1_048_576;
    fn invalid(message: &'static str) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::InvalidData,message)
    }
    fn open(source: &Path) -> std::io::Result<fs::File> {
        let mut options=fs::OpenOptions::new();options.read(true);
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            // A replaced FIFO opens without waiting and is refused as nonregular
            // before any read. Final symlinks are never followed.
            options.custom_flags(libc::O_NONBLOCK|libc::O_NOFOLLOW|libc::O_CLOEXEC);
        }
        #[cfg(windows)] {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_FLAG_OPEN_REPARSE_POINT, so a final link is inspected itself.
            options.custom_flags(0x0020_0000);
        }
        let file=options.open(source)?;
        let metadata=file.metadata()?;
        if !metadata.is_file() || metadata.len()>Self::LIMIT {
            return Err(Self::invalid("Agency source must be a regular file no larger than 1 MiB"));
        }
        #[cfg(windows)] {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes()&0x400!=0 {
                return Err(Self::invalid("Agency source may not be a native reparse point"));
            }
        }
        Ok(file)
    }
    #[cfg(unix)]
    fn identity(file:&fs::File)->std::io::Result<(u64,u64)> {
        use std::os::unix::fs::MetadataExt;
        let metadata=file.metadata()?;Ok((metadata.dev(),metadata.ino()))
    }
    #[cfg(windows)]
    fn identity(file:&fs::File)->std::io::Result<(u64,u64)> {
        use std::os::windows::io::AsRawHandle;
        #[repr(C)]
        struct Information {
            attributes:u32,creation:[u32;2],access:[u32;2],write:[u32;2],
            volume:u32,size_high:u32,size_low:u32,links:u32,index_high:u32,index_low:u32,
        }
        #[link(name="kernel32")]
        extern "system" {
            fn GetFileInformationByHandle(handle:*mut std::ffi::c_void,information:*mut Information)->i32;
        }
        let mut information=std::mem::MaybeUninit::<Information>::uninit();
        if unsafe {GetFileInformationByHandle(file.as_raw_handle(),information.as_mut_ptr())}==0 {
            return Err(std::io::Error::last_os_error());
        }
        let information=unsafe {information.assume_init()};
        Ok((u64::from(information.volume),(u64::from(information.index_high)<<32)|u64::from(information.index_low)))
    }
    #[cfg(not(any(unix,windows)))]
    fn identity(file:&fs::File)->std::io::Result<(u64,u64)> {
        // Preserve the existing other-platform read path, without advertising
        // native identity faculty. Such targets already lack run capture.
        let metadata=file.metadata()?;Ok((metadata.len(),0))
    }
    fn same_metadata(left:&fs::Metadata,right:&fs::Metadata)->std::io::Result<bool> {
        let same=left.is_file()&&right.is_file()&&left.len()==right.len()
            &&left.modified()?==right.modified()?;
        #[cfg(unix)] {
            use std::os::unix::fs::MetadataExt;
            Ok(same&&left.dev()==right.dev()&&left.ino()==right.ino()
                &&left.ctime()==right.ctime()&&left.ctime_nsec()==right.ctime_nsec())
        }
        #[cfg(not(unix))] {Ok(same)}
    }
    fn read(source:&Path)->std::io::Result<Self> {
        use std::io::Read;
        let file=Self::open(source)?;
        let metadata=file.metadata()?;
        let mut bytes=Vec::new();
        (&file).take(Self::LIMIT+1).read_to_end(&mut bytes)?;
        if bytes.len() as u64>Self::LIMIT {
            return Err(Self::invalid("Agency source grew beyond 1 MiB during bounded read"));
        }
        if !Self::same_metadata(&metadata,&file.metadata()?)?
            ||source.canonicalize()?.as_path()!=source {
            return Err(Self::invalid("Agency source changed during bounded read"));
        }
        Ok(Self {file,metadata,bytes})
    }
    fn verify_current(&self,source:&Path)->std::io::Result<()> {
        if !Self::same_metadata(&self.metadata,&self.file.metadata()?)? {
            return Err(Self::invalid("held Agency source changed after admission input capture"));
        }
        let current=Self::read(source)?;
        if Self::identity(&self.file)?!=Self::identity(&current.file)?
            ||!Self::same_metadata(&self.metadata,&current.metadata)?
            ||self.bytes!=current.bytes {
            return Err(Self::invalid("Agency source relation or bytes changed after native admission"));
        }
        Ok(())
    }
}

// The Core ABI carries strings and cannot retain an IO object. Measure facts
// from this actual source error before that boundary; never reconstruct errno.
fn admission_source_io_detail(phase: &'static str, error: &std::io::Error) -> String {
    format!("Agency source {phase} failed: {error}; native cause kind={:?}, raw_os_error={:?}",
        error.kind(),error.raw_os_error())
}

fn admit_run_agency(source: &Path, agency_ref: &str, revision: &str, state_root: &Path) -> Result<Value,WorkcellError> {
    use std::process::{Command,Stdio};
    let refuse=|message:&str|WorkcellError::InvalidDemand(message.into());
    if revision.trim().is_empty() || !source.is_absolute() {
        return Err(refuse("Agency admission needs an exact canonical source and source revision"));
    }
    let canonical=source.canonicalize().map_err(|error|
        WorkcellError::InvalidDemand(admission_source_io_detail("canonicalization before native invocation",&error)))?;
    if canonical.as_path()!=source {
        return Err(refuse("Agency admission needs an exact canonical source and source revision"));
    }
    let snapshot=RunAdmissionSource::read(source).map_err(|error|
        WorkcellError::OperationFailed(admission_source_io_detail("held read before native invocation",&error)))?;
    let bytes=&snapshot.bytes;
    let request:Value=serde_json::from_slice(bytes).map_err(|e|WorkcellError::InvalidDemand(e.to_string()))?;
    if request["schema"]!="actuation.agency-actualisation/v1" || request["differentiated_binding"]["agency_ref"]!=agency_ref {
        return Err(refuse("Agency request does not name the selected native Agency"));
    }
    let mut inputs=admission_inputs::Inputs::create(state_root,bytes)
        .map_err(|e|WorkcellError::OperationFailed(e.to_string()))?;
    let program=env::var_os("OI_ACTUATION_BIN").unwrap_or_else(||"actuation".into());
    let mut command=Command::new(program);command.args(["agency","actualise"]).arg(inputs.request()).arg("--json").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let output=match epilogos_workcell_runtime::capture_bounded_process(command,std::time::Duration::from_secs(15),1_048_576) {
        Ok(output)=>output,
        Err(failure)=> {
            let primary=failure.to_string();
            let retained=inputs.retain_capture(&failure);
            // The typed owner cause remains available during retention; the
            // existing core error ABI carries a safe explicit cause summary.
            return Err(WorkcellError::OperationFailed(format!("{primary}; {}",retained.safe_summary())));
        }
    };
    let admitted=(|| {
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
            ||receipt["effects"]["materialisation"]!="not-performed"||receipt["effects"]["source_mutation"]!="not-performed" {
            return Err(refuse("Agency native admission basis changed"));
        }
        snapshot.verify_current(source).map_err(|error|
            WorkcellError::OperationFailed(admission_source_io_detail(
                "current basis after validated native admission; no Run publication or retry inferred",&error)))?;
        let digest=format!("blake3:{}",blake3::hash(bytes).to_hex());
        Ok(json!({"agency_ref":agency_ref,"agency_rev":revision,"source_ref":source,"source_digest":digest,
            "binding_revision":format!("actuation-receipt/{}",blake3::hash(&output.stdout).to_hex()),"minted_by":null,"admission":receipt}))
    })();
    match admitted {
        Ok(receipt)=> {
            if let Err(primary)=inputs.remove_after_success() {
                #[cfg(unix)] {
                    let restoration=inputs.restore_request_after_cleanup_failure(bytes);
                    let retained=inputs.retain_output(&output,"validated native Agency admission returned; caller input cleanup failed");
                    return Err(WorkcellError::OperationFailed(format!(
                        "native Agency admission returned and was validated (receipt digest blake3:{}), but input cleanup failed: kind={:?}, raw_os_error={:?}; request restoration kind={:?}, raw_os_error={:?}; {}; do not repeat admission or infer Run publication",
                        blake3::hash(&output.stdout).to_hex(),primary.kind(),primary.raw_os_error(),
                        restoration.as_ref().err().map(std::io::Error::kind),restoration.as_ref().err().and_then(std::io::Error::raw_os_error),retained.safe_summary())));
                }
                #[cfg(not(unix))] {
                    // Do not repeat a failed cleanup or publish unqualified ACL
                    // evidence. The actual receipt digest remains a native fact.
                    return Err(WorkcellError::OperationFailed(format!(
                        "native Agency admission returned and was validated (receipt digest blake3:{}), but legacy input cleanup failed: kind={:?}, raw_os_error={:?}; durable private diagnostics withheld; no retry or Run publication inferred",
                        blake3::hash(&output.stdout).to_hex(),primary.kind(),primary.raw_os_error())));
                }
            }
            Ok(receipt)
        }
        Err(primary)=> {
            let retained=inputs.retain_output(&output,&primary.to_string());
            let secondary=retained.safe_summary();
            Err(match primary {
                WorkcellError::InvalidDemand(detail)=>WorkcellError::InvalidDemand(format!("{detail}; {secondary}")),
                WorkcellError::OperationFailed(detail)=>WorkcellError::OperationFailed(format!("{detail}; {secondary}")),
                other=>WorkcellError::OperationFailed(format!("{other}; {secondary}")),
            })
        }
    }
}


#[cfg(all(test,unix))]
mod source_io_tests {
    use super::*;
    use std::{fs,io,path::PathBuf,time::{Duration,Instant}};
    use std::os::unix::{ffi::OsStrExt,fs::{MetadataExt,PermissionsExt}};
    static SEQUENCE:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
    // The coordinator selects an existing owner source and placement. These
    // transient inputs qualify test artifacts; they create no product authority.
    pub(crate) fn admitted_artifact_root() -> std::io::Result<PathBuf> {
        use std::io::Read;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let invalid = |message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message);
        let input = |name| std::env::var_os(name)
            .ok_or_else(|| invalid(format!("required artifact admission input {name} is absent")));
        let selected = PathBuf::from(input("WORKCELL_TEST_ARTIFACT_ROOT")?);
        let source = PathBuf::from(input("WORKCELL_TEST_ARTIFACT_OWNER_SOURCE")?);
        if !selected.is_absolute() || !source.is_absolute() {
            return Err(invalid("artifact placement and owner source must be absolute".into()));
        }
        let base = selected.canonicalize()?;
        let base_metadata = fs::metadata(&base)?;
        if !base_metadata.is_dir() || base_metadata.uid() != unsafe { libc::geteuid() } {
            return Err(invalid("artifact root must be an existing coordinator-owned directory".into()));
        }
        let owner_ref = std::env::var("WORKCELL_TEST_ARTIFACT_OWNER_REF")
            .map_err(|_| invalid("artifact owner reference is missing or not UTF-8".into()))?;
        let admission = std::env::var("WORKCELL_TEST_ARTIFACT_ADMISSION")
            .map_err(|_| invalid("artifact admission kind is missing or not UTF-8".into()))?;
        let mut file = fs::OpenOptions::new().read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC).open(&source)?;
        let initial = file.metadata()?;
        if !initial.is_file() || initial.nlink() != 1 || initial.len() > 65_536 {
            return Err(invalid("artifact owner source must be a bounded regular single-link file".into()));
        }
        let mut bytes = Vec::new();
        (&mut file).take(65_537).read_to_end(&mut bytes)?;
        if bytes.len() > 65_536 {
            return Err(invalid("artifact owner source exceeded the admission byte limit".into()));
        }
        let identity = |m: &fs::Metadata| (m.dev(), m.ino(), m.len(), m.mtime(), m.mtime_nsec(), m.ctime(), m.ctime_nsec());
        let named = fs::symlink_metadata(&source)?;
        if !named.is_file() || identity(&initial) != identity(&file.metadata()?)
            || identity(&initial) != identity(&named) || source.canonicalize()? != source {
            return Err(invalid("artifact owner source changed or is not the selected canonical regular source".into()));
        }
        let text = std::str::from_utf8(&bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent()
            .and_then(Path::parent).ok_or_else(|| invalid("missing compiled product repository".into()))?
            .canonicalize()?;
        let mut central_context = None;
        for ancestor in repository.ancestors() {
            match fs::metadata(ancestor.join("Control/user/native-action-authority.json")) {
                Ok(metadata) if metadata.is_file() => {central_context=Some(ancestor.to_path_buf());break;}
                Ok(_) => return Err(invalid("compiled Central context marker is not a regular file".into())),
                Err(error) if error.kind()==std::io::ErrorKind::NotFound => {},
                Err(error) => return Err(error),
            }
        }
        match admission.as_str() {
            "central-clearing" => {
                let record: serde_json::Value = serde_json::from_slice(&bytes)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
                let clearing = source.parent().ok_or_else(|| invalid("missing clearing owner".into()))?;
                let central = clearing.ancestors().nth(5)
                    .ok_or_else(|| invalid("missing Central root owner".into()))?;
                let id = clearing.file_name().and_then(|value| value.to_str())
                    .ok_or_else(|| invalid("native clearing identity is not UTF-8".into()))?;
                let expected = central.join("Control/agents/now/clearings").join(id).join("now.json");
                let source_ref = format!("central:source:control:root:Control/agents/now/clearings/{id}/now.json");
                if central_context.as_deref() != Some(central) {
                    return Err(invalid("clearing source is outside the actual compiled Central working context".into()));
                }
                if source != expected || owner_ref != format!("central:now:control:root:{id}")
                    || record["schema"] != "central.now-clearing/v1" || record["now_ref"] != owner_ref
                    || record["source_ref"] != source_ref || record["scope_ref"] != "control:root"
                    || !record["policy_revision_at_allocation"].as_str().is_some_and(|value| !value.is_empty())
                    || !record["participant_refs"].as_array().is_some_and(|values| !values.is_empty()) {
                    return Err(invalid("selected owner source does not establish the actual root clearing allocation".into()));
                }
                if !base.starts_with(clearing.join("T").canonicalize()?) {
                    return Err(invalid("artifact root is outside the selected actual clearing T".into()));
                }
            }
            "product-scratch" => {
                let record: serde_json::Value = serde_json::from_slice(&bytes)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
                let project = source.parent().and_then(Path::parent)
                    .ok_or_else(|| invalid("missing declared product owner".into()))?;
                let id = record["project_id"].as_str()
                    .ok_or_else(|| invalid("declared product has no native project identity".into()))?;
                let actual_product_context = match &central_context {
                    Some(central) => project.parent() == Some(central.join("Work").as_path()),
                    None => project == repository,
                };
                if !actual_product_context {
                    return Err(invalid("product scratch source is outside the actual compiled product owner context".into()));
                }
                if record["schema"] != "central.project/v1" || id.is_empty()
                    || record["human_source"] != "ProjectCentral/user"
                    || owner_ref != format!("project:{id}")
                    || source != project.join("ProjectCentral/project.json")
                    || !base.starts_with(project.join("ProjectCentral/now/tmp").canonicalize()?) {
                    return Err(invalid("artifact root does not match the selected actual product scratch owner".into()));
                }
                // This is authored product scratch, not proof of a Run allocation.
            }
            "hosted-runner" => {
                let workspace = PathBuf::from(input("GITHUB_WORKSPACE")?).canonicalize()?;
                if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
                    || std::env::var("CI").as_deref() != Ok("true")
                    || !std::env::var("GITHUB_RUN_ID").is_ok_and(|value| !value.is_empty())
                    || !std::env::var("GITHUB_REPOSITORY").is_ok_and(|value| !value.is_empty())
                    || workspace != repository || base != workspace.join("evidence/native-processes").canonicalize()?
                    || source != workspace.join("evidence/source-commit.txt") || text.trim() != owner_ref
                    || owner_ref.len() != 40 || !owner_ref.bytes().all(|value| value.is_ascii_hexdigit()) {
                    return Err(invalid("artifact root is not the explicitly admitted source-pinned hosted staging exception".into()));
                }
                // CI staging is not a native Project, World, Run or NOW identity.
            }
            _ => return Err(invalid("unrecognised artifact admission kind".into())),
        }
        Ok(base)
    }

    struct Fixture {root:PathBuf}
    impl Fixture {
        fn new(label:&str)->Self {
            let base=admitted_artifact_root().expect("selected native/hosted artifact placement must be admitted before creation");
            let root=base.join(format!("agency-source-io-{label}-{}-{}",std::process::id(),
                SEQUENCE.fetch_add(1,std::sync::atomic::Ordering::Relaxed)));
            fs::create_dir(&root).unwrap();fs::set_permissions(&root,fs::Permissions::from_mode(0o700)).unwrap();
            Self {root}
        }
        fn record(&self,name:&str,error:&io::Error,projected:&str) {
            fs::write(self.root.join(name),serde_json::to_vec(&json!({
                "actual_native_cause":{"kind":format!("{:?}",error.kind()),"raw_os_error":error.raw_os_error(),"message":error.to_string()},
                "projected":projected,"standing":"actual filesystem observation; not native Actuation or Run acceptance"
            })).unwrap()).unwrap();
        }
    }
    struct RestorePermissions {file:fs::File,mode:u32,identity:(u64,u64)}
    impl RestorePermissions {
        fn deny(path:&Path)->Self {
            let file=fs::File::open(path).unwrap();let metadata=file.metadata().unwrap();
            let guard=Self {file,mode:metadata.mode(),identity:(metadata.dev(),metadata.ino())};
            guard.file.set_permissions(fs::Permissions::from_mode(0o0)).unwrap();guard
        }
    }
    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            // Only the held original test object is restored; no pathname chmod
            // can accidentally widen an unrelated replacement's permissions.
            let restored=(||->io::Result<()> {
                let metadata=self.file.metadata()?;
                if (metadata.dev(),metadata.ino())!=self.identity {
                    return Err(io::Error::other("owned fixture descriptor identity changed"));
                }
                self.file.set_permissions(fs::Permissions::from_mode(self.mode))
            })();
            if let Err(error)=restored {
                eprintln!("original fixture mode restoration failed: kind={:?}, raw_os_error={:?}",error.kind(),error.raw_os_error());
                if !std::thread::panicking() {panic!("original fixture mode could not be restored");}
            }
        }
    }
    fn assert_actual_cause(detail:&str,phase:&str,error:&io::Error) {
        assert!(detail.contains(phase));assert!(detail.contains(&error.to_string()));
        assert!(detail.contains(&format!("native cause kind={:?}",error.kind())));
        assert!(detail.contains(&format!("raw_os_error={:?}",error.raw_os_error())));
    }

    #[test]
    fn actual_canonicalization_io_is_distinct_from_argument_refusal_without_new_state() {
        assert_ne!(unsafe {libc::geteuid()},0,"permission observation requires a genuine nonroot native gate");
        let fixture=Fixture::new("canonical");let state=fixture.root.join("uncreated-state");
        let missing=fixture.root.join("missing-source.json");
        let oracle=missing.canonicalize().unwrap_err();assert_eq!(oracle.kind(),io::ErrorKind::NotFound);
        assert_eq!(oracle.raw_os_error(),Some(libc::ENOENT));
        let error=admit_run_agency(&missing,"agency:unavailable","selected-revision",&state).unwrap_err();
        let detail=match &error {WorkcellError::InvalidDemand(detail)=>detail,other=>panic!("changed canonicalization category: {other}")};
        assert_actual_cause(detail,"canonicalization before native invocation",&oracle);
        assert_eq!(super::exit_code(&error),2);assert_eq!(error.clone(),error);
        assert!(std::error::Error::source(&error).is_none(),"Core string ABI has no original IO source object");
        assert!(!state.exists());fixture.record("missing-canonicalization.json",&oracle,detail);
        let directory=fixture.root.join("unreadable-directory");fs::create_dir(&directory).unwrap();
        let source=directory.join("source.json");fs::write(&source,b"retained source bytes").unwrap();
        let guard=RestorePermissions::deny(&directory);
        let oracle=source.canonicalize().unwrap_err();assert_eq!(oracle.kind(),io::ErrorKind::PermissionDenied);
        assert_eq!(oracle.raw_os_error(),Some(libc::EACCES));
        let error=admit_run_agency(&source,"agency:unavailable","selected-revision",&state).unwrap_err();
        let detail=match &error {WorkcellError::InvalidDemand(detail)=>detail,other=>panic!("changed canonicalization IO category: {other}")};
        assert_actual_cause(detail,"canonicalization before native invocation",&oracle);
        assert!(!state.exists());fixture.record("denied-canonicalization.json",&oracle,detail);
        drop(guard);assert_eq!(fs::read(&source).unwrap(),b"retained source bytes");
        let grammar=admit_run_agency(Path::new("relative-source.json"),"agency:unavailable","selected-revision",&state).unwrap_err();
        assert!(matches!(grammar,WorkcellError::InvalidDemand(ref text)
            if text=="Agency admission needs an exact canonical source and source revision"));
        let empty=admit_run_agency(&source,"agency:unavailable","",&state).unwrap_err();
        assert!(matches!(empty,WorkcellError::InvalidDemand(ref text)
            if text=="Agency admission needs an exact canonical source and source revision"));
        let alias=fixture.root.join("source-alias.json");std::os::unix::fs::symlink(&source,&alias).unwrap();
        let alias_error=admit_run_agency(&alias,"agency:unavailable","selected-revision",&state).unwrap_err();
        assert!(matches!(alias_error,WorkcellError::InvalidDemand(ref text)
            if text=="Agency admission needs an exact canonical source and source revision"));
        assert!(!state.exists());
    }

    #[test]
    fn actual_held_read_permission_fifo_link_and_size_preserve_original_cause() {
        assert_ne!(unsafe {libc::geteuid()},0,"permission observation requires a genuine nonroot native gate");
        let fixture=Fixture::new("held-read");let source=fixture.root.join("source.json");
        fs::write(&source,b"actual retained material").unwrap();
        let guard=RestorePermissions::deny(&source);
        let oracle=RunAdmissionSource::read(&source).err().expect("actual denied source unexpectedly readable");
        assert_eq!(oracle.kind(),io::ErrorKind::PermissionDenied);assert_eq!(oracle.raw_os_error(),Some(libc::EACCES));
        let state=fixture.root.join("uncreated-state");
        let error=admit_run_agency(&source,"agency:unavailable","selected-revision",&state).unwrap_err();
        let detail=match &error {WorkcellError::OperationFailed(detail)=>detail,other=>panic!("changed held-read category: {other}")};
        assert_actual_cause(detail,"held read before native invocation",&oracle);assert_eq!(super::exit_code(&error),7);
        assert!(!state.exists());fixture.record("actual-denied-held-read.json",&oracle,detail);
        drop(guard);assert_eq!(fs::read(&source).unwrap(),b"actual retained material");
        let fifo=fixture.root.join("source-fifo");
        let native_name=std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe {libc::mkfifo(native_name.as_ptr(),0o600)},0);
        let began=Instant::now();let refusal=RunAdmissionSource::read(&fifo).err().expect("FIFO admitted as Source");
        assert!(began.elapsed()<Duration::from_secs(1),"actual FIFO open must remain nonblocking");
        assert_eq!(refusal.kind(),io::ErrorKind::InvalidData);assert!(refusal.raw_os_error().is_none());
        let projected=admission_source_io_detail("held read before native invocation",&refusal);
        assert_actual_cause(&projected,"held read before native invocation",&refusal);
        fixture.record("actual-fifo-refusal.json",&refusal,&projected);
        let link=fixture.root.join("source-link");std::os::unix::fs::symlink(&source,&link).unwrap();
        let refusal=RunAdmissionSource::read(&link).err().expect("final symlink admitted");
        assert_eq!(refusal.raw_os_error(),Some(libc::ELOOP));
        let projected=admission_source_io_detail("held read before native invocation",&refusal);
        fixture.record("actual-link-refusal.json",&refusal,&projected);assert_eq!(fs::read(&source).unwrap(),b"actual retained material");
        let large=fixture.root.join("source-oversize");fs::write(&large,vec![b'x';RunAdmissionSource::LIMIT as usize+1]).unwrap();
        let refusal=RunAdmissionSource::read(&large).err().expect("oversized material admitted");
        assert_eq!(refusal.kind(),io::ErrorKind::InvalidData);assert!(refusal.raw_os_error().is_none());
        let projected=admission_source_io_detail("held read before native invocation",&refusal);
        fixture.record("actual-size-refusal.json",&refusal,&projected);
        assert_eq!(fs::metadata(large).unwrap().len(),RunAdmissionSource::LIMIT+1);
    }

    #[test]
    fn actual_current_basis_missing_and_changed_material_are_not_reconstructed_io() {
        let fixture=Fixture::new("current");let directory=fixture.root.join("source-directory");
        fs::create_dir(&directory).unwrap();let source=directory.join("source.json");
        fs::write(&source,b"original native material").unwrap();let snapshot=RunAdmissionSource::read(&source).unwrap();
        let parked=fixture.root.join("retained-original-directory");fs::rename(&directory,&parked).unwrap();
        // Renaming the parent preserves the held child's metadata. An unlink
        // would change ctime and exercise a different real policy refusal.
        assert!(RunAdmissionSource::same_metadata(&snapshot.metadata,&snapshot.file.metadata().unwrap()).unwrap());
        let oracle=fs::File::open(&source).unwrap_err();assert_eq!(oracle.kind(),io::ErrorKind::NotFound);
        let failure=snapshot.verify_current(&source).unwrap_err();assert_eq!(failure.kind(),oracle.kind());
        assert_eq!(failure.raw_os_error(),oracle.raw_os_error());assert_eq!(failure.raw_os_error(),Some(libc::ENOENT));
        let phase="current basis after validated native admission; no Run publication or retry inferred";
        let projected=admission_source_io_detail(phase,&failure);assert_actual_cause(&projected,phase,&failure);
        fixture.record("actual-current-missing.json",&failure,&projected);
        assert_eq!(fs::read(parked.join("source.json")).unwrap(),b"original native material");
        fs::create_dir(&directory).unwrap();fs::write(&source,b"original native material").unwrap();
        let snapshot=RunAdmissionSource::read(&source).unwrap();let retained=directory.join("old-source.json");
        fs::rename(&source,&retained).unwrap();fs::write(&source,b"replacement material").unwrap();
        let failure=snapshot.verify_current(&source).unwrap_err();assert_eq!(failure.kind(),io::ErrorKind::InvalidData);
        assert!(failure.raw_os_error().is_none());let projected=admission_source_io_detail(phase,&failure);
        fixture.record("actual-current-replacement.json",&failure,&projected);
        assert_eq!(fs::read(retained).unwrap(),b"original native material");assert_eq!(fs::read(&source).unwrap(),b"replacement material");
        let snapshot=RunAdmissionSource::read(&source).unwrap();fs::write(&source,b"same inode changed bytes").unwrap();
        let failure=snapshot.verify_current(&source).unwrap_err();assert_eq!(failure.kind(),io::ErrorKind::InvalidData);
        assert!(failure.raw_os_error().is_none());let projected=admission_source_io_detail(phase,&failure);
        fixture.record("actual-current-byte-change.json",&failure,&projected);assert_eq!(fs::read(source).unwrap(),b"same inode changed bytes");
        // This directly exercises actual current material IO only. It does not
        // fabricate an Actuation receipt or claim post-native retention ran.
    }
}
