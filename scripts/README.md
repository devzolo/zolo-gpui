# Zolo verification scripts

Run these commands from the library root with a compatible Zolo CLI:

```powershell
zolo run scripts/verify.zolo
zolo run scripts/verify-desktop.zolo
zolo run scripts/verify-dev.zolo
zolo run scripts/verify-core.zolo -- --reify --run --language-root "C:/path/to/zolo-lang"
zolo run scripts/verify-normal-host-drain.zolo
zolo run scripts/verify-entry-reload.zolo
```

Arguments for the script follow `--`. Use `--cli "C:/path/to/zolo.exe"` to
select the CLI under test. Desktop accepts `--backend native` or `--backend llvm`
for copied embedded executables; the host-drain and entry-reload gates exercise
all three supported backends by default. `verify.zolo` also accepts `--async`,
`--async-failure` and `--skip-install`. Window scripts accept `--help`.

These scripts require the language APIs in
[Zolo bd647c2 or a compatible later revision](https://github.com/devzolo/zolo-lang/commit/bd647c2):
`std::cmd::command`, process PID/EOF/close support and matching native
runtimes. The version label alone does not establish support.
The fixture tools use bounded `std::process::run`, filesystem, JSON and hashing
APIs. Their isolated Rust VM executor lives in `resources/vm-runner.rs`;
`--run` needs a Zolo source checkout and its Cargo dependencies.

`support/` is a development tooling package. The test sequence, assertions,
source changes and receipts are written in Zolo. Its source-based Rust adapter
contains Windows window/input/capture and owned-process Job operations. The
actual-window gates require an interactive Windows desktop and Rust/Cargo;
run one window gate at a time. Native preparation is cached by Zolo.

Generated sources, logs, receipts, executables and images stay under `target/`.
Fixture-only native stubs and Lua-local adjustments are explicit in the core
receipt; the actual-window and source-free deployment gates are separate proofs.
