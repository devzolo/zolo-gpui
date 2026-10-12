fn main() {
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
