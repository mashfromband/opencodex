#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use serde_json::{Value, json};
use std::{
    env, fs,
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() != 4 {
        return Err(
            "BUN INSTALLED_SOURCE NEW_OUTPUT stock|patched|header|header-patched|inbound|handler|http"
                .into(),
        );
    }
    let source = PathBuf::from(&args[1]);
    let output = PathBuf::from(&args[2]);
    let mode = args[3].to_str().ok_or("invalid_mode")?;
    if !source.is_absolute()
        || !output.is_absolute()
        || output.exists()
        || !matches!(
            mode,
            "stock" | "patched" | "header" | "header-patched" | "inbound" | "handler" | "http"
        )
    {
        return Err("invalid_inputs".into());
    }
    fs::create_dir(&output)?;
    for part in ["home", "codex", "appdata", "localappdata"] {
        fs::create_dir(output.join(part))?;
    }
    // Copy only the existing public relay module, resolving its two imports back
    // to the same installed source. No writes to that source or runtime.
    let mut relay = fs::read_to_string(source.join("src/server/relay-eager.ts"))?;
    let prefix = source.to_string_lossy().replace('\\', "/");
    for (literal, target) in [
        ("\"./relay\"", "src/server/relay.ts"),
        (
            "\"./sse-payload-rewrite\"",
            "src/server/sse-payload-rewrite.ts",
        ),
    ] {
        if relay.matches(literal).count() != 1 {
            return Err("unexpected_relay_imports".into());
        }
        relay = relay.replace(
            literal,
            &serde_json::to_string(&format!("{prefix}/{target}"))?,
        );
    }
    if mode == "patched" || mode == "header-patched" {
        let anchor = "    } finally {\n      // Release any retained rewrite-buffer bytes on every teardown path";
        let crlf = anchor.replace('\n', "\r\n");
        let anchor = if relay.contains(anchor) {
            anchor
        } else if relay.contains(&crlf) {
            &crlf
        } else {
            return Err("producer_finally_anchor_missing".into());
        };
        if relay.matches(anchor).count() != 1 {
            return Err("ambiguous_producer_anchor".into());
        }
        relay=relay.replace(anchor,"    } finally {\n      upstream.signal.removeEventListener(\"abort\", wakeUp);\n      upstream.signal.removeEventListener(\"abort\", wakeParkedRead);\n      // Release any retained rewrite-buffer bytes on every teardown path");
    }
    let relay_path = output.join("relay-eager.ts");
    fs::write(&relay_path, relay)?;
    let handler_path = output.join("websocket-handler.ts");
    if mode == "handler" {
        let mut handler = fs::read_to_string(source.join("src/server/index/websocket-handler.ts"))?;
        let dispatcher = "import {\n  disableResponsesRequestTimeout,\n  handleResponses,\n  handleResponsesCompact,\n} from \"../responses\";";
        handler = handler.replace("\r\n", "\n");
        if handler.matches(dispatcher).count() != 1 {
            return Err("handler_dispatch_anchor".into());
        }
        handler = handler.replace(dispatcher, "const {disableResponsesRequestTimeout,handleResponses,handleResponsesCompact}=globalThis.__lifetimeDispatch;");
        let imports: Vec<String> = handler
            .split(" from \"")
            .skip(1)
            .filter_map(|part| part.split('"').next())
            .filter(|part| part.starts_with('.'))
            .map(str::to_owned)
            .collect();
        for literal in imports {
            let mut resolved = source.join("src/server/index");
            for component in literal.split('/') {
                match component {
                    "." => {}
                    ".." => {
                        resolved.pop();
                    }
                    other => resolved.push(other),
                }
            }
            if resolved.with_extension("ts").is_file() {
                resolved.set_extension("ts");
            } else {
                resolved.push("index.ts");
            }
            if !resolved.is_file() {
                return Err("handler_import_missing".into());
            }
            handler = handler.replace(
                &format!(" from \"{literal}\""),
                &format!(
                    " from {}",
                    serde_json::to_string(&resolved.to_string_lossy().replace('\\', "/"))?
                ),
            );
        }
        for (anchor, extra) in [
            (
                "const turnAbort = new AbortController();",
                "globalThis.__lifetimeHandlerRefs.push({turnAbort:new WeakRef(turnAbort),frame:new WeakRef(frame)});",
            ),
            (
                "turnAdmissionLease.bindAbortController(turnAbort);",
                "globalThis.__lifetimeHandlerRefs.at(-1).lease=new WeakRef(turnAdmissionLease);",
            ),
            (
                "let logged = false;",
                "globalThis.__lifetimeHandlerRefs.at(-1).logCtx=new WeakRef(logCtx);",
            ),
        ] {
            if handler.matches(anchor).count() != 1 {
                return Err("handler_instrumentation_anchor".into());
            }
            handler = handler.replace(anchor, &format!("{anchor}\n{extra}"));
        }
        fs::write(&handler_path, handler)?;
    }
    let script = r#"
const {pathToFileURL}=await import('node:url'); const {join}=await import('node:path');
const {getEventListeners}=await import('node:events');
const base=process.env.LIFETIME_SOURCE;
const {CodexWsPool}=await import(pathToFileURL(join(base,'src/server/responses/codex-ws-pool.ts')).href);
const {codexWsExchange}=await import(pathToFileURL(join(base,'src/server/responses/codex-ws-exchange.ts')).href);
const {fetchWithHeaderTimeout}=await import(pathToFileURL(join(base,'src/server/responses/fetch-helpers.ts')).href);
const {linkAbortSignal}=await import(pathToFileURL(join(base,'src/server/responses/core-lifetime.ts')).href);
const {prepareCodexWsRequest}=await import(pathToFileURL(join(base,'src/server/responses/codex-ws-request.ts')).href);
const {relaySseEagerBounded}=await import(pathToFileURL(process.env.LIFETIME_RELAY).href);
const {sendResponseToWebSocket}=await import(pathToFileURL(join(base,'src/server/ws-bridge.ts')).href);
const {heapStats}=await import('bun:jsc');
const weak=[]; let creates=0,terminalCount=0,inspectedDone=0,fallbacks=0;
const handlerMode=process.env.LIFETIME_MODE==='handler';
const httpMode=process.env.LIFETIME_MODE==='http';
const inboundMode=process.env.LIFETIME_MODE==='inbound'||handlerMode;
const timeout=async(predicate)=>{const end=performance.now()+5000;while(!predicate()){if(performance.now()>end)throw Error('inbound condition deadline');await Bun.sleep(1)}};
let peer,inboundClient,inboundServer,receivedTerminal=0,receivedDeltaBytes=0,handlerCompleted=0;
let lifecycle,registry;
let httpServer,httpCreated=0;const httpWeak=[];
if(httpMode){
 httpServer=Bun.serve({hostname:'127.0.0.1',port:0,async fetch(req){
  const index=httpCreated++;httpWeak.push({req:new WeakRef(req),signal:new WeakRef(req.signal)});
  const body=await req.json();if(body.input.length!==128*1024+1)throw Error('inbound HTTP body contract');
  return await one(index,req.signal);
 }});
}
if(inboundMode){
 let handler;
 if(handlerMode){
  globalThis.fetch=async()=>{fallbacks++;throw Error('network fetch disabled')};
  globalThis.__lifetimeHandlerRefs=[];
  globalThis.__lifetimeDispatch={handleResponses:async(req,config,logCtx,options)=>{
   globalThis.__lifetimeHandlerRefs.at(-1).req=new WeakRef(req);
   const result=await one(handlerCompleted,options.abortSignal);
   handlerCompleted++;return result;
  }};
  const {createWebsocketHandler}=await import(pathToFileURL(process.env.LIFETIME_HANDLER).href);
  lifecycle=await import(pathToFileURL(join(base,'src/server/lifecycle.ts')).href);
  registry=await import(pathToFileURL(join(base,'src/codex/websocket-registry.ts')).href);
  handler=createWebsocketHandler({config:{maxInboundBodyBytes:1048576},deps:{}});
 }
 inboundServer=Bun.serve({hostname:'127.0.0.1',port:0,fetch(req,server){const data={turnId:0};if(handlerMode){data.admissionLease=registry.tryReserveCodexWebSocket();data.authContext={kind:'pool',accountId:'synthetic-lifetime'};}if(server.upgrade(req,{data}))return;data.admissionLease?.release();return new Response('upgrade required',{status:400})},websocket:handlerMode?{...handler,open(ws){peer=ws;handler.open(ws)}}:{open(ws){peer=ws},message(){}}});
 inboundClient=new WebSocket('ws://127.0.0.1:'+inboundServer.port);
 inboundClient.addEventListener('message',event=>{const frame=JSON.parse(String(event.data));if(frame.type==='response.completed')receivedTerminal++;if(frame.type==='response.output_text.delta')receivedDeltaBytes+=Buffer.byteLength(frame.delta)});
 await timeout(()=>peer&&inboundClient.readyState===WebSocket.OPEN);
}
const upstream=Bun.serve({hostname:'127.0.0.1',port:0,fetch(req,server){if(server.upgrade(req))return;return new Response('upgrade required',{status:400});},
 websocket:{message(ws,text){const frame=JSON.parse(String(text)); if(frame.type!=='response.create')throw Error('unexpected frame');
  const id='lifetime-'+(++creates),item='item-'+creates;
  for(const event of [{type:'response.created',response:{id,status:'in_progress'}},
   {type:'response.output_item.added',response_id:id,item:{id:item,type:'message',role:'assistant',content:[]}},
   {type:'response.output_text.delta',response_id:id,item_id:item,output_index:0,content_index:0,delta:'x'.repeat(256*1024)},
   {type:'response.completed',response:{id,status:'completed',output:[],usage:{input_tokens:1,output_tokens:1}}}])ws.send(JSON.stringify(event));}}});
const url='ws://127.0.0.1:'+upstream.port;
const pool=new CodexWsPool({idleMs:60000,maxAgeMs:60000,maxSessions:1});
const identity={key:'synthetic-lifetime',scope:'synthetic-lifetime'};
const phase=()=>{const m=process.memoryUsage();return{external:m.external,heapUsed:m.heapUsed,arrayBuffers:m.arrayBuffers,objectCount:heapStats().objectCount}};
const baseline=phase();
async function one(index,handlerSignal){
 const controller=new AbortController();const signal=controller.signal;
 const unlink=handlerSignal?linkAbortSignal(controller,handlerSignal):()=>{};
 const frame={model:'synthetic-model',input:'q'.repeat(128*1024)+index,stream:true};
 const init={method:'POST',body:JSON.stringify(frame),signal:handlerSignal??signal};
 const prepared=prepareCodexWsRequest('https://chatgpt.com/backend-api/codex/responses',init);
 if(!prepared)throw Error('preparation refused');
 const session=pool.acquire(identity,url,{});if(!session)throw Error('pool acquire failed');
 let wireSignal;
 const executor=async(input,wireInit)=>{wireSignal=new WeakRef(wireInit.signal);return codexWsExchange({session,url,init:wireInit,prepared,sseFallback:async()=>{fallbacks++;throw Error('unexpected fallback')}})};
 const response=(process.env.LIFETIME_MODE.startsWith('header')||inboundMode||httpMode)
  ?await fetchWithHeaderTimeout('https://chatgpt.com/backend-api/codex/responses',init,handlerSignal??signal,10000,true,executor)
  :await executor(url,init);
 const body=response.body;
 const original=body.getReader;let readerRef;
 body.getReader=function(...args){const reader=original.apply(this,args);readerRef=new WeakRef(reader);return reader};
 const relayed=relaySseEagerBounded(body,controller,{inspectChunk(){},finishInspection(){},sawTerminal(){return true},onSynthetic(){throw Error('synthetic terminal')},onClientCancel(){throw Error('cancel')},onDone(){inspectedDone++;unlink()}});
 let bridgeResponse,bridgeReader;
 if(handlerMode||httpMode){
  const original=relayed.getReader;
  relayed.getReader=function(...args){const reader=original.apply(this,args);if(handlerMode)globalThis.__lifetimeHandlerRefs[index].bridgeReader=new WeakRef(reader);return reader};
  bridgeResponse=new Response(relayed,{headers:{'content-type':'text/event-stream'}});
 }else if(inboundMode){
  const original=relayed.getReader;
  relayed.getReader=function(...args){const reader=original.apply(this,args);bridgeReader=new WeakRef(reader);return reader};
  bridgeResponse=new Response(relayed,{headers:{'content-type':'text/event-stream'}});
  const turnId=++peer.data.turnId;
  await sendResponseToWebSocket(peer,bridgeResponse,()=>peer.data.turnId===turnId);
  await timeout(()=>receivedTerminal===index+1);
  if(peer.data.cancel!==undefined||peer.readyState!==1||inboundClient.readyState!==WebSocket.OPEN)throw Error('inbound lifetime contract');
 }else{
  const bytes=await new Response(relayed).text();
  if(!bytes.includes('response.completed')||!bytes.includes('x'.repeat(256*1024)))throw Error('incomplete response');
 }
 if(!handlerMode&&!httpMode)terminalCount++;
 weak.push({controller:new WeakRef(controller),signal:new WeakRef(signal),wireSignal,init:new WeakRef(init),response:new WeakRef(response),body:new WeakRef(body),reader:readerRef,request:new WeakRef(frame),prepared:new WeakRef(prepared),relayed:new WeakRef(relayed),bridgeResponse:bridgeResponse?new WeakRef(bridgeResponse):undefined,bridgeReader,
  listeners:getEventListeners(signal,'abort').length,aborted:signal.aborted});
 if(handlerMode||httpMode)return bridgeResponse;
}
try {
 for(let i=0;i<6;i++){
  if(handlerMode){
   inboundClient.send(JSON.stringify({type:'response.create',model:'synthetic-model',input:'q'.repeat(128*1024)+i}));
   await timeout(()=>receivedTerminal===i+1&&peer.data.cancel===undefined&&lifecycle.getActiveTurnCount()===0);
   terminalCount++;
  }else if(httpMode){
   const response=await fetch('http://127.0.0.1:'+httpServer.port+'/v1/responses',{method:'POST',body:JSON.stringify({input:'q'.repeat(128*1024)+i})});
   const text=await response.text();if(!text.includes('response.completed')||!text.includes('x'.repeat(256*1024)))throw Error('HTTP response incomplete');
   terminalCount++;
  }else await one(i);
 }
 // Keep the pool/socket alive, but none of a completed exchange's owners.
 const before=phase(),poolBefore=pool.snapshot();
 for(let i=0;i<5;i++){await Bun.sleep(20);Bun.gc(true)}
 const after=phase();
 const live=Object.fromEntries(['controller','signal','wireSignal','init','response','body','reader','request','prepared','relayed','bridgeResponse','bridgeReader'].map(kind=>[kind,weak.filter(row=>row[kind]?.deref()!==undefined).length]));
 const handlerLive=handlerMode?Object.fromEntries(['turnAbort','frame','lease','logCtx','req','bridgeReader'].map(kind=>[kind,globalThis.__lifetimeHandlerRefs.filter(row=>row[kind]?.deref()!==undefined).length])):null;
 const httpLive=httpMode?Object.fromEntries(['req','signal'].map(kind=>[kind,httpWeak.filter(row=>row[kind].deref()!==undefined).length])):null;
 console.log(JSON.stringify({bun:Bun.version,creates,terminalCount,inspectedDone,fallbacks,baseline,before,after,poolBefore,poolAfter:pool.snapshot(),
  listenerCounts:weak.map(row=>row.listeners),aborted:weak.map(row=>row.aborted),live,receivedTerminal,receivedDeltaBytes,
  inboundOpen:inboundMode?peer.readyState===1&&inboundClient.readyState===WebSocket.OPEN:null,
  inboundCancelCleared:inboundMode?peer.data.cancel===undefined:null,
  handlerCompleted,handlerLive,handlerActive:handlerMode?lifecycle.getActiveTurnCount():null,
  handlerSocketActive:handlerMode?registry.codexWebSocketAdmissionMetrics().active:null,
  httpCreated,httpLive,httpPending:httpMode?httpServer.pendingRequests:null,
  scope:'isolated native exchanges, optional real inbound HTTP Request or WebSocket bridge/handler; excludes full handleResponses routing/auth/state; forced GC only in owned process'}));
} finally {pool.dispose();inboundClient?.close();await inboundServer?.stop(true);await httpServer?.stop(true);await upstream.stop(true)}
"#;
    let system = env::var_os("SystemRoot").ok_or("SystemRoot_missing")?;
    let mut command = Command::new(&args[0]);
    command
        .env_clear()
        .env("SystemRoot", &system)
        .env("WINDIR", &system)
        .env("PATH", PathBuf::from(&system).join("System32"))
        .env("HOME", &output)
        .env("USERPROFILE", &output)
        .env("APPDATA", output.join("appdata"))
        .env("LOCALAPPDATA", output.join("localappdata"))
        .env("TEMP", &output)
        .env("TMP", &output)
        .env("CODEX_HOME", output.join("codex"))
        .env("OPENCODEX_HOME", output.join("home"))
        .env("CODEX_CI", "1")
        .env("LIFETIME_SOURCE", &source)
        .env("LIFETIME_RELAY", &relay_path)
        .env("LIFETIME_HANDLER", &handler_path)
        .env("LIFETIME_MODE", mode)
        .current_dir(&output)
        .args(["--no-env-file", "--no-orphans", "--eval", script])
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::from(fs::File::create(output.join("result.json"))?))
        .stderr(Stdio::from(fs::File::create(output.join("stderr.txt"))?));
    let mut child = Owned(command.spawn()?);
    let deadline = Instant::now() + Duration::from_secs(60);
    let exit = loop {
        if let Some(exit) = child.0.try_wait()? {
            break exit;
        }
        if Instant::now() >= deadline {
            return Err("child_deadline".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if !exit.success() {
        return Err("isolated_child_failed".into());
    }
    let bytes = fs::read(output.join("result.json"))?;
    if bytes.len() > 16384 {
        return Err("output_limit".into());
    }
    let result: Value = serde_json::from_slice(&bytes)?;
    for field in ["creates", "terminalCount", "inspectedDone"] {
        if result[field] != 6 {
            return Err("incomplete_exchanges".into());
        }
    }
    if result["fallbacks"] != 0
        || result["poolAfter"]["size"] != 1
        || result["poolAfter"]["active"] != 0
    {
        return Err("pool_contract".into());
    }
    for kind in [
        "controller",
        "signal",
        "wireSignal",
        "init",
        "response",
        "body",
        "reader",
        "request",
        "prepared",
        "relayed",
    ] {
        if result["live"][kind] != 0 {
            println!(
                "{}",
                json!({"mode":mode,"isolated_child_exit":exit.code(),"result":result})
            );
            return Err("completed_exchange_owner_not_collected".into());
        }
    }
    if mode == "inbound" || mode == "handler" {
        if result["receivedTerminal"] != 6
            || result["receivedDeltaBytes"] != 6 * 256 * 1024
            || result["inboundOpen"] != true
            || result["inboundCancelCleared"] != true
        {
            return Err("inbound_delivery_contract".into());
        }
        for kind in ["bridgeResponse", "bridgeReader"] {
            if result["live"][kind] != 0 {
                println!(
                    "{}",
                    json!({"mode":mode,"isolated_child_exit":exit.code(),"result":result})
                );
                return Err("completed_inbound_owner_not_collected".into());
            }
        }
    }
    if mode == "handler" {
        if result["handlerCompleted"] != 6
            || result["handlerActive"] != 0
            || result["handlerSocketActive"] != 1
        {
            return Err("handler_lifetime_contract".into());
        }
        for kind in [
            "turnAbort",
            "frame",
            "lease",
            "logCtx",
            "req",
            "bridgeReader",
        ] {
            if result["handlerLive"][kind] != 0 {
                println!(
                    "{}",
                    json!({"mode":mode,"isolated_child_exit":exit.code(),"result":result})
                );
                return Err("completed_handler_owner_not_collected".into());
            }
        }
    }
    if mode == "http" {
        if result["httpCreated"] != 6 || result["httpPending"] != 0 {
            return Err("http_delivery_contract".into());
        }
        for kind in ["req", "signal"] {
            if result["httpLive"][kind] != 0 {
                println!(
                    "{}",
                    json!({"mode":mode,"isolated_child_exit":exit.code(),"result":result})
                );
                return Err("completed_http_owner_not_collected".into());
            }
        }
    }
    println!(
        "{}",
        json!({"mode":mode,"isolated_child_exit":exit.code(),"result":result})
    );
    Ok(())
}
