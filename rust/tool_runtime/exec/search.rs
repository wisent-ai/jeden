use glob::Pattern;
use ignore::{WalkBuilder, WalkState};
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::tool_runtime::shared::{bool_input, jail_path, string_input, u64_input};
use crate::tool_runtime::ToolRuntime;

const MAX_SEARCH_FILES: usize = 20_000;
const MAX_SEARCH_FILE_BYTES: u64 = 8 * 1024 * 1024;

fn check(runtime: &ToolRuntime<'_>) -> Result<(), String> {
    if runtime.operation.cancellation().is_cancelled() { return Err("search cancelled".into()); }
    if runtime.operation.deadline().is_some_and(|deadline| std::time::Instant::now() >= deadline) { return Err("search deadline exceeded".into()); }
    Ok(())
}

fn rel_path(cwd: &Path, file: &Path) -> String { file.strip_prefix(cwd).unwrap_or(file).to_string_lossy().replace('\\', "/") }

fn roots(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Vec<PathBuf>, String> {
    if let Some(paths)=input.get("paths").and_then(Value::as_array) { return paths.iter().filter_map(Value::as_str).map(|path|jail_path(runtime.cwd,path)).collect(); }
    Ok(vec![jail_path(runtime.cwd,&string_input(input,"path").unwrap_or_else(||".".into()))?])
}

fn discover(runtime: &ToolRuntime<'_>, input: &Value, include_dirs: bool) -> Result<Vec<(PathBuf,bool)>,String> {
    check(runtime)?;
    let hidden=bool_input(input,"hidden",false);
    let gitignore=bool_input(input,"gitignore",true);
    let output=Arc::new(Mutex::new(Vec::<(PathBuf,bool)>::new()));
    let error=Arc::new(Mutex::new(None::<String>));
    for root in roots(runtime,input)? {
        let metadata=fs::metadata(&root).map_err(|value|value.to_string())?;
        if metadata.is_file() { output.lock().map_err(|_|"search result lock poisoned")?.push((root,false)); continue; }
        let mut builder=WalkBuilder::new(&root);
        builder.hidden(!hidden).git_ignore(gitignore).git_exclude(gitignore).ignore(gitignore).parents(gitignore).require_git(false).threads(std::thread::available_parallelism().map(usize::from).unwrap_or(2).min(8));
        let output=Arc::clone(&output); let error=Arc::clone(&error); let cancellation=runtime.operation.cancellation().clone();
        builder.build_parallel().run(|| {
            let output=Arc::clone(&output); let error=Arc::clone(&error); let cancellation=cancellation.clone(); let root=root.clone();
            Box::new(move |entry| {
                if cancellation.is_cancelled() { return WalkState::Quit; }
                let entry=match entry { Ok(entry)=>entry,Err(value)=>{ if let Ok(mut slot)=error.lock(){ if slot.is_none(){*slot=Some(value.to_string());} } return WalkState::Continue; } };
                if entry.path()==root { return WalkState::Continue; }
                let is_dir=entry.file_type().is_some_and(|kind|kind.is_dir());
                if !include_dirs && is_dir { return WalkState::Continue; }
                if let Ok(mut values)=output.lock() { if values.len()>=MAX_SEARCH_FILES { return WalkState::Quit; } values.push((entry.into_path(),is_dir)); }
                WalkState::Continue
            })
        });
        check(runtime)?;
    }
    if let Some(error)=error.lock().map_err(|_|"search error lock poisoned")?.take(){return Err(error);}
    let mut values=Arc::try_unwrap(output).map_err(|_|"search workers still active")?.into_inner().map_err(|_|"search result lock poisoned")?;
    values.sort_by(|left,right|left.0.cmp(&right.0)); values.dedup_by(|left,right|left.0==right.0); values.truncate(MAX_SEARCH_FILES);
    Ok(values)
}

pub(crate) fn search_text(runtime:&ToolRuntime<'_>,input:&Value)->Result<Value,String>{
    let query=string_input(input,"query").ok_or("search_text requires query")?; let label=string_input(input,"path").ok_or("search_text requires path")?; let case=bool_input(input,"caseSensitive",false); let file=jail_path(runtime.cwd,&label)?;
    let mut matches=Vec::new(); let reader=BufReader::new(File::open(file).map_err(|value|value.to_string())?); let needle=if case{query.clone()}else{query.to_lowercase()};
    for (index,line) in reader.lines().enumerate(){check(runtime)?;let line=line.map_err(|value|value.to_string())?;let found=if case{line.contains(&needle)}else{line.to_lowercase().contains(&needle)};if found{matches.push(json!({"line":index+1,"text":line}));if matches.len()>=50{break;}}}
    Ok(json!({"ok":true,"path":label,"query":query,"matches":matches}))
}

fn text_files(runtime:&ToolRuntime<'_>,input:&Value)->Result<Vec<PathBuf>,String>{Ok(discover(runtime,input,false)?.into_iter().filter_map(|(path,is_dir)|if is_dir{None}else{Some(path)}).collect())}

fn parallel_literal(runtime:&ToolRuntime<'_>,files:&[PathBuf],query:&str,case:bool,max_matches:usize)->Result<Vec<(usize,usize,String)>,String>{
    let output=Mutex::new(Vec::new());
    let workers=std::thread::available_parallelism().map(usize::from).unwrap_or(2).min(8);
    let chunk=(files.len().max(1)+workers-1)/workers;
    std::thread::scope(|scope|{
        for (chunk_index,part) in files.chunks(chunk).enumerate(){
            let output=&output;let cancellation=runtime.operation.cancellation().clone();let needle=query.to_string();
            scope.spawn(move||{
                let mut collected=0usize;
                for (offset,path) in part.iter().enumerate(){
                    if collected>=max_matches{break;}
                    if cancellation.is_cancelled(){break;}
                    if fs::metadata(path).map(|meta|meta.len()>MAX_SEARCH_FILE_BYTES).unwrap_or(true){continue;}
                    let Ok(content)=fs::read_to_string(path)else{continue};if content.contains('\0'){continue;}
                    for (line_index,line)in content.lines().enumerate(){
                        let found=if case{line.contains(&needle)}else{line.to_lowercase().contains(&needle)};
                        if found&&collected<max_matches{if let Ok(mut values)=output.lock(){values.push((chunk_index*chunk+offset,line_index+1,line.to_string()));collected+=1;}}
                    }
                }
            });
        }
    });
    check(runtime)?;
    let mut values=output.into_inner().map_err(|_|"search result lock poisoned")?;
    values.sort_by(|a,b|(a.0,a.1).cmp(&(b.0,b.1)));Ok(values)
}

pub(crate) fn search_files(runtime:&ToolRuntime<'_>,input:&Value)->Result<Value,String>{
    let query=string_input(input,"query").ok_or("search_files requires query")?;
    let case=bool_input(input,"caseSensitive",false);
    let skip=u64_input(input,"skip",0).min(100_000)as usize;
    let limit=u64_input(input,"limit",500).clamp(1,500)as usize;
    let files=text_files(runtime,input)?;
    let needle=if case{query.clone()}else{query.to_lowercase()};
    let found=parallel_literal(runtime,&files,&needle,case,skip.saturating_add(limit))?;
    let matches=found.into_iter().skip(skip).take(limit).map(|(file,line,text)|json!({"path":rel_path(runtime.cwd,&files[file]),"line":line,"text":text})).collect::<Vec<_>>();
    Ok(json!({"searchedFiles":files.len(),"skip":skip,"limit":limit,"matches":matches}))
}

pub(crate) fn glob_paths(runtime:&ToolRuntime<'_>,input:&Value)->Result<Value,String>{
    let limit=u64_input(input,"limit",200).clamp(1,2000)as usize;let skip=u64_input(input,"skip",0)as usize;let raw=input.get("patterns").and_then(Value::as_array).map(|values|values.iter().filter_map(Value::as_str).map(ToOwned::to_owned).collect::<Vec<_>>()).unwrap_or_else(||vec![string_input(input,"patterns").unwrap_or_else(||"**".into())]);let patterns=raw.iter().map(|value|Pattern::new(value).map_err(|error|error.to_string())).collect::<Result<Vec<_>,_>>()?;
    let matches=discover(runtime,input,true)?.into_iter().filter_map(|(path,is_dir)|{let label=rel_path(runtime.cwd,&path);patterns.iter().any(|pattern|pattern.matches(&label)).then(||json!({"path":label,"type":if is_dir{"directory"}else{"file"}}))}).skip(skip).take(limit).collect::<Vec<_>>();Ok(json!({"skip":skip,"limit":limit,"matches":matches}))
}

pub(crate) fn grep_regex(runtime:&ToolRuntime<'_>,input:&Value)->Result<Value,String>{
    let expr=string_input(input,"expr").or_else(||string_input(input,"pattern")).ok_or("grep_regex requires expr")?;
    let case=bool_input(input,"caseSensitive",false);
    let multiline=bool_input(input,"multiline",false)||expr.contains('\n');
    let matcher=regex::RegexBuilder::new(&expr).case_insensitive(!case).dot_matches_new_line(multiline).build().map_err(|error|error.to_string())?;
    let files=text_files(runtime,input)?;
    let skip=u64_input(input,"skip",0).min(100_000)as usize;
    let limit=u64_input(input,"limit",500).clamp(1,500)as usize;
    let max_matches=skip.saturating_add(limit);
    let output=Mutex::new(Vec::<(usize,usize,String)>::new());
    let workers=std::thread::available_parallelism().map(usize::from).unwrap_or(2).min(8);
    let chunk=(files.len().max(1)+workers-1)/workers;
    std::thread::scope(|scope|{
        for (chunk_index,part)in files.chunks(chunk).enumerate(){
            let output=&output;let matcher=&matcher;let cancellation=runtime.operation.cancellation().clone();
            scope.spawn(move||{
                let mut collected=0usize;
                for(offset,path)in part.iter().enumerate(){
                    if collected>=max_matches{break;}
                    if cancellation.is_cancelled(){break;}
                    if fs::metadata(path).map(|meta|meta.len()>MAX_SEARCH_FILE_BYTES).unwrap_or(true){continue;}
                    let Ok(content)=fs::read_to_string(path)else{continue};if content.contains('\0'){continue;}
                    let file=chunk_index*chunk+offset;
                    if multiline{
                        for found in matcher.find_iter(&content){
                            let line=content[..found.start()].bytes().filter(|byte|*byte==b'\n').count()+1;
                            let text=found.as_str().split_whitespace().collect::<Vec<_>>().join(" ").chars().take(500).collect();
                            if collected>=max_matches{break;}if let Ok(mut values)=output.lock(){values.push((file,line,text));collected+=1;}
                        }
                    }else{
                        for(line_index,line)in content.lines().enumerate(){if collected>=max_matches{break;}if matcher.is_match(line){if let Ok(mut values)=output.lock(){values.push((file,line_index+1,line.to_string()));collected+=1;}}}
                    }
                }
            });
        }
    });
    check(runtime)?;
    let mut found=output.into_inner().map_err(|_|"grep result lock poisoned")?;
    found.sort_by(|a,b|(a.0,a.1).cmp(&(b.0,b.1)));
    let matches=found.into_iter().skip(skip).take(limit).map(|(file,line,text)|json!({"path":rel_path(runtime.cwd,&files[file]),"line":line,"text":text})).collect::<Vec<_>>();
    Ok(json!({"searchedFiles":files.len(),"skip":skip,"limit":limit,"matches":matches}))
}
