// Pure private-controller gate; native rendering is stubbed explicitly below.
// Retained-library usage: --library <zolo-gpui> --language-root <zolo-lang>
// --cli <fresh-compiled-zolo> --reify --run. Rust builds the raw VM
// helper from the language checkout. All transformations affect fixtures only.
const fs=require('node:fs'),cp=require('node:child_process'),path=require('node:path');
const args=process.argv.slice(2);
function option(name) {
  const flag='--'+name,index=args.findIndex(arg=>arg===flag||arg.startsWith(flag+'='));
  if(index<0)return undefined;
  const value=args[index].startsWith(flag+'=')?args[index].slice(flag.length+1):args[index+1];
  if(!value||value.startsWith('--'))throw new Error(flag+' requires a path');
  return value;
}
const libraryOption=option('library');
const libraryRoot=libraryOption?path.resolve(libraryOption):undefined;
const languageRoot=path.resolve(option('language-root')||path.resolve(__dirname,'../../..'));
const sourceDir=libraryRoot?path.join(libraryRoot,'src'):__dirname;
const testsDir=libraryRoot?path.join(libraryRoot,'tests/pure'):__dirname;
const testPath=name=>path.join(testsDir,name+(libraryRoot?'.fixture':''));
const fixtureDir=libraryRoot?path.join(libraryRoot,'target/pure-core'):__dirname;
const fixturePath=name=>path.join(fixtureDir,name);
fs.mkdirSync(fixtureDir,{recursive:true});
const cli=path.resolve(option('cli')||process.env.ZOLO_GPUI_TEST_CLI||path.join(languageRoot,'target/debug/zolo'+(process.platform==='win32'?'.exe':'')));
if(!fs.existsSync(cli))throw new Error('Build the current Zolo CLI, or supply --cli <path> / ZOLO_GPUI_TEST_CLI: '+cli);
const cliHash=require('node:crypto').createHash('sha256').update(fs.readFileSync(cli)).digest('hex');
const cliReceipt=`GPUI_PURE_CLI path=${cli}; sha256=${cliHash}; modified=${fs.statSync(cli).mtime.toISOString()}`;
const inputReceipt=`GPUI_PURE_INPUT source=${sourceDir}; tests=${testsDir}; language=${languageRoot}; artifacts=${fixtureDir}`;
console.log(cliReceipt);
console.log(inputReceipt);
let text=fs.readFileSync(path.join(sourceDir,'lib.zolo'),'utf8');
text=text.replace(/^use plugin.*$/mg,'').replace(/extern rust deps \{[\s\S]*?\}/,'').replace(/extern rust from[^\{]+\{[\s\S]*?\}/,'');
// Preliminary ordinary-library check while the reification worker is active.
// This is not a retained-schema verification gate.
if(!process.argv.includes('--reify')) text=text.replace(/@reify\(T\)/g,'').replace('typeinfo(T).key','"preliminary-schema"');
text+=`\nfn native_run(title:str,width:float,height:float,updates:bool,render:fn()->[NativeNode],dispatch:fn(int)->Any,update:fn()->NativePoll,close:fn()->bool) {}
fn native_smoke(render:fn()->[NativeNode],dispatch:fn(int)->Any,update:fn()->NativePoll,close:fn()->bool,x:float,y:float) {}
fn native_probe(render:fn()->[NativeNode],dispatch:fn(int)->Any,event:int)->Any {return render()}
fn native_desktop(initial:[DesktopWindow],updates:bool,render:fn(int)->[DesktopNode],dispatch:fn([DesktopEvent])->nil,advance:fn()->DesktopTurn,close:fn(int)->bool) {}
var injected_native_validation_error = ""
var injected_native_validation_hook: (fn() -> void)? = nil
fn native_validate(_nodes:[DesktopNode])->str { if injected_native_validation_hook != nil { injected_native_validation_hook() }; return injected_native_validation_error }
`;
fs.writeFileSync(fixturePath('core-check.zolo'),text);
if(process.argv.includes('--run')) {
  if(!fs.existsSync(path.join(languageRoot,'crates/zolo-vm/Cargo.toml'))||!fs.existsSync(path.join(languageRoot,'crates/zolo-std/Cargo.toml')))
    throw new Error('--run requires a Zolo language checkout with Rust sources; supply --language-root <checkout>');
  // These independent fixtures share a chunk only to expose private helpers.
  // Top-level mutable test bindings avoid Lua's 200-local chunk limit; real
  // library consumers load each module in its own chunk.
  text+='\n'+fs.readFileSync(testPath('lifecycle-tests.zolo'),'utf8').replace(/^let /mg,'var ');
  if(process.argv.includes('--pending-generic-fix')) {
    console.log('TEMPORARY: Entity generic method gate omitted until compiler receiver fix; all schema gates remain active.');
    text=text.replace('entity_cell.update(|number| number + 1)','').replace('assert(entity_cell.read() == 5)','');
  }
  fs.writeFileSync(fixturePath('core-check.zolo'),text);
  const compiled=cp.spawnSync(cli,['compile','--stdin','--no-build-script'],{input:text,encoding:'utf8',windowsHide:true,maxBuffer:10*1024*1024});
  fs.writeFileSync(fixturePath('core-check.stderr'),compiled.stderr||'');
  if(compiled.status!==0){console.error(compiled.stderr||compiled.error);process.exitCode=compiled.status??1;return;}
  // This private-helper fixture intentionally flattens independent modules.
  // Real consumers load them separately; Lua's 200-local *chunk* limit would
  // otherwise reject the fixture before any assertion executes. Only its
  // generated column-zero declarations become globals in this isolated VM.
  // All generated function bodies, schema values and scheduler code are kept.
  let fixtureGlobals=0;
  const lua=(compiled.stdout||'').replace(/^local ([^\r\n]*)/mg,(_,declaration)=>{
    fixtureGlobals++;
    if(declaration.startsWith('function ')||declaration.includes('='))return declaration;
    return declaration+' = '+declaration.split(',').map(()=> 'nil').join(', ');
  });
  fs.writeFileSync(fixturePath('core-check.lua'),lua);
  const fixtureReceipt=`FIXTURE_ONLY flattened_chunk_globals=${fixtureGlobals}; generated bodies/reified schemas unchanged; native functions stubbed`;
  console.log(fixtureReceipt);
  const root=languageRoot;
  const runnerDir=fixturePath('core-runner');
  fs.mkdirSync(path.join(runnerDir,'src'),{recursive:true});
  const manifest=`[package]\nname="gpui-core-fixture"\nversion="0.0.0"\nedition="2024"\n[workspace]\n[dependencies]\nzolo-vm={path=${JSON.stringify(path.join(root,'crates/zolo-vm').replaceAll('\\','/'))}}\nzolo-std={path=${JSON.stringify(path.join(root,'crates/zolo-std').replaceAll('\\','/'))}}\n`;
  fs.writeFileSync(path.join(runnerDir,'Cargo.toml'),manifest);
  fs.writeFileSync(path.join(runnerDir,'src/main.rs'),`fn main() {
    std::thread::Builder::new().stack_size(64 * 1024 * 1024)
        .spawn(run).expect("fixture VM thread").join().expect("fixture VM thread completed");
}
fn run() {
    let path = std::env::args().nth(1).expect("fixture Lua path");
    let source = std::fs::read_to_string(&path).expect("fixture Lua source");
    let mut state = zolo_vm::new_state();
    zolo_std::open(&mut state);
    if let Err(error) = state.load_string(&source, &format!("@{}", path))
        .and_then(|_| state.call(0, zolo_vm::types::LUA_MULTRET)) {
        eprintln!("GPUI_PURE_VM: {:?}: {}", error, state.to_str(-1).unwrap_or_default());
        std::process::exit(1);
    }
}
`);
  if(!fs.existsSync(path.join(runnerDir,'Cargo.lock')))fs.copyFileSync(path.join(root,'Cargo.lock'),path.join(runnerDir,'Cargo.lock'));
  const built=cp.spawnSync('cargo',['build','--offline','--manifest-path',path.join(runnerDir,'Cargo.toml'),'--target-dir',path.join(root,'target')],{encoding:'utf8',windowsHide:true,maxBuffer:10*1024*1024});
  if(built.status!==0){console.error(built.stderr||built.error);process.exitCode=built.status??1;return;}
  const runner=path.join(root,'target/debug/gpui-core-fixture'+(process.platform==='win32'?'.exe':''));
  const run=cp.spawnSync(runner,[fixturePath('core-check.lua')],{encoding:'utf8',windowsHide:true,maxBuffer:10*1024*1024});
  fs.writeFileSync(fixturePath('lifecycle-receipt.log'),cliReceipt+'\n'+inputReceipt+'\n'+fixtureReceipt+'\n'+(run.stdout||'')+(run.stderr||''));
  console.log(run.stdout||''); console.log(run.stderr||''); if(run.error)console.error(run.error);console.log('lifecycle exit='+run.status);process.exitCode=run.status??1;return;
}
const result=cp.spawnSync(cli,['compile','--stdin','--no-build-script'],{input:text,encoding:'utf8',windowsHide:true,maxBuffer:10*1024*1024});
fs.writeFileSync(fixturePath('core-check.lua'),result.stdout||'');
fs.writeFileSync(fixturePath('core-check.stderr'),result.stderr||'');
console.log(result.stderr||'');if(result.error)console.error(result.error);console.log('core compile exit='+result.status);process.exitCode=result.status??1;
