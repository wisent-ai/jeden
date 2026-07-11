use super::process;
use super::types::{bounded_json, command_exists, nonempty, HealthDescriptor, ServiceError, ServiceResult};
use crate::tool_runtime::runtime_ops::OperationContext;
use parking_lot::Mutex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

pub(crate) const TOOLS: &[(&str, &str)] = &[
    ("ssh_read", "Read a bounded remote file through a reusable SSH connection"),
    ("ssh_search", "Search a remote file using a reusable SSH connection"),
    ("ssh_write", "Write a remote file through a reusable SSH connection"),
    ("ssh_exec", "Execute a remote command through a reusable SSH connection"),
];
#[derive(Clone)] struct Host { target: String, port: Option<u16>, identity: Option<String> }
pub(crate) struct SshService { cwd: PathBuf, hosts: BTreeMap<String,Host>, connected: Mutex<BTreeSet<String>> }
impl SshService {
    pub(crate) fn discover(cwd: &Path, config: &Value) -> Self {
        let mut hosts = BTreeMap::new();
        if let Some(values)=config.get("sshHosts").or_else(|| config.pointer("/toolServices/ssh/hosts")).and_then(Value::as_object) {
            for (alias,value) in values.iter().take(64) {
                let host = if let Some(target)=value.as_str(){Some(Host{target:target.into(),port:None,identity:None})}else{value.get("host").and_then(Value::as_str).map(|target|Host{target: if let Some(user)=value.get("user").and_then(Value::as_str){format!("{user}@{target}")}else{target.into()},port:value.get("port").and_then(Value::as_u64).and_then(|v|u16::try_from(v).ok()),identity:value.get("identity").or_else(||value.get("key")).and_then(Value::as_str).map(str::to_owned)})};
                if let Some(host)=host{hosts.insert(alias.clone(),host);}
            }
        }
        Self { cwd:cwd.to_path_buf(), hosts, connected:Mutex::new(BTreeSet::new()) }
    }
    pub(crate) fn health(&self)->HealthDescriptor { if command_exists("ssh"){HealthDescriptor::healthy("ssh","OpenSSH")}else{HealthDescriptor::unavailable("ssh","ssh executable not found")} }
    pub(crate) fn execute(&self,tool:&str,input:&Value,context:&OperationContext<'_>,allow_write:bool,allow_command:bool)->ServiceResult<Value>{
        if !allow_command{return Err(ServiceError::PermissionDenied("SSH requires command permission".into()));}
        if !self.health().available(){return Err(ServiceError::Unavailable{service:"ssh",detail:self.health().detail});}
        let uri=nonempty(input.get("uri"),"uri")?; let (alias,path)=parse_uri(&uri)?; let host=self.hosts.get(&alias).ok_or_else(||ServiceError::Unavailable{service:"ssh",detail:format!("host alias {alias} is not configured")})?.clone();
        let common=self.ensure_connection(&alias,&host,context)?;
        let (remote,stdin)=match tool{
            "ssh_read"=>(format!("cat -- {}",quote(&path)),None),
            "ssh_search"=>{let pattern=nonempty(input.get("pattern"),"pattern")?;(format!("grep -nE -- {} {}",quote(&pattern),quote(&path)),None)},
            "ssh_write"=>{if !allow_write{return Err(ServiceError::PermissionDenied("SSH write requires write permission".into()));}let content=nonempty(input.get("content"),"content")?;(format!("cat > {}",quote(&path)),Some(content.into_bytes()))},
            "ssh_exec"=>{let command=nonempty(input.get("command"),"command")?;(command,None)},
            _=>return Err(ServiceError::InvalidInput(format!("unknown SSH tool {tool}"))),
        };
        let mut args=common;args.push(host.target);args.push(remote);
        let output=process::run("ssh",context,&self.cwd,"ssh",&args,stdin,Duration::from_secs(input.get("timeoutSeconds").and_then(Value::as_u64).unwrap_or(60).clamp(1,300)))?;
        bounded_json(context,"ssh",&json!({"ok":true,"uri":uri,"output":output,"connectionReused":true}))
    }
    fn ensure_connection(&self,alias:&str,host:&Host,context:&OperationContext<'_>)->ServiceResult<Vec<String>>{
        let digest=hex::encode(Sha256::digest(format!("{}:{}",self.cwd.display(),alias).as_bytes()));
        let socket=context.artifacts().root().join(format!("ssh-control-{}",&digest[..16]));
        let mut args=vec!["-o".into(),"ControlMaster=auto".into(),"-o".into(),"ControlPersist=300".into(),"-o".into(),format!("ControlPath={}",socket.display()),"-o".into(),"BatchMode=yes".into()];
        if let Some(port)=host.port{args.extend(["-p".into(),port.to_string()]);} if let Some(identity)=&host.identity{args.extend(["-i".into(),identity.clone()]);}
        let key=format!("{}:{}",alias,socket.display()); let needs_connect=!self.connected.lock().contains(&key);
        if needs_connect { let mut connect=args.clone();connect.extend(["-MNf".into(),host.target.clone()]);process::run("ssh",context,&self.cwd,"ssh",&connect,None,Duration::from_secs(20))?;self.connected.lock().insert(key); }
        Ok(args)
    }
    #[cfg(test)] pub(crate) fn mark_connected_for_test(&self,key:&str){self.connected.lock().insert(key.into());}
}
fn parse_uri(uri:&str)->ServiceResult<(String,String)>{let url=Url::parse(uri).map_err(|e|ServiceError::InvalidInput(format!("invalid SSH URI: {e}")))?;if url.scheme()!="ssh"{return Err(ServiceError::InvalidInput("URI scheme must be ssh".into()));}let alias=url.host_str().filter(|v|!v.is_empty()).ok_or_else(||ServiceError::InvalidInput("SSH URI needs a host alias".into()))?;let path=url.path();if path.is_empty()||path=="/"{return Err(ServiceError::InvalidInput("SSH URI needs an absolute path".into()));}Ok((alias.into(),path.into()))}
fn quote(value:&str)->String{format!("'{}'",value.replace('\'',"'\\''"))}
