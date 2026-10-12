const fs=require('node:fs'),path=require('node:path');
// scripts/prepare-normal-host-drain.cjs --library src/lib.zolo --output-dir target/normal-host-drain
const options={};
for(let i=2;i<process.argv.length;i+=2) {
  const key=process.argv[i];
  if(!['--library','--fixture','--output-dir'].includes(key)||!process.argv[i+1])throw new Error('Expected --library PATH, --fixture PATH or --output-dir PATH');
  options[key]=process.argv[i+1];
}
const defaultLibrary=fs.existsSync(path.join(__dirname,'../src/lib.zolo'))?path.join(__dirname,'../src/lib.zolo'):path.join(__dirname,'lib.zolo');
const libraryPath=path.resolve(options['--library']||defaultLibrary);
const defaultFixture=fs.existsSync(path.join(__dirname,'../tests/normal-host-drain.zolo'))?path.join(__dirname,'../tests/normal-host-drain.zolo'):path.join(__dirname,'normal-host-drain.zolo');
const fixturePath=path.resolve(options['--fixture']||defaultFixture);
const dir=path.resolve(options['--output-dir']||path.join(__dirname,'../target/normal-host-drain'));
fs.mkdirSync(dir,{recursive:true});
let library=fs.readFileSync(libraryPath,'utf8');
library=library.replace(/^use plugin.*$/mg,'').replace(/extern rust deps \{[\s\S]*?\}/,'').replace(/extern rust from[^\{]+\{[\s\S]*?\}/,'');
library+=`
var injected_host_action: (fn() -> Any)? = nil
var injected_host_failure = false
pub fn install_host_action(action: fn() -> Any) { injected_host_action = action }
pub fn fail_host_once() { injected_host_failure = true }
fn native_run(title:str,width:float,height:float,updates:bool,render:fn()->[NativeNode],dispatch:fn(int)->Any,update:fn()->NativePoll,close:fn()->bool) {}
fn native_smoke(render:fn()->[NativeNode],dispatch:fn(int)->Any,update:fn()->NativePoll,close:fn()->bool,x:float,y:float) {}
fn native_probe(render:fn()->[NativeNode],dispatch:fn(int)->Any,event:int)->Any { return render() }
fn native_desktop(initial:[DesktopWindow],updates:bool,render:fn(int)->[DesktopNode],dispatch:fn([DesktopEvent])->nil,advance:fn()->DesktopTurn,close:fn(int)->bool) {
    render(initial[0].id)
    advance()
    let action = injected_host_action ?? panic("missing injected host action")
    action()
    advance()
    while close(initial[0].id) { sleep(1) }
    // Simulate the platform exiting after its last physical window closes,
    // before its next asynchronous close/drain callback can run.
    print("HOST:NORMAL:RETURN")
    if injected_host_failure {
        injected_host_failure = false
        error("original host failure", 0)
    }
}
fn native_validate(_nodes:[DesktopNode])->str { return "" }
`;
fs.writeFileSync(path.join(dir,'group_host.zolo'),library);
fs.copyFileSync(fixturePath,path.join(dir,'main.zolo'));
console.log('Prepared real-library non-GUI host-return drain oracle in '+dir);
