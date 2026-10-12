const fs=require('node:fs'),cp=require('node:child_process'),path=require('node:path');
const options={};
for(let i=2;i<process.argv.length;i+=2) {
  if(!['--library','--fixture','--cli','--output-dir','--backend'].includes(process.argv[i])||!process.argv[i+1])throw Error('Unknown option or missing value');
  options[process.argv[i]]=process.argv[i+1];
}
const dir=path.resolve(options['--output-dir']||path.join(__dirname,'../target/entry-reload'));
const args=[path.join(__dirname,'prepare-entry-reload.cjs'),'--output-dir',dir];
for(const key of ['--library','--fixture'])if(options[key])args.push(key,options[key]);
const prep=cp.spawnSync(process.execPath,args,{encoding:'utf8',windowsHide:true});
if(prep.status!==0)throw Error(prep.stderr||prep.stdout||prep.error?.message);
const cli=options['--cli']||process.env.ZOLO_BIN||'zolo';
const backends=options['--backend']?[options['--backend']]:['vm','native','llvm'];
const source=path.join(dir,'main.zolo');
for(const backend of backends) {
  const base=path.join(dir,backend);
  if(backend!=='vm') {
    const build=cp.spawnSync(cli,['build',source,'--emit',backend,'--release','--plugins','dynamic','-o',base],{encoding:'utf8',windowsHide:true,timeout:180000,maxBuffer:4*1024*1024});
    fs.writeFileSync(base+'.build.log',(build.stdout||'')+(build.stderr||''));
    if(build.status!==0){console.log(JSON.stringify({backend,build_status:build.status,error:build.error?.message}));process.exitCode=1;continue;}
  }
  const run=backend==='vm'?cp.spawnSync(cli,['run',source,'--no-cache','--no-build-script'],{encoding:'utf8',windowsHide:true,timeout:30000,maxBuffer:4*1024*1024}):cp.spawnSync(base+'.exe',[],{encoding:'utf8',windowsHide:true,timeout:30000,maxBuffer:4*1024*1024});
  const stdout=(run.stdout||'').trim().replaceAll('\r','');
  const passed=run.status===0&&['ENTRY:REJECTED:PASS','ENTRY:MIXED_BATCH:PASS','ENTRY:ACCEPTED:PASS','ENTRY:IDLE:PASS','ENTRY:MULTIPLE_REPLAYS:PASS','ENTRY:MODULE_INVALIDATION:PASS','ENTRY:WINDOW_RETRY:PASS','ENTRY:RETIRED:PASS','ENTRY:LEGACY:PASS','ENTRY:TRANSACTION:PASS'].every(line=>stdout.split('\n').includes(line));
  const receipt={backend,status:run.status,stdout,passed,error:run.error?.message};
  fs.writeFileSync(base+'.run.log',(run.stdout||'')+(run.stderr||''));
  fs.writeFileSync(base+'.json',JSON.stringify(receipt,null,2));console.log(JSON.stringify(receipt));if(!passed)process.exitCode=1;
}
