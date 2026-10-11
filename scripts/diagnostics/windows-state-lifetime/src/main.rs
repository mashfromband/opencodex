#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use serde_json::{Value, json};
use std::{
    env, fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
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
    if args.len() != 3 {
        return Err("BUN SOURCE NEW_OUTPUT".into());
    }
    let source = PathBuf::from(&args[1]);
    let output = PathBuf::from(&args[2]);
    if !source.is_absolute() || !output.is_absolute() || output.exists() {
        return Err("invalid_inputs".into());
    }
    fs::create_dir(&output)?;
    for part in ["home", "codex", "appdata", "localappdata"] {
        fs::create_dir(output.join(part))?;
    }
    let script = r#"
const {join}=await import('node:path');const {pathToFileURL}=await import('node:url');
const {fsyncSync,writeFileSync,renameSync,existsSync}=await import('node:fs');
const {heapStats}=await import('bun:jsc');
const load=path=>import(pathToFileURL(join(process.env.STATE_SOURCE,path)).href);
const state=await load('src/responses/state.ts');
const spill=await load('src/responses/spill-store.ts');
const acl=await load('src/lib/windows-secret-acl.ts');
const service=Bun.serve({hostname:'127.0.0.1',port:0,fetch(){return new Response('healthy')}});
const weak=[];let gateRelease,aclEntered=0,blockedOnce=false,fsyncCount=0;
const gate=new Promise(resolve=>{gateRelease=resolve});
const timeout=async(predicate)=>{const end=performance.now()+15000;while(!predicate()){if(performance.now()>end)throw Error('state condition deadline');await Bun.sleep(1)}};
const phase=()=>{const m=process.memoryUsage();return{external:m.external,arrayBuffers:m.arrayBuffers,heapSize:heapStats().heapSize,pending:state.pendingResponseSpillMetricsForTests(),metrics:state.responseStateMetrics()}};
const gc=async()=>{for(let i=0;i<5;i++){await Bun.sleep(10);Bun.gc(true)}};
// Only delay the existing async ACL test seam; use the actual icacls executable afterwards.
acl.setAsyncIcaclsRunnerForTests(async(args,timeoutMs)=>{
 aclEntered++;await gate;
 const child=Bun.spawn([join(process.env.SystemRoot,'System32','icacls.exe'),...args],{stdin:'ignore',stdout:'pipe',stderr:'ignore',timeout:timeoutMs,windowsHide:true});
 const textPromise=new Response(child.stdout).text();
 const exitCode=await child.exited;const stdout=await textPromise;
 return{success:exitCode===0,exitCode,timedOut:false,stdout};
});
// Deliberately slow exactly one existing fsync seam in this isolated child. This
// proves the synchronous call can block /health, not that production had slow IO.
spill.setSpillIoForTest({fsync(fd){fsyncCount++;if(!blockedOnce){blockedOnce=true;writeFileSync('blocked-marker.tmp',JSON.stringify({port:service.port}));renameSync('blocked-marker.tmp','blocked-marker.json');Bun.sleepSync(2500)}fsyncSync(fd)}});
state.setResponseStateByteCapForTests(1024);
function submit(index){const item={role:'user',content:'q'.repeat(128*1024)+index};weak.push(new WeakRef(item));state.rememberResponseState({input:[item]}, {id:'state-lifetime-'+index,status:'completed',output:[]});}
try{
 for(let i=0;i<3;i++)submit(i);
 await timeout(()=>aclEntered>0);
 await gc();const queued=phase();const liveQueued=weak.filter(ref=>ref.deref()!==undefined).length;
 gateRelease();
 await state.awaitResponseSpillPublicationTailForTests();
 const settled=phase();await gc();const collected=phase();const liveCollected=weak.filter(ref=>ref.deref()!==undefined).length;
 console.log(JSON.stringify({bun:Bun.version,queued,settled,collected,liveQueued,liveCollected,fsyncCount,blockedOnce,
  scope:'three isolated small state publications with actual ACL and filesystem APIs; gated async ACL and injected slow fsync; not full core auth/routing or a production causal proof'}));
 // Keep the listener alive until the external Rust client confirms its response.
 await timeout(()=>existsSync('health-complete'));
}finally{gateRelease();acl.setAsyncIcaclsRunnerForTests(null);spill.setSpillIoForTest(null);await service.stop(true)}
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
        .env("OPENCODEX_HOME", output.join("home"))
        .env("CODEX_HOME", output.join("codex"))
        .env("CODEX_CI", "1")
        .env("STATE_SOURCE", &source)
        .current_dir(&output)
        .args(["--no-env-file", "--no-orphans", "--eval", script])
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::from(fs::File::create(output.join("result.json"))?))
        .stderr(Stdio::from(fs::File::create(output.join("stderr.txt"))?));
    let mut child = Owned(command.spawn()?);
    let deadline = Instant::now() + Duration::from_secs(60);
    let marker = output.join("blocked-marker.json");
    while !marker.is_file() {
        if child.0.try_wait()?.is_some() {
            return Err("child_ended_before_marker".into());
        }
        if Instant::now() >= deadline {
            return Err("marker_deadline".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let value: Value = serde_json::from_slice(&fs::read(marker)?)?;
    let port: u16 = value["port"]
        .as_u64()
        .and_then(|n| u16::try_from(n).ok())
        .filter(|p| *p != 0 && *p != 10100)
        .ok_or("invalid_port")?;
    let start = Instant::now();
    let budget = Duration::from_secs(5);
    let mut socket = TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), budget)?;
    socket.set_write_timeout(Some(budget))?;
    socket.set_read_timeout(Some(budget))?;
    socket.write_all(b"GET /health HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")?;
    let mut reply = Vec::new();
    socket.take(16385).read_to_end(&mut reply)?;
    let elapsed = start.elapsed().as_millis();
    if reply.len() > 16384 || !reply.starts_with(b"HTTP/1.1 200") || !reply.ends_with(b"healthy") {
        return Err("health_contract".into());
    }
    fs::write(output.join("health-complete"), b"complete")?;
    let exit = loop {
        if let Some(exit) = child.0.try_wait()? {
            break exit;
        }
        if Instant::now() >= deadline {
            return Err("child_deadline".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if !exit.success() {
        return Err("child_failed".into());
    }
    let result: Value = serde_json::from_slice(&fs::read(output.join("result.json"))?)?;
    let evidence = json!({"isolated_child_exit":exit.code(),"health_ms":elapsed,"result":result});
    fs::write(
        output.join("verified.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    if result["liveQueued"] != 3
        || result["liveCollected"] != 0
        || result["queued"]["pending"]["count"] != 3
        || result["collected"]["pending"]["count"] != 0
        || result["collected"]["pending"]["bytes"] != 0
        || result["collected"]["metrics"]["spillStubCount"] != 3
        || result["collected"]["metrics"]["spillWriteFailures"] != 0
        || result["blockedOnce"] != true
        || result["fsyncCount"].as_u64().unwrap_or(0) < 3
        || elapsed < 2000
    {
        return Err("state_lifetime_contract".into());
    }
    println!("{evidence}");
    Ok(())
}
