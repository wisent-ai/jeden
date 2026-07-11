use super::*;
use crate::tool_runtime::runtime_ops::{ArtifactSink,CancellationToken,OperationContext,OutputLimits};
use serde_json::json;
use std::fs;
use std::path::{Path,PathBuf};
use std::sync::atomic::{AtomicU64,Ordering};
use std::sync::Mutex as StdMutex;

static ID:AtomicU64=AtomicU64::new(1);
static ENV:StdMutex<()>=StdMutex::new(());
fn temp(name:&str)->PathBuf{let path=std::env::temp_dir().join(format!("jeden-tool-services-{name}-{}-{}",std::process::id(),ID.fetch_add(1,Ordering::Relaxed)));fs::create_dir_all(&path).unwrap();path}
fn operation(root:&Path)->OperationContext<'static>{OperationContext::new(CancellationToken::new(),ArtifactSink::new(root)).with_output_limits(OutputLimits{head_bytes:1024,tail_bytes:1024})}
#[cfg(unix)]fn executable(path:&Path,body:&str){use std::os::unix::fs::PermissionsExt;fs::write(path,body).unwrap();let mut permissions=fs::metadata(path).unwrap().permissions();permissions.set_mode(0o755);fs::set_permissions(path,permissions).unwrap();}
fn serve_once(status:&str,content_type:&str,body:Vec<u8>)->String{use std::io::{Read,Write};use std::net::TcpListener;let listener=TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();let status=status.to_string();let content_type=content_type.to_string();std::thread::spawn(move||{let(mut stream,_)=listener.accept().unwrap();let mut request=[0u8;8192];let _=stream.read(&mut request);let header=format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());stream.write_all(header.as_bytes()).unwrap();stream.write_all(&body).unwrap();});format!("http://{address}")}

#[test]
fn health_controls_registry_availability(){
 let _guard=ENV.lock().unwrap();let old=std::env::var_os("PATH");std::env::set_var("PATH","");
 let root=temp("health");let descriptors=capability_descriptors(&root);
 assert!(descriptors.iter().any(|d|d.id=="tool/image_inspect"&&d.ui.executable));
 assert!(descriptors.iter().any(|d|d.id=="tool/browser_tab"&&!d.ui.executable&&d.health.state==HealthState::Unavailable));
 invalidate(&root);if let Some(value)=old{std::env::set_var("PATH",value)}else{std::env::remove_var("PATH")};
}

#[test]
fn browser_fixture_reuses_session_preserves_screenshot_and_handles_error_cancel(){
 let _guard=ENV.lock().unwrap();
 let root=temp("browser");let bridge=root.join("browser-bridge");
 executable(&bridge,"#!/bin/sh\ninput=$(cat)\ncase \"$input\" in *screenshot*) printf '%s\\n' '{\"ok\":true,\"data\":\"iVBORw0KGgo=\",\"format\":\"png\",\"tab\":\"t1\"}' ;; *) printf '%s\\n' '{\"ok\":true,\"tab\":\"t1\"}' ;; esac\n");
 let service=browser::BrowserService::discover(&root,&json!({"toolServices":{"browser":{"bridge":bridge}}}));
 assert_eq!(service.session_id_for_test("s"),service.session_id_for_test("s"));
 let op=operation(&root.join("artifacts"));assert!(service.execute("browser_tab",&json!({"action":"open","session":"s"}),&op).is_ok());
 let shot=service.execute("browser_screenshot",&json!({"session":"s"}),&op).unwrap();assert!(Path::new(shot["artifact"].as_str().unwrap()).is_file());
 assert!(matches!(service.execute("browser_tab",&json!({}),&op),Err(ServiceError::InvalidInput(_))));
 let cancelled=CancellationToken::new();cancelled.cancel();let cancelled_op=OperationContext::new(cancelled,ArtifactSink::new(root.join("cancel")));assert_eq!(service.execute("browser_tab",&json!({"action":"open"}),&cancelled_op).unwrap_err(),ServiceError::Cancelled);
}

#[test]
fn debugger_fixture_reuses_adapter_and_propagates_failure_cancel(){
 let _guard=ENV.lock().unwrap();
 let root=temp("debugger");let adapter=root.join("adapter.py");
 executable(&adapter,"#!/usr/bin/env python3\nimport json,sys\nr=sys.stdin.buffer;w=sys.stdout.buffer\nwhile True:\n h=r.readline()\n if not h: break\n n=int(h.split(b':',1)[1]);r.readline();q=json.loads(r.read(n));ok=q['command']!='fail';reply={'seq':q['seq'],'type':'response','request_seq':q['seq'],'success':ok,'message':'fixture failure' if not ok else ''};b=json.dumps(reply).encode();w.write(f'Content-Length: {len(b)}\\r\\n\\r\\n'.encode()+b);w.flush()\n");
 let service=debugger::DebuggerService::discover(&root,&json!({"toolServices":{"debugger":{"command":adapter}}}));let op=operation(&root.join("artifacts"));
 let first=service.execute("debug_request",&json!({"session":"one","command":"threads"}),&op);assert!(first.is_ok(),"{first:?}");assert!(service.execute("debug_request",&json!({"session":"one","command":"stackTrace"}),&op).is_ok());assert_eq!(service.session_count(),1);
 assert!(matches!(service.execute("debug_request",&json!({"session":"one","command":"fail"}),&op),Err(ServiceError::Backend{..})));
 let token=CancellationToken::new();token.cancel();let cancelled=OperationContext::new(token,ArtifactSink::new(root.join("cancel")));assert_eq!(service.execute("debug_request",&json!({"session":"one","command":"threads"}),&cancelled).unwrap_err(),ServiceError::Cancelled);
}

#[test]
fn media_inspection_success_error_cancel_and_artifact_bytes(){
 let root=temp("media");let image=root.join("pixel.png");let mut png=b"\x89PNG\r\n\x1a\n".to_vec();png.extend_from_slice(&[0;8]);png.extend_from_slice(&2u32.to_be_bytes());png.extend_from_slice(&3u32.to_be_bytes());fs::write(&image,&png).unwrap();
 let service=media::MediaService::discover(&root,&json!({}));let op=operation(&root.join("artifacts"));let result=service.execute("image_inspect",&json!({"path":"pixel.png"}),&op).unwrap();assert_eq!(result["width"],2);assert_eq!(result["height"],3);
 assert!(matches!(service.execute("image_inspect",&json!({"path":"missing.png"}),&op),Err(ServiceError::Io(_))));
 let artifact=types::write_media_artifact(&op,"fixture","png",&png).unwrap();assert_eq!(fs::read(artifact["artifact"].as_str().unwrap()).unwrap(),png);
 let token=CancellationToken::new();token.cancel();let cancelled=OperationContext::new(token,ArtifactSink::new(root.join("cancel")));assert_eq!(service.execute("image_inspect",&json!({"path":"pixel.png"}),&cancelled).unwrap_err(),ServiceError::Cancelled);
}

#[test]
fn ssh_uri_validation_and_unavailable_backend_are_typed(){
 let _guard=ENV.lock().unwrap();
 let root=temp("ssh");let service=ssh::SshService::discover(&root,&json!({"sshHosts":{"fixture":"localhost"}}));let op=operation(&root.join("artifacts"));
 assert!(matches!(service.execute("ssh_read",&json!({"uri":"http://fixture/a"}),&op,false,true),Err(ServiceError::InvalidInput(_))));
 let token=CancellationToken::new();token.cancel();let cancelled=OperationContext::new(token,ArtifactSink::new(root.join("cancel")));let result=service.execute("ssh_read",&json!({"uri":"ssh://fixture/a"}),&cancelled,false,true);assert!(matches!(result,Err(ServiceError::Cancelled)|Err(ServiceError::Unavailable{..})));
}

#[test]
fn github_fixture_covers_success_error_and_cancel(){
 let _guard=ENV.lock().unwrap();let root=temp("github");let bin=root.join("bin");fs::create_dir_all(&bin).unwrap();executable(&bin.join("gh"),"#!/bin/sh\nprintf '%s' '[{\"title\":\"fixture\",\"url\":\"https://github.com/example/repo/issues/1\"}]'\n");executable(&bin.join("git"),"#!/bin/sh\nexit 0\n");let old=std::env::var_os("PATH");std::env::set_var("PATH",&bin);
 let service=github::GithubService::discover(&root,&json!({}));let op=operation(&root.join("artifacts"));let success=service.execute("github_search",&json!({"kind":"issues","query":"fixture"}),&op,false,true).unwrap();assert_eq!(success[0]["title"],"fixture");
 assert!(matches!(service.execute("git_guarded_push",&json!({"confirm":true}),&op,false,true),Err(ServiceError::PermissionDenied(_))));assert!(matches!(service.execute("github_search",&json!({"kind":"invalid","query":"x"}),&op,false,true),Err(ServiceError::InvalidInput(_))));
 let token=CancellationToken::new();token.cancel();let cancelled=OperationContext::new(token,ArtifactSink::new(root.join("cancel")));assert_eq!(service.execute("github_search",&json!({"kind":"issues","query":"x"}),&cancelled,false,true).unwrap_err(),ServiceError::Cancelled);
 if let Some(value)=old{std::env::set_var("PATH",value)}else{std::env::remove_var("PATH")};
}

#[test]
fn web_without_provider_is_unavailable_and_cancel_is_typed(){
 let root=temp("web");let service=web::WebService::discover(&root,&json!({}));let op=operation(&root.join("artifacts"));assert!(matches!(service.execute(&json!({"query":"rust"}),&op),Err(ServiceError::Unavailable{..})));
 let token=CancellationToken::new();token.cancel();let cancelled=OperationContext::new(token,ArtifactSink::new(root.join("cancel")));assert_eq!(service.execute(&json!({"query":"rust"}),&cancelled).unwrap_err(),ServiceError::Cancelled);
}

#[test]
fn ssh_fixture_reuses_control_connection_and_supports_success(){
 let _guard=ENV.lock().unwrap();let root=temp("ssh-reuse");let bin=root.join("bin");fs::create_dir_all(&bin).unwrap();let count=root.join("masters");let script=bin.join("ssh");executable(&script,&format!("#!/bin/sh\nfor arg in \"$@\"; do if [ \"$arg\" = \"-MNf\" ]; then echo master >> '{}'; exit 0; fi; done\nprintf remote-output\n",count.display()));let old=std::env::var_os("PATH");std::env::set_var("PATH",&bin);
 let service=ssh::SshService::discover(&root,&json!({"sshHosts":{"fixture":"fixture.invalid"}}));let op=operation(&root.join("artifacts"));for _ in 0..2{let result=service.execute("ssh_read",&json!({"uri":"ssh://fixture/tmp/a"}),&op,false,true).unwrap();assert_eq!(result["output"],"remote-output");}assert_eq!(fs::read_to_string(&count).unwrap().lines().count(),1);
 if let Some(value)=old{std::env::set_var("PATH",value)}else{std::env::remove_var("PATH")};
}

#[test]
fn web_fixture_falls_back_and_keeps_url_citations(){
 let endpoint=serve_once("200 OK","application/json",br#"{"web":{"results":[{"title":"Rust","url":"https://www.rust-lang.org/","description":"language"}]}}"#.to_vec());let service=web::WebService::discover(Path::new("."),&json!({"toolServices":{"web":{"tavilyApiKey":"bad","tavilyEndpoint":"http://127.0.0.1:1","braveApiKey":"ok","braveEndpoint":endpoint}}}));let root=temp("web-success");let result=service.execute(&json!({"query":"rust","limit":1}),&operation(&root)).unwrap();assert_eq!(result["provider"],"brave");assert_eq!(result["citations"][0]["url"],"https://www.rust-lang.org/");
}

#[test]
fn image_generation_and_tts_fixtures_preserve_media_artifacts(){
 let image_endpoint=serve_once("200 OK","application/json",br#"{"data":[{"b64_json":"iVBORw0KGgo="}]}"#.to_vec());let image=media::MediaService::discover(Path::new("."),&json!({"toolServices":{"image":{"apiKey":"fixture","baseUrl":image_endpoint}}}));let root=temp("image-provider");let generated=image.execute("image_generate",&json!({"prompt":"pixel"}),&operation(&root)).unwrap();assert_eq!(fs::read(generated["artifact"].as_str().unwrap()).unwrap(),base64::Engine::decode(&base64::engine::general_purpose::STANDARD,"iVBORw0KGgo=").unwrap());
 let tts_endpoint=serve_once("200 OK","audio/mpeg",b"fixture-audio".to_vec());let tts=media::MediaService::discover(Path::new("."),&json!({"toolServices":{"image":{"apiKey":"fixture","baseUrl":tts_endpoint}}}));let audio=tts.execute("tts",&json!({"text":"hello","format":"mp3"}),&operation(&root)).unwrap();assert_eq!(fs::read(audio["artifact"].as_str().unwrap()).unwrap(),b"fixture-audio");
}
