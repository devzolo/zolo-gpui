const fs=require('node:fs'),cp=require('node:child_process'),path=require('node:path');
// From the project root: node scripts/verify-normal-host-drain.cjs
// Optional: --library PATH --fixture PATH --cli PATH --output-dir PATH --backend vm|native|llvm
const options={};
for(let i=2;i<process.argv.length;i+=2) {
  const key=process.argv[i];
  if(!['--library','--fixture','--cli','--output-dir','--backend'].includes(key)||!process.argv[i+1])throw new Error('Unknown option or missing value: '+key);
  options[key]=process.argv[i+1];
}
const cli=options['--cli']||process.env.ZOLO_BIN||'zolo';
const dir=path.resolve(options['--output-dir']||path.join(__dirname,'../target/normal-host-drain'));
const prepareArgs=[path.join(__dirname,'prepare-normal-host-drain.cjs'),'--output-dir',dir];
if(options['--library'])prepareArgs.push('--library',options['--library']);
if(options['--fixture'])prepareArgs.push('--fixture',options['--fixture']);
const prepare=cp.spawnSync(process.execPath,prepareArgs,{encoding:'utf8',windowsHide:true});
if(prepare.status!==0)throw new Error((prepare.stderr||prepare.stdout||prepare.error?.message||'Fixture preparation failed').trim());
const source=path.join(dir,'main.zolo');
const backends=options['--backend']?[options['--backend']]:['vm','native','llvm'];
if(backends.some(backend=>!['vm','native','llvm'].includes(backend)))throw new Error('Expected --backend vm, native or llvm');
for(const backend of backends) {
  const base=path.join(dir,backend);
  let build={status:0};
  if(backend!=='vm')build=cp.spawnSync(cli,['build',source,'--emit',backend,'--release','--plugins','dynamic','-o',base],{encoding:'utf8',windowsHide:true,timeout:180000,maxBuffer:4*1024*1024});
  if(build.status!==0){fs.writeFileSync(base+'.build.log',(build.stdout||'')+(build.stderr||''));console.log(JSON.stringify({backend,build_status:build.status,error:build.error?.message}));process.exitCode=1;continue;}
  const run=backend==='vm'?cp.spawnSync(cli,['run',source,'--no-cache','--no-build-script'],{encoding:'utf8',windowsHide:true,timeout:30000,maxBuffer:4*1024*1024}):cp.spawnSync(base+'.exe',[],{encoding:'utf8',windowsHide:true,timeout:30000,maxBuffer:4*1024*1024});
  fs.writeFileSync(base+'.run.log',(run.stdout||'')+(run.stderr||''));
  const stdout=(run.stdout||'').trim().replaceAll('\r','');
  const lines=stdout.split('\n');
  const passed=run.status===0&&[0,1,2,3].every(mode=>lines.includes('HOST:DRAIN:PASS:'+mode)&&lines.includes('HOST:RECEIPT:'+mode+':main=true:child_started=true:child=true:late=0'));
  const receipt={backend,status:run.status,stdout,passed,error:run.error?.message};
  fs.writeFileSync(base+'.json',JSON.stringify(receipt,null,2));console.log(JSON.stringify(receipt));if(!receipt.passed)process.exitCode=1;
}
