#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod health;
mod owned;
use serde_json::{Value, json};
use std::{
    env, fs,
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() == 2 && args[0] == "--write-bun-test" {
        use std::io::Write;
        let output = PathBuf::from(&args[1]);
        if !output.is_absolute() {
            return Err("absolute_test_output_required".into());
        }
        let emitter = serde_json::to_string(&env::current_exe()?.to_string_lossy())?;
        let test = r#"
import {test,expect} from 'bun:test';
import {gcTick} from '../test/harness.ts';
const emitter=__EMITTER__;
test.each(['stdout','stderr'])('completed subprocess is collectable after late %s getter',async(channel)=>{
 const weak=[];
 const results=await Promise.all(Array.from({length:6},async()=>{
  const proc=Bun.spawn([emitter,channel==='stderr'?'--emit-stderr':'--emit'],{
   stdin:'ignore',stdout:channel==='stdout'?'pipe':'ignore',stderr:channel==='stderr'?'pipe':'ignore',windowsHide:true
  });
  weak.push(new WeakRef(proc));
  try{
   const exitCode=await proc.exited;
   const text=await new Response(channel==='stdout'?proc.stdout:proc.stderr).text();
   expect(text).toBe('synthetic subprocess output');
   expect(exitCode).toBe(0);
   return exitCode;
  }finally{if(proc.exitCode===null){proc.kill();await proc.exited}}
 }));
 expect(results).toEqual([0,0,0,0,0,0]);
 expect(weak).toHaveLength(6);
 const deadline=performance.now()+1000;
 let live=6;
 do{await gcTick();live=weak.filter(ref=>ref.deref()!==undefined).length}while(live&&performance.now()<deadline);
 expect(live).toBe(0);
});
"#;
        if test.matches("__EMITTER__").count() != 1 {
            return Err("test_emitter_anchor".into());
        }
        fs::File::create_new(output)?
            .write_all(test.replace("__EMITTER__", &emitter).as_bytes())?;
        return Ok(());
    }
    if args.len() == 1 && (args[0] == "--emit" || args[0] == "--emit-stderr") {
        use std::io::Write;
        if args[0] == "--emit-stderr" {
            std::io::stderr().write_all(b"synthetic subprocess output")?;
            std::io::stderr().flush()?;
        } else {
            std::io::stdout().write_all(b"synthetic subprocess output")?;
            std::io::stdout().flush()?;
        }
        return Ok(());
    }
    let burst = args.len() == 4 && args[3] == "burst";
    let early = args.len() == 4 && args[3] == "parallel-early";
    let ignore = args.len() == 4 && args[3] == "parallel-ignore";
    let native_refresh = args.len() == 4
        && (args[3] == "native-late-refresh" || args[3] == "native-stderr-late-refresh");
    let native_stderr = args.len() == 4
        && (args[3] == "native-stderr-late"
            || args[3] == "native-stderr-early"
            || args[3] == "native-stderr-late-refresh");
    let native = args.len() == 4
        && (args[3] == "native-late"
            || args[3] == "native-early"
            || native_stderr
            || native_refresh);
    let native_early = native && (args[3] == "native-early" || args[3] == "native-stderr-early");
    let parallel = args.len() == 4 && (args[3] == "parallel" || early || ignore || native);
    if args.len() != 3 && !burst && !parallel {
        return Err(
            "BUN INSTALLED_SOURCE NEW_OUTPUT [parallel|burst|parallel-early|parallel-ignore]"
                .into(),
        );
    }
    let bun = PathBuf::from(&args[0]);
    let source = PathBuf::from(&args[1]);
    let output = PathBuf::from(&args[2]);
    if !bun.is_absolute() || !source.is_absolute() || !output.is_absolute() || output.exists() {
        return Err("invalid_inputs".into());
    }
    fs::create_dir(&output)?;
    for part in ["home", "codex", "appdata", "localappdata"] {
        fs::create_dir(output.join(part))?;
    }
    // The child cannot import modules or create descendants until the Rust
    // supervisor has attached it to its own kill-on-close job and releases it.
    let script = r#"
const {existsSync,writeFileSync,renameSync}=await import('node:fs');
while(!existsSync('run-marker'))await Bun.sleep(1);
const {join,basename}=await import('node:path');const {pathToFileURL}=await import('node:url');
const {heapStats}=await import('bun:jsc');
const load=path=>import(pathToFileURL(join(process.env.ACL_SOURCE,path)).href);
const nativeMode=process.env.ACL_NATIVE_MODE;
const nativeStderr=process.env.ACL_NATIVE_STDERR==='1';
const nativeRefresh=process.env.ACL_NATIVE_REFRESH==='1';
const acl=nativeMode==='none'?await load('src/lib/windows-secret-acl.ts'):null;
const weak=[];const spawns=[];const original=Bun.spawn;
// Return the exact subprocess. Do not touch stdout, exited, or any proc getter:
// the default runner must keep its original exit-then-read order.
Bun.spawn=function(...args){
 const start=performance.now();const proc=Reflect.apply(original,Bun,args);
 weak.push(new WeakRef(proc));
 const executable=basename(Array.isArray(args[0])?args[0][0]:'').toLowerCase();
 const option=Array.isArray(args[0])?args[0][2]:'';
 const step=['/grant:r','/inheritance:r','/remove:g','/findsid'].includes(option)?option:'other';
 spawns.push({kind:executable==='icacls.exe'?'acl':executable==='powershell.exe'?'principal':'other',step,spawn_ms:performance.now()-start});
 return proc;
};
const classes=['Subprocess','ReadableStream','Promise','ArrayBuffer','Uint8Array'];
const phase=()=>{
 const h=heapStats(),m=process.memoryUsage();const counts={};
 for(const name of classes)counts[name]=Object.hasOwn(h.objectTypeCounts,name)?h.objectTypeCounts[name]:null;
 return{external:m.external,arrayBuffers:m.arrayBuffers,heapUsed:m.heapUsed,heapSize:h.heapSize,objectCount:h.objectCount,classes:counts};
};
const collect=async()=>{for(let i=0;i<5;i++){await Bun.sleep(10);Bun.gc(true)}};
const runnerMode=process.env.ACL_RUNNER_MODE;
if(runnerMode!=='stock'&&nativeMode==='none'){
 const {waitForSubprocessExit}=await load('src/lib/bounded-subprocess.ts');
 const {resolveTrustedWindowsIcaclsExe}=await load('src/lib/windows-elevation.ts');
 // Explicit timing controls only. The stock path never replaces its runner.
 // Both controls keep real identity lookup, real ACL commands and exit reaping.
 acl.setAsyncIcaclsRunnerForTests(async(args,timeoutMs)=>{
  const proc=Bun.spawn([resolveTrustedWindowsIcaclsExe(),...args],{stdin:'ignore',stdout:runnerMode==='ignore'?'ignore':'pipe',stderr:'ignore',windowsHide:true});
  const reading=runnerMode==='early'?new Response(proc.stdout).text().catch(()=>''):null;
  const {exitCode,timedOut}=await waitForSubprocessExit(proc,timeoutMs);
  const stdout=!timedOut&&reading?await reading:'';
  return{success:!timedOut&&exitCode===0,exitCode:timedOut?null:exitCode,timedOut,stdout};
 });
}
const before=phase();let completed=0;
const burst=process.env.ACL_BURST==='1';let service=null;
const parallel=process.env.ACL_PARALLEL==='1';
if(burst){
 service=Bun.serve({hostname:'127.0.0.1',port:0,fetch(){return Response.json({pid:process.pid,status:'ok'})}});
 writeFileSync('listen-marker.tmp',JSON.stringify({port:service.port,pid:process.pid}));
 renameSync('listen-marker.tmp','listen-marker.json');
 while(!existsSync('begin-acl-marker'))await Bun.sleep(1);
}
try{
 const harden=async(i)=>{
  if(nativeMode!=='none'){
   const proc=Bun.spawn([process.env.ACL_EMITTER,nativeStderr?'--emit-stderr':'--emit'],{stdin:'ignore',stdout:nativeStderr?'ignore':'pipe',stderr:nativeStderr?'pipe':'ignore',windowsHide:true});
   const reading=nativeMode==='early'?new Response(nativeStderr?proc.stderr:proc.stdout).text():null;
   const exit=await proc.exited;
   const stdout=reading?await reading:await new Response(nativeStderr?proc.stderr:proc.stdout).text();
   if(exit!==0||stdout!=='synthetic subprocess output')throw Error('native_output_contract');
   // The public unref method recomputes pending activity, after actual exit
   // and full output consumption. It abandons no child in this control.
   if(nativeRefresh)proc.unref();
   completed++;return;
  }
  const path=join(process.cwd(),'synthetic-'+i+'.txt');writeFileSync(path,'synthetic small ACL input');
  const result=await acl.hardenSecretPathAsync(path,{required:true,deadlineMs:15000});
  if(result.ok!==true)throw Error('acl_not_ok');completed++;
 };
 if(burst||parallel){
  const results=await Promise.allSettled(Array.from({length:6},(_,i)=>harden(i)));
  if(results.some(result=>result.status!=='fulfilled'))throw Error('acl_burst_failed');
 }else{for(let i=0;i<6;i++)await harden(i)}
}finally{Bun.spawn=original;if(runnerMode!=='stock')acl.setAsyncIcaclsRunnerForTests(null)}
const settled=phase();
if(burst){
 writeFileSync('acl-settled-marker','done');
 while(!existsSync('health-complete-marker'))await Bun.sleep(1);
 await service.stop(true);
}
const liveIndexes=()=>weak.flatMap((ref,index)=>ref.deref()===undefined?[]:[index]);
await collect();const firstCollected=phase();const firstLive=liveIndexes();
// Finalizers may require later event-loop turns. Preserve both observations;
// do not call an immediate survivor a persistent leak.
await Bun.sleep(1000);await collect();const collected=phase();const finalLive=liveIndexes();
const live=finalLive.length;
console.log(JSON.stringify({bun:Bun.version,runnerMode,nativeMode,nativeStderr,nativeRefresh,burst,parallel,completed,spawnCount:weak.length,spawns,before,settled,firstCollected,firstLive,collected,finalLive,live,
 scope:nativeMode!=='none'?'six owned Rust emitters through only Bun.spawn/exited/Response/GC; no opencodex imports or ACL; not production total retention proof':runnerMode==='stock'?'six small files through unmodified default async ACL runner; same proc returned without getters; isolated child GC only, not production total retention proof':'six small files with explicit runner timing control; real ACL, identity and exit reaping; not default-runner or production total retention proof'}));
"#;
    let system = env::var_os("SystemRoot").ok_or("SystemRoot_missing")?;
    let mut command = Command::new(bun);
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
        .env("ACL_SOURCE", &source)
        .env("ACL_BURST", if burst { "1" } else { "0" })
        .env("ACL_PARALLEL", if parallel { "1" } else { "0" })
        .env(
            "ACL_NATIVE_MODE",
            if native_early {
                "early"
            } else if native {
                "late"
            } else {
                "none"
            },
        )
        .env("ACL_EMITTER", env::current_exe()?)
        .env("ACL_NATIVE_STDERR", if native_stderr { "1" } else { "0" })
        .env("ACL_NATIVE_REFRESH", if native_refresh { "1" } else { "0" })
        .env(
            "ACL_RUNNER_MODE",
            if early {
                "early"
            } else if ignore {
                "ignore"
            } else {
                "stock"
            },
        )
        .current_dir(&output)
        .args(["--no-env-file", "--no-orphans", "--eval", script])
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::from(fs::File::create_new(
            output.join("result.json"),
        )?))
        .stderr(Stdio::from(fs::File::create_new(
            output.join("stderr.txt"),
        )?));
    // Declare child before job so the job closes (terminating descendants) first.
    let mut child = owned::OwnedChild(command.spawn()?);
    let job = owned::Job::new()?;
    job.attach(&child.0)?;
    fs::write(output.join("run-marker"), b"start")?;
    let mut samples = fs::File::create_new(output.join("memory.jsonl"))?;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(90);
    let mut health = Vec::new();
    let mut health_port = None;
    let mut health_complete = !burst;
    if burst {
        let marker = output.join("listen-marker.json");
        while !marker.is_file() {
            if child.0.try_wait()?.is_some() || Instant::now() >= deadline {
                return Err("listener_unavailable".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let marker: Value = serde_json::from_slice(&fs::read(marker)?)?;
        if marker["pid"] != child.0.id() {
            return Err("listener_identity".into());
        }
        let port = marker["port"]
            .as_u64()
            .and_then(|p| u16::try_from(p).ok())
            .filter(|p| *p != 0 && *p != 10100)
            .ok_or("listener_port")?;
        let baseline = health::probe(
            port,
            child.0.id(),
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(5)),
        );
        if baseline["healthy"] != true {
            return Err("listener_warmup".into());
        }
        health.push(baseline);
        health_port = Some(port);
        fs::write(output.join("begin-acl-marker"), b"begin")?;
    }
    let exit = loop {
        if let Some(exit) = child.0.try_wait()? {
            break exit;
        }
        if Instant::now() >= deadline {
            return Err("child_deadline".into());
        }
        if !health_complete {
            if output.join("acl-settled-marker").is_file() {
                fs::write(output.join("health-complete-marker"), b"complete")?;
                health_complete = true;
            } else if let Some(port) = health_port {
                health.push(health::probe(
                    port,
                    child.0.id(),
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_secs(5)),
                ));
            }
        }
        use std::io::Write;
        let measured = owned::memory(&child.0);
        writeln!(
            samples,
            "{}",
            json!({"elapsed_ms":started.elapsed().as_millis(),
            "memory":measured.as_ref().ok(),"error":measured.err().map(|_|"memory_unavailable")})
        )?;
        std::thread::sleep(Duration::from_millis(100));
    };
    let reap_deadline = Instant::now() + Duration::from_secs(5);
    while job.active()? != 0 {
        if Instant::now() >= reap_deadline {
            return Err("owned_job_descendants_remain".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if !exit.success() {
        return Err("child_failed".into());
    }
    let result: Value = serde_json::from_slice(&fs::read(output.join("result.json"))?)?;
    let verified = json!({"isolated_child_exit":exit.code(),"owned_job_active":job.active()?,"health":health,"result":result});
    fs::write(
        output.join("verified.json"),
        serde_json::to_vec_pretty(&verified)?,
    )?;
    if result["completed"] != 6
        || result["burst"] != burst
        || result["parallel"] != parallel
        || result["runnerMode"]
            != (if early {
                "early"
            } else if ignore {
                "ignore"
            } else {
                "stock"
            })
        || (burst && (!health_complete || health.len() < 2))
        || result["live"] != 0
        || result["nativeMode"]
            != (if native_early {
                "early"
            } else if native {
                "late"
            } else {
                "none"
            })
        || result["nativeStderr"] != native_stderr
        || result["nativeRefresh"] != native_refresh
        || result["spawnCount"]
            .as_u64()
            .is_none_or(|count| count != if native { 6 } else { 19 })
        || !result["spawns"].as_array().is_some_and(|spawns| {
            spawns
                .iter()
                .any(|s| s["kind"] == if native { "other" } else { "acl" })
        })
    {
        return Err("acl_lifetime_contract".into());
    }
    println!("{verified}");
    Ok(())
}
