//! The two programs fed to an interpreter on startup so it speaks the framed
//! protocol the rest of the kernel expects.
//!
//! Split out of `runtime_ops/kernel.rs`, which had grown past the module line
//! cap.

pub(super) const PYTHON_BOOTSTRAP: &str = r#"import sys,json,traceback,base64
G={'__name__':'__main__'}
def emit(i,s,d,m=None):
 for p in [d[x:x+3072] for x in range(0,len(d),3072)] or ['']:
  print(json.dumps({'id':i,'type':'chunk','stream':s,'data':p,'mime':m}),file=sys.__stdout__,flush=True)
class W:
 def __init__(self,i,s): self.i=i; self.s=s
 def write(self,d):
  if d: emit(self.i,self.s,str(d))
 def flush(self): pass
for line in sys.stdin:
 try:
  r=json.loads(line); i=r['id']; code=r['code']; oldo,olde=sys.stdout,sys.stderr; sys.stdout,sys.stderr=W(i,'stdout'),W(i,'stderr')
  try:
   try: v=eval(compile(code,'<jeden>','eval'),G,G)
   except SyntaxError: exec(compile(code,'<jeden>','exec'),G,G); v=None
   if v is not None:
    if hasattr(v,'_repr_png_'):
     p=v._repr_png_(); emit(i,'display',base64.b64encode(p).decode() if isinstance(p,bytes) else str(p),'image/png;base64')
    elif hasattr(v,'_repr_html_'): emit(i,'display',str(v._repr_html_()),'text/html')
    elif hasattr(v,'_repr_json_'): emit(i,'display',json.dumps(v._repr_json_()),'application/json')
    else: emit(i,'display',repr(v),'text/plain')
   done={'id':i,'type':'done','ok':True}
  except BaseException as e:
   traceback.print_exc(); done={'id':i,'type':'done','ok':False,'error':str(e)}
  finally: sys.stdout,sys.stderr=oldo,olde
  print(json.dumps(done),flush=True)
 except BaseException as e: print(json.dumps({'id':0,'type':'done','ok':False,'error':str(e)}),flush=True)
"#;

pub(super) const JAVASCRIPT_BOOTSTRAP: &str = r#"const vm=require('vm'),readline=require('readline'),util=require('util');let id=0;function emit(s,d,m){d=String(d);for(let x=0;x<d.length;x+=3072)process.stdout.write(JSON.stringify({id,type:'chunk',stream:s,data:d.slice(x,x+3072),mime:m})+'\n')}const context=vm.createContext({});context.console={log:(...a)=>emit('stdout',a.map(x=>typeof x==='string'?x:util.inspect(x)).join(' ')+'\n'),error:(...a)=>emit('stderr',a.map(x=>typeof x==='string'?x:util.inspect(x)).join(' ')+'\n')};readline.createInterface({input:process.stdin}).on('line',line=>{try{const r=JSON.parse(line);id=r.id;let v=vm.runInContext(r.code,context,{timeout:Math.max(1,r.timeoutMs)});if(v!==undefined)emit('display',typeof v==='string'?v:util.inspect(v,{depth:4}),'text/plain');process.stdout.write(JSON.stringify({id,type:'done',ok:true})+'\n')}catch(e){emit('stderr',e.stack||String(e));process.stdout.write(JSON.stringify({id,type:'done',ok:false,error:String(e.message||e)})+'\n')}});
"#;
