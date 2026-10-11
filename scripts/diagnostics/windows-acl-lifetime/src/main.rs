#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
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
    if args.len() != 3 {
        return Err("BUN INSTALLED_SOURCE NEW_OUTPUT".into());
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
const {existsSync,writeFileSync}=await import('node:fs');
while(!existsSync('run-marker'))await Bun.sleep(1);
const {join,basename}=await import('node:path');const {pathToFileURL}=await import('node:url');
const {heapStats}=await import('bun:jsc');
const load=path=>import(pathToFileURL(join(process.env.ACL_SOURCE,path)).href);
const acl=await load('src/lib/windows-secret-acl.ts');
const weak=[];const spawns=[];const original=Bun.spawn;
// Return the exact subprocess. Do not touch stdout, exited, or any proc getter:
// the default runner must keep its original exit-then-read order.
Bun.spawn=function(...args){
 const start=performance.now();const proc=Reflect.apply(original,Bun,args);
 weak.push(new WeakRef(proc));
 const executable=basename(Array.isArray(args[0])?args[0][0]:'').toLowerCase();
 spawns.push({kind:executable==='icacls.exe'?'acl':executable==='powershell.exe'?'principal':'other',spawn_ms:performance.now()-start});
 return proc;
};
const classes=['Subprocess','ReadableStream','Promise','ArrayBuffer','Uint8Array'];
const phase=()=>{
 const h=heapStats(),m=process.memoryUsage();const counts={};
 for(const name of classes)counts[name]=Object.hasOwn(h.objectTypeCounts,name)?h.objectTypeCounts[name]:null;
 return{external:m.external,arrayBuffers:m.arrayBuffers,heapUsed:m.heapUsed,heapSize:h.heapSize,objectCount:h.objectCount,classes:counts};
};
const collect=async()=>{for(let i=0;i<5;i++){await Bun.sleep(10);Bun.gc(true)}};
const before=phase();let completed=0;
try{
 for(let i=0;i<6;i++){
  const path=join(process.cwd(),'synthetic-'+i+'.txt');writeFileSync(path,'synthetic small ACL input');
  const result=await acl.hardenSecretPathAsync(path,{required:true,deadlineMs:15000});
  if(result.ok!==true)throw Error('acl_not_ok');completed++;
 }
}finally{Bun.spawn=original}
const settled=phase();await collect();const collected=phase();
const live=weak.filter(ref=>ref.deref()!==undefined).length;
console.log(JSON.stringify({bun:Bun.version,completed,spawnCount:weak.length,spawns,before,settled,collected,live,
 scope:'six small files through unmodified default async ACL runner; same proc returned without getters; isolated child GC only, not production leak or stall proof'}));
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
    let exit = loop {
        if let Some(exit) = child.0.try_wait()? {
            break exit;
        }
        if Instant::now() >= deadline {
            return Err("child_deadline".into());
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
    let verified =
        json!({"isolated_child_exit":exit.code(),"owned_job_active":job.active()?,"result":result});
    fs::write(
        output.join("verified.json"),
        serde_json::to_vec_pretty(&verified)?,
    )?;
    if result["completed"] != 6
        || result["live"] != 0
        || result["spawnCount"].as_u64().is_none_or(|count| count < 7)
        || !result["spawns"]
            .as_array()
            .is_some_and(|spawns| spawns.iter().any(|s| s["kind"] == "acl"))
    {
        return Err("acl_lifetime_contract".into());
    }
    println!("{verified}");
    Ok(())
}
