mod ast;
mod lsp;

use serde_json::{json, Value};
use super::{DynamicToolDescriptor, DynamicToolRegistration, ToolRuntime};

pub(crate) use ast::{ast_rewrite, ast_search};
pub(crate) use lsp::lsp;

fn descriptors() -> Vec<DynamicToolDescriptor> {
    vec![
        DynamicToolDescriptor { name:"ast_search".into(), description:"Bounded tree-sitter structural query with captured source ranges".into(), input:json!({"type":"object","required":["path","query"],"properties":{"path":{"type":"string"},"language":{"type":"string"},"query":{"type":"string"},"capture":{"type":"string"},"limit":{"type":"number"}}}), healthy:true, health:"tree-sitter parsers loaded".into() },
        DynamicToolDescriptor { name:"ast_rewrite".into(), description:"Preview, apply, or discard revision-guarded tree-sitter rewrites".into(), input:json!({"type":"object","required":["action"],"properties":{"action":{"enum":["preview","apply","discard"]},"path":{"type":"string"},"query":{"type":"string"},"capture":{"type":"string"},"replacement":{"type":"string"},"pendingId":{"type":"string"}}}), healthy:true, health:"pending rewrite registry available".into() },
        { let servers=lsp::healthy_servers(); DynamicToolDescriptor { name:"lsp".into(), description:"Persistent bounded LSP diagnostics, navigation, rename, code actions, and formatting".into(), input:json!({"type":"object","required":["action"],"properties":{"action":{"enum":["health","diagnostics","definition","references","rename","codeActions","format"]},"path":{"type":"string"},"line":{"type":"number"},"column":{"type":"number"},"newName":{"type":"string"},"server":{"type":"string"},"serverArgs":{"type":"array"}}}), healthy:!servers.is_empty(), health:if servers.is_empty(){"no language server passed its probe".into()}else{format!("probed: {}",servers.join(", "))} } },
    ]
}


fn execute(runtime:&ToolRuntime<'_>,tool:&str,input:&Value)->Option<Result<Value,String>> {
    match tool {
        "ast_search"=>Some(ast_search(runtime,input)),
        "ast_rewrite"=>Some(ast_rewrite(runtime,input)),
        "lsp"=>Some(lsp(runtime,input)),
        _=>None,
    }
}

pub(crate) fn registration()->DynamicToolRegistration { DynamicToolRegistration { owner:"jeden-language", descriptors, execute } }
