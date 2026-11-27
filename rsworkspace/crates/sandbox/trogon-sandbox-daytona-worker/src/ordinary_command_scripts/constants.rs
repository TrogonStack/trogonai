/// Prints the platform, architecture and Node version of the sandbox as JSON.
pub(crate) const PLATFORM_PROBE: &str = "process.stdout.write(JSON.stringify({platform:process.platform,architecture:process.arch,node_major:Number(process.versions.node.split('.')[0]),node_minor:Number(process.versions.node.split('.')[1])}))";

/// Creates the runtime directory, which must not exist yet, and records the nonce that owns it. Arguments: the
/// directory and the nonce.
pub(crate) const CREATE_ROOT: &str = "try { const f=require('node:fs');f.mkdirSync(process.argv[1],{mode:0o755});f.writeFileSync(process.argv[1]+'/owner',process.argv[2],{flag:'wx',mode:0o600});process.stdout.write('created'); } catch { process.exit(78); }";

/// Removes the runtime directory when its owner file still holds the nonce. A directory that is already gone counts
/// as removed. Arguments: the directory and the nonce.
pub(crate) const REMOVE_ROOT: &str = "try { const f=require('node:fs'),p=process.argv[1],n=process.argv[2];if(!f.existsSync(p)){process.stdout.write('absent');process.exit(0);}const s=f.lstatSync(p),o=f.lstatSync(p+'/owner');if(!s.isDirectory()||s.isSymbolicLink()||!o.isFile()||o.isSymbolicLink()||o.size!==n.length||f.readFileSync(p+'/owner','utf8')!==n)process.exit(78);f.rmSync(p,{recursive:true,force:false});process.stdout.write('absent');}catch{process.exit(78);}";

/// Re-reads the installed supervisor and bridge without following links, checks their digests, and asks the
/// supervisor to probe its kernel prerequisites. Arguments: supervisor path, its digest, bridge path, its digest.
pub(crate) const VERIFY_RUNTIME: &str = r#"try {
const fs=require('node:fs'),crypto=require('node:crypto'),cp=require('node:child_process');
function checked(path,hash,bound){
const fd=fs.openSync(path,fs.constants.O_RDONLY|fs.constants.O_NOFOLLOW),stat=fs.fstatSync(fd);
if(!stat.isFile()||stat.size>bound)throw 0;
const data=Buffer.alloc(bound+1);let n=0;
while(n<data.length){const r=fs.readSync(fd,data,n,data.length-n,null);if(!r)break;n+=r;}fs.closeSync(fd);
if(n>bound||crypto.createHash('sha256').update(data.subarray(0,n)).digest('hex')!==hash)throw 0;
}
checked(process.argv[1],process.argv[2],16777216);checked(process.argv[3],process.argv[4],4194304);
const probe=cp.spawnSync(process.argv[1],['--probe'],{encoding:'utf8',timeout:10000,maxBuffer:65536});
if(probe.status!==0||probe.stderr.length)throw 0;
const value=JSON.parse(probe.stdout);
if(Object.keys(value).sort().join(',')!=='kind,supported,version'||value.kind!=='probe'||value.version!==1||value.supported!==true)throw 0;
process.stdout.write('verified');
}catch{process.exit(78);}"#;
