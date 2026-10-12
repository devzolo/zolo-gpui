const fs=require('node:fs'),cp=require('node:child_process'),path=require('node:path');
const options={};
for(let i=2;i<process.argv.length;i+=2) {
  if(!['--library','--fixture','--output-dir'].includes(process.argv[i])||!process.argv[i+1])throw Error('Unknown option or missing value');
  options[process.argv[i]]=process.argv[i+1];
}
const dir=path.resolve(options['--output-dir']||path.join(__dirname,'../target/entry-reload'));
const args=[path.join(__dirname,'prepare-normal-host-drain.cjs'),'--output-dir',dir];
if(options['--library'])args.push('--library',options['--library']);
const prep=cp.spawnSync(process.execPath,args,{encoding:'utf8',windowsHide:true});
if(prep.status!==0)throw Error(prep.stderr||prep.stdout||prep.error?.message);
const source=path.join(dir,'group_host.zolo');
let library=fs.readFileSync(source,'utf8').replace(/^use std::ui::.*$/m,'use std::ui::{collect}');
library=library.replace(/fn native_desktop\(initial:[\s\S]*?\n\}/,`fn native_desktop(initial:[DesktopWindow],updates:bool,render:fn(int)->[DesktopNode],dispatch:fn([DesktopEvent])->nil,advance:fn()->DesktopTurn,close:fn(int)->bool) {
    reload_render = render
    reload_dispatch = dispatch
    reload_advance = advance
    let action = reload_host_action ?? panic("missing host scenario")
    action()
}`);
library=library.replace(/fn native_smoke\([^\n]*\) \{\}/,`fn native_smoke(render:fn()->[NativeNode],dispatch:fn(int)->Any,update:fn()->NativePoll,close:fn()->bool,x:float,y:float) {
    legacy_render = render
    legacy_update = update
    let action = legacy_host_action ?? panic("missing smoke scenario")
    action()
    while close() { sleep(1) }
}`);
library+=`
var reload_host_action: (fn() -> Any)? = nil
var reload_actions: [fn() -> Any] = []
var reload_render: (fn(int) -> [DesktopNode])? = nil
var reload_dispatch: (fn([DesktopEvent]) -> nil)? = nil
var reload_advance: (fn() -> DesktopTurn)? = nil
var legacy_host_action: (fn() -> Any)? = nil
var legacy_render: (fn() -> [NativeNode])? = nil
var legacy_update: (fn() -> NativePoll)? = nil
fn begin_updates() -> bool { return true }
fn end_updates() {}
fn poll_updates(before_replay: (fn() -> Any)? = nil, after_replay: (fn(Any, bool) -> Any)? = nil) -> bool {
    let actions = reload_actions
    reload_actions = []
    var changed = false
    for action in actions {
        let token: Any = if before_replay == nil { nil } else { before_replay() }
        var succeeded = true
        try { action() } catch _ { succeeded = false }
        if after_replay != nil { after_replay(token, succeeded) }
        if succeeded { changed = true }
    }
    return changed
}
pub fn install_reload_host(action: fn() -> Any) { reload_host_action = action }
pub fn queue_entry(action: fn() -> Any) { reload_actions.push(action) }
pub fn advance_once() -> DesktopTurn { return (reload_advance ?? panic("missing host"))() }
pub fn render_now(window: Window) -> [DesktopNode] { return (reload_render ?? panic("missing render"))(window_data(window).id) }
pub fn click_now(window: Window) {
    let dispatch = reload_dispatch ?? panic("missing dispatch")
    let event = DesktopEvent.new(window: window_data(window).id, node: "root", kind: "click", text: "", number: 0.0, checked: false, key: "")
    dispatch([event])
}
pub fn start_tasks(window: Window) { window_data(window).tasks.poll() }
pub fn window_id(window: Window) -> int { return window_data(window).id }
pub fn install_legacy_host(action: fn() -> Any) { legacy_host_action = action }
pub fn legacy_render_now() -> [NativeNode] { return (legacy_render ?? panic("missing legacy render"))() }
pub fn legacy_advance_once() -> NativePoll { return (legacy_update ?? panic("missing legacy update"))() }
pub fn dispose_test_app(app: App) {
    let outcome = drain_application(app, app_data(app))
    app_registry.remove(app)
    assert(!outcome.failed, "test app cleanup must succeed")
}
`;
fs.writeFileSync(source,library);
const defaultFixture=fs.existsSync(path.join(__dirname,'../tests/entry-reload.zolo'))?path.join(__dirname,'../tests/entry-reload.zolo'):path.join(__dirname,'entry-reload.zolo');
fs.copyFileSync(path.resolve(options['--fixture']||defaultFixture),path.join(dir,'main.zolo'));
console.log('Prepared entry reload publication oracle in '+dir);
